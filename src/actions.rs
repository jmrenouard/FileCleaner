//! Ordered lifecycle action pipeline, pure-Rust compression, atomic `.fc-tmp`
//! staging, and cross-filesystem SHA-256 verified file transfers.

use crate::config::{ActionStep, CompressionFormat, RuleConfig};
use crate::error::{FileCleanerError, Result};
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Datelike, Timelike, Utc};
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const DEFAULT_RENAME_TEMPLATE: &str = "%Y%m%d_%H%M%S";
const COPY_BUF_SIZE: usize = 64 * 1024;
const DEFAULT_BROTLI_QUALITY: u32 = 6;
const BROTLI_WINDOW_BITS: u32 = 22;
const BROTLI_BUF_SIZE: usize = 4096;

static TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Mutable candidate state propagated sequentially across ordered `RuleConfig::actions` steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingTarget {
    /// Current path of the candidate artifact.
    pub path: PathBuf,
    /// Current byte size of the candidate artifact.
    pub size_bytes: u64,
    /// Original modification timestamp preserved across transforms.
    pub mtime: DateTime<Utc>,
    /// Optional original access timestamp preserved across transforms.
    pub atime: Option<DateTime<Utc>>,
}

/// Summary produced after executing (or simulating) an action pipeline for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionReport {
    /// Initial candidate path before any action ran.
    pub original_path: PathBuf,
    /// Final destination or archive path (`None` when terminated by `ActionStep::Delete`).
    pub final_path: Option<PathBuf>,
    /// Ordered step labels executed (`RenameByDate`, `Compress(Gzip)`, `Move`, etc.).
    pub steps_executed: Vec<String>,
    /// Lowercase hex SHA-256 digest verified during cross-filesystem `Move`/`Trash`/`CopyTruncate`.
    pub sha256: Option<String>,
    /// Bytes freed from the monitored source location (`0` in dry-run mode).
    pub bytes_freed: u64,
    /// Whether execution occurred in read-only dry-run simulation mode.
    pub dry_run: bool,
}

/// Stateless executor implementing FileCleaner's crash-safe action pipeline.
#[derive(Debug, Default, Clone, Copy)]
pub struct ActionExecutor;

/// RAII cleanup guard ensuring partial `.fc-tmp.*` staging artifacts are removed on failure.
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl ActionExecutor {
    /// Executes `rule.actions` sequentially against `target`.
    ///
    /// When `dry_run || rule.dry_run` is `true`, performs zero filesystem mutations
    /// and creates zero `.fc-tmp` staging artifacts while projecting `final_path`
    /// and `steps_executed` consistently.
    pub fn execute_pipeline(
        mut target: WorkingTarget,
        rule: &RuleConfig,
        global_trash_dir: Option<&Path>,
        dry_run: bool,
    ) -> Result<ActionReport> {
        let effective_dry_run = dry_run || rule.dry_run;
        if rule.actions.is_empty() {
            return Err(FileCleanerError::ConfigValidation(format!(
                "rule '{}' must specify at least one action step",
                rule.name
            )));
        }

        for (idx, step) in rule.actions.iter().enumerate() {
            if matches!(step, ActionStep::Delete | ActionStep::Trash)
                && idx + 1 < rule.actions.len()
            {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{}' has terminal action {step:?} followed by additional steps",
                    rule.name
                )));
            }
        }

        let original_path = target.path.clone();
        let original_size = target.size_bytes;
        let mut remaining_in_source = original_size;
        let mut final_path: Option<PathBuf> = Some(original_path.clone());
        let mut steps_executed = Vec::with_capacity(rule.actions.len());
        let mut verified_sha256: Option<String> = None;

        for &step in &rule.actions {
            match step {
                ActionStep::RenameByDate => {
                    let dest = compute_rename_destination(&target.path, target.mtime, rule)?;
                    if dest != target.path && fs::symlink_metadata(&dest).is_ok() {
                        return Err(FileCleanerError::io(
                            &dest,
                            io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "rename destination already exists",
                            ),
                        ));
                    }
                    if !effective_dry_run && dest != target.path {
                        reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                        fs::rename(&target.path, &dest)
                            .map_err(|err| FileCleanerError::io(&dest, err))?;
                        restore_file_times(&dest, target.mtime, target.atime, None)?;
                    }
                    target.path = dest.clone();
                    final_path = Some(dest);
                    steps_executed.push("RenameByDate".to_string());
                }
                ActionStep::Compress => {
                    if !effective_dry_run {
                        reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                    }
                    if has_compressed_extension(&target.path) {
                        final_path = Some(target.path.clone());
                        remaining_in_source = target.size_bytes;
                        steps_executed.push(format!("Compress({:?})", rule.compression));
                        continue;
                    }
                    let dest = compute_compressed_path(&target.path, rule.compression);
                    if fs::symlink_metadata(&dest).is_ok() && !rule.compress_overwrite {
                        return Err(FileCleanerError::io(
                            &dest,
                            io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "compressed destination already exists",
                            ),
                        ));
                    }
                    if !effective_dry_run {
                        let compressed_size = compress_file_staged(
                            &target,
                            &dest,
                            rule.compression,
                            rule.compression_level,
                        )?;
                        if rule.compress_remove_source {
                            reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                            fs::remove_file(&target.path)
                                .map_err(|err| FileCleanerError::io(&target.path, err))?;
                            target.path = dest.clone();
                            target.size_bytes = compressed_size;
                            remaining_in_source = compressed_size;
                        }
                    } else if rule.compress_remove_source {
                        target.path = dest.clone();
                    }
                    final_path = Some(dest);
                    steps_executed.push(format!("Compress({:?})", rule.compression));
                }
                ActionStep::Move => {
                    let base_dir = rule.move_to.as_deref().ok_or_else(|| {
                        FileCleanerError::ConfigValidation(format!(
                            "rule '{}' includes ActionStep::Move without move_to",
                            rule.name
                        ))
                    })?;
                    let dest_dir = resolve_move_dest_dir(
                        base_dir,
                        rule.move_date_subdir.as_deref(),
                        target.mtime,
                        &rule.name,
                    )?;
                    let file_name = extract_file_name(&target.path)?;
                    let dest = dest_dir.join(file_name);

                    if dest != target.path
                        && fs::symlink_metadata(&dest).is_ok()
                        && !rule.move_overwrite
                    {
                        return Err(FileCleanerError::io(
                            &dest,
                            io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "move destination already exists",
                            ),
                        ));
                    }

                    if !effective_dry_run {
                        reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                        if let Some(digest) = Self::move_with_cross_fs_fallback(
                            &target.path,
                            &dest,
                            target.mtime,
                            target.atime,
                            rule.move_overwrite,
                            false,
                        )? {
                            verified_sha256 = Some(digest);
                        }
                        remaining_in_source = 0;
                    }
                    target.path = dest.clone();
                    final_path = Some(dest);
                    steps_executed.push("Move".to_string());
                }
                ActionStep::CopyTruncate => {
                    let dest_dir = rule
                        .copy_truncate_to
                        .as_deref()
                        .or(rule.move_to.as_deref())
                        .ok_or_else(|| {
                            FileCleanerError::ConfigValidation(format!(
                                "rule '{}' includes ActionStep::CopyTruncate without copy_truncate_to or move_to",
                                rule.name
                            ))
                        })?;
                    let compress_fmt = if rule.compression != CompressionFormat::Gzip
                        || rule.compression_level.is_some()
                    {
                        Some(rule.compression)
                    } else {
                        None
                    };
                    let archive_dest = compute_copy_truncate_archive_path(
                        &target.path,
                        dest_dir,
                        target.mtime,
                        rule.rename_template.as_deref(),
                        compress_fmt,
                    )?;

                    if !effective_dry_run {
                        let sha = Self::execute_copy_truncate(
                            &target,
                            &archive_dest,
                            compress_fmt,
                            rule.compression_level,
                        )?;
                        if sha.is_some() {
                            verified_sha256 = sha;
                        }
                        target.size_bytes = 0;
                        remaining_in_source = 0;
                    }
                    final_path = Some(archive_dest);
                    steps_executed.push("CopyTruncate".to_string());
                }
                ActionStep::Trash => {
                    let trash_dir =
                        rule.trash_dir
                            .as_deref()
                            .or(global_trash_dir)
                            .ok_or_else(|| {
                                FileCleanerError::ConfigValidation(format!(
                                    "rule '{}' includes ActionStep::Trash without trash_dir or global_trash_dir",
                                    rule.name
                                ))
                            })?;
                    let file_name = extract_file_name(&target.path)?;
                    let dest = unique_collision_free_path(trash_dir, file_name);
                    if !effective_dry_run {
                        reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                        if let Some(digest) = Self::move_with_cross_fs_fallback(
                            &target.path,
                            &dest,
                            target.mtime,
                            target.atime,
                            false,
                            false,
                        )? {
                            verified_sha256 = Some(digest);
                        }
                        remaining_in_source = 0;
                    }
                    target.path = dest.clone();
                    final_path = Some(dest);
                    steps_executed.push("Trash".to_string());
                }
                ActionStep::Delete => {
                    if !effective_dry_run {
                        reject_symlink_if_disallowed(&target.path, rule.follow_symlinks)?;
                        fs::remove_file(&target.path)
                            .map_err(|err| FileCleanerError::io(&target.path, err))?;
                        remaining_in_source = 0;
                    }
                    final_path = None;
                    steps_executed.push("Delete".to_string());
                }
            }
        }

        let bytes_freed = if effective_dry_run {
            0
        } else {
            original_size.saturating_sub(remaining_in_source)
        };

        Ok(ActionReport {
            original_path,
            final_path,
            steps_executed,
            sha256: verified_sha256,
            bytes_freed,
            dry_run: effective_dry_run,
        })
    }

    /// Moves `src` to `dest`, using `std::fs::rename` on the fast path and falling back
    /// to staged `.fc-tmp` copy + SHA-256 re-read verification on `CrossesDevices` (`EXDEV`)
    /// or whenever `force_cross_fs` is `true`.
    ///
    /// Returns `Ok(Some(sha256_hex))` when the verified cross-filesystem transfer ran,
    /// or `Ok(None)` when the fast same-filesystem rename succeeded.
    pub fn move_with_cross_fs_fallback(
        src: &Path,
        dest: &Path,
        mtime: DateTime<Utc>,
        atime: Option<DateTime<Utc>>,
        overwrite: bool,
        force_cross_fs: bool,
    ) -> Result<Option<String>> {
        Self::move_with_cross_fs_fallback_hook(
            src,
            dest,
            mtime,
            atime,
            overwrite,
            force_cross_fs,
            |_| Ok(()),
        )
    }

    /// Cross-filesystem move helper accepting an inspection/tamper hook executed after
    /// staging and `sync_all()` but before independent SHA-256 re-read verification.
    pub fn move_with_cross_fs_fallback_hook<F>(
        src: &Path,
        dest: &Path,
        mtime: DateTime<Utc>,
        atime: Option<DateTime<Utc>>,
        overwrite: bool,
        force_cross_fs: bool,
        pre_verify_hook: F,
    ) -> Result<Option<String>>
    where
        F: FnOnce(&Path) -> Result<()>,
    {
        if src == dest {
            return Ok(None);
        }

        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| FileCleanerError::io(parent, err))?;
            }
        }

        if !overwrite && fs::symlink_metadata(dest).is_ok() {
            return Err(FileCleanerError::io(
                dest,
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destination already exists and overwrite is disabled",
                ),
            ));
        }

        if !force_cross_fs {
            match fs::rename(src, dest) {
                Ok(()) => return Ok(None),
                Err(err) if is_cross_device_error(&err) => {}
                Err(err) => return Err(FileCleanerError::io(dest, err)),
            }
        }

        let (_, src_meta) = open_regular_nofollow_read(src)?;
        let staged_tmp = stage_temp_path(dest);
        let mut guard = TempFileGuard::new(staged_tmp.clone());

        let (expected_sha, expected_len) = copy_stream_with_sha256(src, &staged_tmp)?;
        pre_verify_hook(&staged_tmp)?;
        verify_file_sha256(&staged_tmp, &expected_sha, expected_len)?;

        commit_staged_file(&staged_tmp, dest, mtime, atime, &src_meta, &mut guard)?;

        fs::remove_file(src).map_err(|err| FileCleanerError::io(src, err))?;

        Ok(Some(expected_sha))
    }

    /// Copies `src` into `staged_tmp` with `sync_all()` and verifies integrity by re-reading
    /// `staged_tmp` from disk and comparing length and SHA-256 digests.
    ///
    /// On any failure or mismatch, removes `staged_tmp` and returns an error.
    pub fn copy_and_verify_sha256(src: &Path, staged_tmp: &Path) -> Result<(String, u64)> {
        let mut guard = TempFileGuard::new(staged_tmp.to_path_buf());
        let (expected_sha, expected_len) = copy_stream_with_sha256(src, staged_tmp)?;
        verify_file_sha256(staged_tmp, &expected_sha, expected_len)?;
        guard.disarm();
        Ok((expected_sha, expected_len))
    }

    /// Executes `CopyTruncate`: stages archive copy (optionally compressed), commits atomically,
    /// then opens the original file in write mode and truncates to `0` bytes (`set_len(0)` + `sync_all()`).
    pub fn execute_copy_truncate(
        target: &WorkingTarget,
        archive_dest: &Path,
        compress: Option<CompressionFormat>,
        compression_level: Option<u32>,
    ) -> Result<Option<String>> {
        let sym_before = fs::symlink_metadata(&target.path)
            .map_err(|err| FileCleanerError::io(&target.path, err))?;
        if sym_before.file_type().is_symlink() || !sym_before.is_file() {
            return Err(FileCleanerError::io(
                &target.path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing copy_truncate on symlink or non-regular file",
                ),
            ));
        }

        if let Some(parent) = archive_dest.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| FileCleanerError::io(parent, err))?;
            }
        }

        let sha = if let Some(fmt) = compress {
            let _ = compress_file_staged(target, archive_dest, fmt, compression_level)?;
            None
        } else {
            let staged_tmp = stage_temp_path(archive_dest);
            let mut guard = TempFileGuard::new(staged_tmp.clone());
            let (sha, _) = Self::copy_and_verify_sha256(&target.path, &staged_tmp)?;
            commit_staged_file(
                &staged_tmp,
                archive_dest,
                target.mtime,
                target.atime,
                &sym_before,
                &mut guard,
            )?;
            Some(sha)
        };

        let (src_file, write_meta) = open_regular_nofollow_write(&target.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (write_meta.dev(), write_meta.ino()) != (sym_before.dev(), sym_before.ino()) {
                return Err(FileCleanerError::io(
                    &target.path,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source file identity changed before copy_truncate",
                    ),
                ));
            }
        }
        #[cfg(not(unix))]
        let _ = write_meta;

        src_file
            .set_len(0)
            .map_err(|err| FileCleanerError::io(&target.path, err))?;
        src_file
            .sync_all()
            .map_err(|err| FileCleanerError::io(&target.path, err))?;

        Ok(sha)
    }
}

/// Computes lowercase hex SHA-256 digest and byte length of an existing file on disk.
pub fn compute_file_sha256(path: &Path) -> Result<(String, u64)> {
    let file = File::open(path).map_err(|err| FileCleanerError::io(path, err))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; COPY_BUF_SIZE];
    let mut total_bytes: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| FileCleanerError::io(path, err))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total_bytes = total_bytes.saturating_add(n as u64);
    }

    Ok((to_hex_lower(&hasher.finalize()), total_bytes))
}

/// Verifies `path` on disk matches `expected_hex` and `expected_len`, returning
/// [`FileCleanerError::IntegrityMismatch`] on divergence.
pub fn verify_file_sha256(path: &Path, expected_hex: &str, expected_len: u64) -> Result<()> {
    let (actual_hex, actual_len) = compute_file_sha256(path)?;
    if actual_hex != expected_hex || actual_len != expected_len {
        return Err(FileCleanerError::IntegrityMismatch {
            path: path.to_path_buf(),
            expected: format!("{expected_hex} ({expected_len} bytes)"),
            actual: format!("{actual_hex} ({actual_len} bytes)"),
        });
    }
    Ok(())
}

/// Rejects symlinks when `follow_symlinks` is disabled before mutating or unlinking.
fn reject_symlink_if_disallowed(path: &Path, follow_symlinks: bool) -> Result<()> {
    if follow_symlinks {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path).map_err(|err| FileCleanerError::io(path, err))?;
    if meta.file_type().is_symlink() {
        return Err(FileCleanerError::io(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to operate on symlink when follow_symlinks is false",
            ),
        ));
    }
    Ok(())
}

/// Creates a new staging file with private `0o600` permissions on Unix.
fn open_staged_tmp_private(staged_tmp: &Path) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(staged_tmp)
        .map_err(|err| FileCleanerError::io(staged_tmp, err))
}

/// Opens a regular file for reading without following symlinks or reparse points.
fn open_regular_nofollow_read(path: &Path) -> Result<(File, fs::Metadata)> {
    open_regular_nofollow(path, false)
}

/// Opens a regular file for writing without following symlinks or reparse points.
fn open_regular_nofollow_write(path: &Path) -> Result<(File, fs::Metadata)> {
    open_regular_nofollow(path, true)
}

/// Opens a regular file without following symlinks and verifies handle identity matches `symlink_metadata`.
fn open_regular_nofollow(path: &Path, write: bool) -> Result<(File, fs::Metadata)> {
    let sym = fs::symlink_metadata(path).map_err(|err| FileCleanerError::io(path, err))?;
    if sym.file_type().is_symlink() || !sym.is_file() {
        return Err(FileCleanerError::io(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to open symlink or non-regular file",
            ),
        ));
    }

    let mut opts = OpenOptions::new();
    if write {
        opts.write(true);
    } else {
        opts.read(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    let file = opts
        .open(path)
        .map_err(|err| FileCleanerError::io(path, err))?;
    let meta = file
        .metadata()
        .map_err(|err| FileCleanerError::io(path, err))?;
    if !meta.is_file() {
        return Err(FileCleanerError::io(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "opened path is not a regular file",
            ),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (meta.dev(), meta.ino()) != (sym.dev(), sym.ino()) {
            return Err(FileCleanerError::io(
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file identity changed while opening",
                ),
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(FileCleanerError::io(
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing to open reparse point",
                ),
            ));
        }
    }

    Ok((file, meta))
}

fn copy_stream_with_sha256(src: &Path, staged_tmp: &Path) -> Result<(String, u64)> {
    let (src_file, _) = open_regular_nofollow_read(src)?;
    let mut reader = BufReader::new(src_file);

    let tmp_file = open_staged_tmp_private(staged_tmp)?;
    let mut writer = BufWriter::new(tmp_file);

    let mut hasher = Sha256::new();
    let mut buf = [0u8; COPY_BUF_SIZE];
    let mut total_bytes: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| FileCleanerError::io(src, err))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer
            .write_all(&buf[..n])
            .map_err(|err| FileCleanerError::io(staged_tmp, err))?;
        total_bytes = total_bytes.saturating_add(n as u64);
    }

    writer
        .flush()
        .map_err(|err| FileCleanerError::io(staged_tmp, err))?;
    let tmp_file = writer
        .into_inner()
        .map_err(|err| FileCleanerError::io(staged_tmp, err.into_error()))?;
    tmp_file
        .sync_all()
        .map_err(|err| FileCleanerError::io(staged_tmp, err))?;

    Ok((to_hex_lower(&hasher.finalize()), total_bytes))
}

fn compress_file_staged(
    target: &WorkingTarget,
    dest: &Path,
    format: CompressionFormat,
    level: Option<u32>,
) -> Result<u64> {
    let (src_file, src_meta) = open_regular_nofollow_read(&target.path)?;
    let mut reader = BufReader::new(src_file);

    let staged_tmp = stage_temp_path(dest);
    let mut guard = TempFileGuard::new(staged_tmp.clone());

    let mut tmp_file = open_staged_tmp_private(&staged_tmp)?;

    let entry_name = extract_file_name(&target.path)?
        .to_string_lossy()
        .into_owned();

    match format {
        CompressionFormat::Gzip => {
            let gzip_level = level
                .map(|l| Compression::new(l.min(9)))
                .unwrap_or_default();
            let mtime_secs = u32::try_from(target.mtime.timestamp().max(0)).unwrap_or(u32::MAX);
            let mut encoder = GzBuilder::new()
                .filename(entry_name.as_bytes())
                .mtime(mtime_secs)
                .write(tmp_file, gzip_level);
            io::copy(&mut reader, &mut encoder).map_err(|e| compression_err(dest, e))?;
            tmp_file = encoder.finish().map_err(|e| compression_err(dest, e))?;
        }
        CompressionFormat::Lz4 => {
            let mut encoder = lz4_flex::frame::FrameEncoder::new(tmp_file);
            io::copy(&mut reader, &mut encoder).map_err(|e| compression_err(dest, e))?;
            tmp_file = encoder.finish().map_err(|e| compression_err(dest, e))?;
        }
        CompressionFormat::Brotli => {
            let quality = level.unwrap_or(DEFAULT_BROTLI_QUALITY).min(11);
            let mut encoder = brotli::CompressorWriter::new(
                &mut tmp_file,
                BROTLI_BUF_SIZE,
                quality,
                BROTLI_WINDOW_BITS,
            );
            io::copy(&mut reader, &mut encoder).map_err(|e| compression_err(dest, e))?;
            encoder.flush().map_err(|e| compression_err(dest, e))?;
            // Drop encoder explicitly before syncing so final Brotli EOF blocks are written.
        }
        CompressionFormat::Zip => {
            let mut zip_writer = ZipWriter::new(tmp_file);
            let mut options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(level.map(|l| i64::from(l.min(9))));
            if let Some(zip_dt) = to_zip_datetime(target.mtime) {
                options = options.last_modified_time(zip_dt);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                options = options.unix_permissions(src_meta.permissions().mode());
            }

            zip_writer
                .start_file(&entry_name, options)
                .map_err(|e| compression_err(dest, e))?;
            io::copy(&mut reader, &mut zip_writer).map_err(|e| compression_err(dest, e))?;
            tmp_file = zip_writer.finish().map_err(|e| compression_err(dest, e))?;
        }
    }

    tmp_file
        .flush()
        .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
    tmp_file
        .sync_all()
        .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
    drop(tmp_file);

    commit_staged_file(
        &staged_tmp,
        dest,
        target.mtime,
        target.atime,
        &src_meta,
        &mut guard,
    )?;

    let final_meta = fs::symlink_metadata(dest).map_err(|err| FileCleanerError::io(dest, err))?;
    Ok(final_meta.len())
}

fn compression_err<E: std::fmt::Display>(dest: &Path, err: E) -> FileCleanerError {
    FileCleanerError::Compression {
        path: dest.to_path_buf(),
        reason: err.to_string(),
    }
}

fn commit_staged_file(
    staged_tmp: &Path,
    dest: &Path,
    mtime: DateTime<Utc>,
    atime: Option<DateTime<Utc>>,
    src_meta: &fs::Metadata,
    guard: &mut TempFileGuard,
) -> Result<()> {
    restore_file_times(staged_tmp, mtime, atime, Some(src_meta))?;

    #[cfg(unix)]
    fs::set_permissions(staged_tmp, src_meta.permissions())
        .map_err(|err| FileCleanerError::io(staged_tmp, err))?;

    fs::rename(staged_tmp, dest).map_err(|err| FileCleanerError::io(dest, err))?;
    guard.disarm();

    #[cfg(not(unix))]
    fs::set_permissions(dest, src_meta.permissions())
        .map_err(|err| FileCleanerError::io(dest, err))?;

    Ok(())
}

/// Returns `true` when `path` already has a supported compressed archive extension.
pub(crate) fn has_compressed_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    ext.eq_ignore_ascii_case("gz")
        || ext.eq_ignore_ascii_case("lz4")
        || ext.eq_ignore_ascii_case("br")
        || ext.eq_ignore_ascii_case("zip")
}

/// Returns `true` when the filename stem already carries a trailing `_{stamp}` matching `template`.
pub(crate) fn has_trailing_date_stamp(path: &Path, template: &str, mtime: DateTime<Utc>) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let formatted = mtime.format(template).to_string();
    for (pos, _) in stem.match_indices('_') {
        let cand = &stem[pos + 1..];
        if !cand.is_empty()
            && (cand == formatted
                || chrono::NaiveDateTime::parse_from_str(cand, template).is_ok()
                || chrono::NaiveDate::parse_from_str(cand, template).is_ok())
        {
            return true;
        }
    }
    false
}

fn compute_rename_destination(
    src: &Path,
    mtime: DateTime<Utc>,
    rule: &RuleConfig,
) -> Result<PathBuf> {
    let template = rule
        .rename_template
        .as_deref()
        .unwrap_or(DEFAULT_RENAME_TEMPLATE);
    validate_strftime(template, &rule.name, "rename_template")?;

    if has_trailing_date_stamp(src, template, mtime) {
        return Ok(src.to_path_buf());
    }

    let formatted = mtime.format(template).to_string();
    let new_name = format_stamped_filename(src, &formatted, None)?;
    Ok(match src.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(new_name),
        _ => PathBuf::from(new_name),
    })
}

fn compute_copy_truncate_archive_path(
    src: &Path,
    dest_dir: &Path,
    mtime: DateTime<Utc>,
    rename_template: Option<&str>,
    compress: Option<CompressionFormat>,
) -> Result<PathBuf> {
    let template = rename_template.unwrap_or(DEFAULT_RENAME_TEMPLATE);
    validate_strftime(template, "copy_truncate", "rename_template")?;
    let formatted = mtime.format(template).to_string();
    let suffix = compress.map(compression_suffix);

    let first_name = format_stamped_filename(src, &formatted, suffix)?;
    let first_path = dest_dir.join(first_name);
    if fs::symlink_metadata(&first_path).is_err() {
        return Ok(first_path);
    }

    let mut counter: u64 = 1;
    loop {
        let stamped = format!("{formatted}_{counter}");
        let name = format_stamped_filename(src, &stamped, suffix)?;
        let candidate = dest_dir.join(name);
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
        counter = counter.saturating_add(1);
    }
}

fn format_stamped_filename(src: &Path, stamp: &str, extra_suffix: Option<&str>) -> Result<String> {
    let file_name = extract_file_name(src)?;
    let name_path = Path::new(file_name);
    let stem = name_path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| file_name.to_string_lossy());
    let ext = name_path.extension().map(|e| e.to_string_lossy());

    let mut out = match ext {
        Some(ext) if !ext.is_empty() => format!("{stem}_{stamp}.{ext}"),
        _ => format!("{stem}_{stamp}"),
    };
    if let Some(suffix) = extra_suffix {
        out.push_str(suffix);
    }
    Ok(out)
}

fn compute_compressed_path(src: &Path, format: CompressionFormat) -> PathBuf {
    let mut os = src.as_os_str().to_os_string();
    os.push(compression_suffix(format));
    PathBuf::from(os)
}

fn compression_suffix(format: CompressionFormat) -> &'static str {
    match format {
        CompressionFormat::Gzip => ".gz",
        CompressionFormat::Lz4 => ".lz4",
        CompressionFormat::Brotli => ".br",
        CompressionFormat::Zip => ".zip",
    }
}

fn resolve_move_dest_dir(
    base_dir: &Path,
    date_subdir: Option<&str>,
    mtime: DateTime<Utc>,
    rule_name: &str,
) -> Result<PathBuf> {
    let Some(fmt) = date_subdir else {
        return Ok(base_dir.to_path_buf());
    };
    validate_strftime(fmt, rule_name, "move_date_subdir")?;
    let rendered = mtime.format(fmt).to_string();
    let mut resolved = base_dir.to_path_buf();
    for segment in rendered.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(FileCleanerError::ConfigValidation(format!(
                "rule '{rule_name}' move_date_subdir resolved unsafe '..' component"
            )));
        }
        let seg_path = Path::new(segment);
        if seg_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(FileCleanerError::ConfigValidation(format!(
                "rule '{rule_name}' move_date_subdir resolved invalid path segment '{segment}'"
            )));
        }
        resolved.push(segment);
    }
    Ok(resolved)
}

fn unique_collision_free_path(dir: &Path, file_name: &OsStr) -> PathBuf {
    let candidate = dir.join(file_name);
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }

    let name_str = file_name.to_string_lossy();
    let (stem, ext) = split_compound_stem_ext(&name_str);

    let mut counter: u64 = 1;
    loop {
        let disambiguated = if ext.is_empty() {
            format!("{stem}_{counter}")
        } else {
            format!("{stem}_{counter}.{ext}")
        };
        let next = dir.join(disambiguated);
        if fs::symlink_metadata(&next).is_err() {
            return next;
        }
        counter = counter.saturating_add(1);
    }
}

fn split_compound_stem_ext(file_name: &str) -> (&str, &str) {
    let p = Path::new(file_name);
    match (
        p.file_stem().and_then(|s| s.to_str()),
        p.extension().and_then(|e| e.to_str()),
    ) {
        (Some(stem), Some(ext)) if !ext.is_empty() => (stem, ext),
        _ => (file_name, ""),
    }
}

fn validate_strftime(fmt: &str, rule_name: &str, field: &str) -> Result<()> {
    if fmt.trim().is_empty() || StrftimeItems::new(fmt).any(|item| matches!(item, Item::Error)) {
        return Err(FileCleanerError::ConfigValidation(format!(
            "rule '{rule_name}' has invalid {field} '{fmt}'"
        )));
    }
    Ok(())
}

fn extract_file_name(path: &Path) -> Result<&OsStr> {
    path.file_name().ok_or_else(|| {
        FileCleanerError::io(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "candidate path has no file name",
            ),
        )
    })
}

fn stage_temp_path(dest: &Path) -> PathBuf {
    let seq = TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut os = dest.as_os_str().to_os_string();
    os.push(format!(".fc-tmp.{pid}.{seq}"));
    PathBuf::from(os)
}

fn restore_file_times(
    path: &Path,
    mtime: DateTime<Utc>,
    atime: Option<DateTime<Utc>>,
    fallback_meta: Option<&fs::Metadata>,
) -> Result<()> {
    let sys_mtime = datetime_to_system_time(mtime);
    let mut times = FileTimes::new().set_modified(sys_mtime);
    if let Some(sys_atime) = atime
        .map(datetime_to_system_time)
        .or_else(|| fallback_meta.and_then(|meta| meta.accessed().ok()))
    {
        times = times.set_accessed(sys_atime);
    }

    let file = open_for_set_times(path).map_err(|err| FileCleanerError::io(path, err))?;
    file.set_times(times)
        .map_err(|err| FileCleanerError::io(path, err))
}

fn open_for_set_times(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}

fn datetime_to_system_time(dt: DateTime<Utc>) -> SystemTime {
    let secs = dt.timestamp();
    let nanos = dt.timestamp_subsec_nanos();
    if secs >= 0 {
        UNIX_EPOCH + Duration::new(secs as u64, nanos)
    } else if nanos == 0 {
        UNIX_EPOCH - Duration::new(secs.unsigned_abs(), 0)
    } else {
        let whole_back = secs.unsigned_abs();
        let adj_secs = whole_back.saturating_sub(1);
        let adj_nanos = 1_000_000_000 - nanos;
        UNIX_EPOCH - Duration::new(adj_secs, adj_nanos)
    }
}

fn to_zip_datetime(dt: DateTime<Utc>) -> Option<zip::DateTime> {
    let year = u16::try_from(dt.year()).ok()?;
    let month = u8::try_from(dt.month()).ok()?;
    let day = u8::try_from(dt.day()).ok()?;
    let hour = u8::try_from(dt.hour()).ok()?;
    let minute = u8::try_from(dt.minute()).ok()?;
    let second = u8::try_from(dt.second().min(59)).ok()?;
    zip::DateTime::from_date_and_time(year, month, day, hour, minute, second).ok()
}

fn is_cross_device_error(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        err.kind() == io::ErrorKind::CrossesDevices || err.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(not(unix))]
    {
        err.kind() == io::ErrorKind::CrossesDevices
    }
}

fn to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackupGuardConfig, CombineMode, EvictionOrder, InUseConfig, TimeBasis};
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn base_rule(path: PathBuf) -> RuleConfig {
        RuleConfig {
            name: "unit-rule".to_string(),
            path,
            enabled: true,
            recursive: false,
            max_depth: None,
            follow_symlinks: false,
            include_globs: vec![],
            exclude_globs: vec![],
            include_regex: None,
            exclude_regex: None,
            min_file_size: None,
            max_file_size: None,
            time_basis: TimeBasis::Modified,
            filename_date_regex: None,
            filename_date_format: None,
            max_files: None,
            max_age: None,
            max_total_size: None,
            combine_mode: CombineMode::Any,
            eviction_order: EvictionOrder::OldestFirst,
            dry_run: false,
            backup_guard: BackupGuardConfig::default(),
            in_use: InUseConfig::default(),
            pre_delete_hook: None,
            actions: vec![ActionStep::Delete],
            move_to: None,
            move_date_subdir: None,
            move_overwrite: false,
            trash_dir: None,
            compression: CompressionFormat::Gzip,
            compression_level: None,
            compress_remove_source: true,
            compress_overwrite: false,
            copy_truncate_to: None,
            rename_template: None,
        }
    }

    #[test]
    fn reject_invalid_rule_pipelines_and_unsafe_move_subdir() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sample.log");
        fs::write(&src, b"hello").unwrap();
        let target = WorkingTarget {
            path: src,
            size_bytes: 5,
            mtime: Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap(),
            atime: None,
        };

        let mut empty_rule = base_rule(tmp.path().to_path_buf());
        empty_rule.actions = vec![];
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &empty_rule, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut non_terminal = base_rule(tmp.path().to_path_buf());
        non_terminal.actions = vec![ActionStep::Delete, ActionStep::Move];
        non_terminal.move_to = Some(tmp.path().join("dest"));
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &non_terminal, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut bad_subdir = base_rule(tmp.path().to_path_buf());
        bad_subdir.actions = vec![ActionStep::Move];
        bad_subdir.move_to = Some(tmp.path().join("dest"));
        bad_subdir.move_date_subdir = Some("../escape".to_string());
        assert!(matches!(
            ActionExecutor::execute_pipeline(target, &bad_subdir, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));
    }

    #[test]
    fn unique_collision_free_path_handles_extensionless_and_existing() {
        let tmp = TempDir::new().unwrap();
        let p0 = tmp.path().join("README");
        fs::write(&p0, b"0").unwrap();
        let p1 = tmp.path().join("README_1");
        fs::write(&p1, b"1").unwrap();

        let next = unique_collision_free_path(tmp.path(), OsStr::new("README"));
        assert_eq!(next, tmp.path().join("README_2"));
    }

    #[test]
    fn zip_datetime_rejects_pre_1980_and_accepts_modern_utc() {
        assert!(to_zip_datetime(Utc.with_ymd_and_hms(1975, 1, 1, 0, 0, 0).unwrap()).is_none());
        assert!(to_zip_datetime(Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap()).is_some());
    }

    #[test]
    fn verify_file_sha256_detects_mismatch_and_missing_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("data.bin");
        fs::write(&f, b"abcdef").unwrap();
        let (hex, len) = compute_file_sha256(&f).unwrap();
        assert_eq!(len, 6);
        assert!(verify_file_sha256(&f, &hex, len).is_ok());

        let err = verify_file_sha256(&f, "0000", len).unwrap_err();
        assert!(matches!(err, FileCleanerError::IntegrityMismatch { .. }));

        let missing = tmp.path().join("missing.bin");
        assert!(matches!(
            compute_file_sha256(&missing),
            Err(FileCleanerError::Io { .. })
        ));
    }

    #[test]
    fn missing_destinations_invalid_templates_and_helper_edge_cases() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("no_ext");
        fs::write(&src, b"raw-bytes").unwrap();
        let mtime = Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap();
        let target = WorkingTarget {
            path: src.clone(),
            size_bytes: 9,
            mtime,
            atime: None,
        };

        // Missing move_to / copy_truncate_to / trash_dir return ConfigValidation.
        let mut no_move = base_rule(tmp.path().to_path_buf());
        no_move.actions = vec![ActionStep::Move];
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &no_move, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut no_ct = base_rule(tmp.path().to_path_buf());
        no_ct.actions = vec![ActionStep::CopyTruncate];
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &no_ct, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut no_trash = base_rule(tmp.path().to_path_buf());
        no_trash.actions = vec![ActionStep::Trash];
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &no_trash, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        // Invalid rename_template rejected cleanly.
        let mut bad_tpl = base_rule(tmp.path().to_path_buf());
        bad_tpl.actions = vec![ActionStep::RenameByDate];
        bad_tpl.rename_template = Some("%Q".to_string());
        assert!(matches!(
            ActionExecutor::execute_pipeline(target.clone(), &bad_tpl, None, false),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        // Extensionless RenameByDate + relative compute_rename_destination branch.
        let rename_ok = RuleConfig {
            actions: vec![ActionStep::RenameByDate],
            ..base_rule(tmp.path().to_path_buf())
        };
        let renamed = ActionExecutor::execute_pipeline(target, &rename_ok, None, false).unwrap();
        assert_eq!(
            renamed.final_path,
            Some(tmp.path().join("no_ext_20261120_090500"))
        );
        let rel_dest = compute_rename_destination(Path::new("rel.log"), mtime, &rename_ok).unwrap();
        assert_eq!(rel_dest, PathBuf::from("rel_20261120_090500.log"));

        // resolve_move_dest_dir skips empty and '.' segments.
        let resolved = resolve_move_dest_dir(tmp.path(), Some("./%Y//%m"), mtime, "r").unwrap();
        assert_eq!(resolved, tmp.path().join("2026").join("11"));

        // Same-path move is a clean no-op.
        let same_file = tmp.path().join("same.txt");
        fs::write(&same_file, b"keep").unwrap();
        assert_eq!(
            ActionExecutor::move_with_cross_fs_fallback(
                &same_file, &same_file, mtime, None, false, false
            )
            .unwrap(),
            None
        );

        // CopyTruncate falling back to move_to + Gzip compression_level + same-second collision.
        let ct_fallback_dir = tmp.path().join("ct_fallback");
        let ct_src = tmp.path().join("stream.log");
        fs::write(&ct_src, b"stream-data-payload").unwrap();
        let mut ct_rule = base_rule(tmp.path().to_path_buf());
        ct_rule.actions = vec![ActionStep::CopyTruncate];
        ct_rule.move_to = Some(ct_fallback_dir.clone());
        ct_rule.compression = CompressionFormat::Gzip;
        ct_rule.compression_level = Some(6);
        let ct_target = WorkingTarget {
            path: ct_src.clone(),
            size_bytes: 19,
            mtime,
            atime: None,
        };
        let ct1 =
            ActionExecutor::execute_pipeline(ct_target.clone(), &ct_rule, None, false).unwrap();
        assert_eq!(
            ct1.final_path,
            Some(ct_fallback_dir.join("stream_20261120_090500.log.gz"))
        );
        fs::write(&ct_src, b"second-batch").unwrap();
        let ct2 = ActionExecutor::execute_pipeline(ct_target, &ct_rule, None, false).unwrap();
        assert_eq!(
            ct2.final_path,
            Some(ct_fallback_dir.join("stream_20261120_090500_1.log.gz"))
        );

        // Negative timestamp SystemTime conversion, restore_file_times without atime, and cross-device detection.
        let pre_epoch_exact = Utc.with_ymd_and_hms(1969, 12, 31, 23, 59, 50).unwrap();
        let pre_epoch_nanos = pre_epoch_exact + chrono::Duration::nanoseconds(250_000_000);
        assert!(datetime_to_system_time(pre_epoch_exact) < UNIX_EPOCH);
        assert!(datetime_to_system_time(pre_epoch_nanos) < UNIX_EPOCH);
        assert!(restore_file_times(&same_file, mtime, None, None).is_ok());

        let cross_err = io::Error::from(io::ErrorKind::CrossesDevices);
        assert!(is_cross_device_error(&cross_err));
        #[cfg(unix)]
        {
            let exdev = io::Error::from_raw_os_error(libc::EXDEV);
            assert!(is_cross_device_error(&exdev));
        }
        assert!(!is_cross_device_error(&io::Error::from(
            io::ErrorKind::NotFound
        )));
        assert!(extract_file_name(Path::new("/")).is_err());

        // Idempotency helper coverage: compressed extensions & trailing date stamps.
        assert!(has_compressed_extension(Path::new("dump.sql.gz")));
        assert!(has_compressed_extension(Path::new("dump.sql.LZ4")));
        assert!(has_compressed_extension(Path::new("dump.sql.br")));
        assert!(has_compressed_extension(Path::new("dump.sql.ZIP")));
        assert!(!has_compressed_extension(Path::new("dump.sql")));
        assert!(!has_compressed_extension(Path::new("no_extension")));

        assert!(has_trailing_date_stamp(
            Path::new("service_20261120_090500.log"),
            DEFAULT_RENAME_TEMPLATE,
            mtime
        ));
        assert!(has_trailing_date_stamp(
            Path::new("daily_2026-11-20.sql"),
            "%Y-%m-%d",
            mtime
        ));
        assert!(!has_trailing_date_stamp(
            Path::new("my_app_service.log"),
            DEFAULT_RENAME_TEMPLATE,
            mtime
        ));
        assert!(!has_trailing_date_stamp(
            Path::new("plain.log"),
            DEFAULT_RENAME_TEMPLATE,
            mtime
        ));

        // Directory (non-regular) rejected by nofollow read/write helpers.
        assert!(matches!(
            open_regular_nofollow_read(tmp.path()),
            Err(FileCleanerError::Io { .. })
        ));
        assert!(matches!(
            open_regular_nofollow_write(tmp.path()),
            Err(FileCleanerError::Io { .. })
        ));
        assert!(reject_symlink_if_disallowed(&same_file, true).is_ok());
    }
}
