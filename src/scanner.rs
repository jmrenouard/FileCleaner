//! Directory traversal, filter algebra, timestamp resolution, and deterministic sorting.

use crate::config::{DaemonConfig, RuleConfig, TimeBasis};
use crate::error::{FileCleanerError, Result};
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

/// Injectable wall-clock abstraction enabling deterministic timestamp tests.
pub trait Clock: Send + Sync {
    /// Returns the current UTC timestamp.
    fn now(&self) -> DateTime<Utc>;
}

/// Production wall-clock implementation backed by [`Utc::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Deterministic fixed clock used in unit and integration tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// Metadata and resolved timestamp for a single candidate file discovered by [`Scanner`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedFile {
    /// Full filesystem path to the candidate file.
    pub path: PathBuf,
    /// Relative path from the rule root normalized with `/` separators.
    pub rel_path: String,
    /// Candidate filename (basename).
    pub name: String,
    /// File length in bytes.
    pub size_bytes: u64,
    /// Resolved UTC timestamp according to the rule's [`TimeBasis`].
    pub resolved_ts: DateTime<Utc>,
    /// Filesystem modification timestamp (`mtime`) in UTC.
    pub modified_ts: DateTime<Utc>,
    /// Filesystem birth/creation timestamp (`birthtime`) in UTC when supported.
    pub created_ts: Option<DateTime<Utc>>,
}

/// Output produced by scanning a rule's target directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Discovered eligible files sorted newest-first (`resolved_ts` desc, `rel_path` asc).
    pub files: Vec<ScannedFile>,
    /// Files skipped fail-closed because `TimeBasis::FilenameDate` failed to match or parse.
    pub skipped_timestamp_parse: Vec<PathBuf>,
    /// Non-fatal diagnostic warnings emitted during traversal or timestamp parsing.
    pub warnings: Vec<String>,
}

/// Deterministic directory scanner implementing the Phase 1 specification.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scanner;

struct CompiledFilters {
    include_globs: Vec<GlobMatcher>,
    exclude_globs: Vec<GlobMatcher>,
    include_regex: Option<Regex>,
    exclude_regex: Option<Regex>,
}

struct FilenameDateParser<'a> {
    regex: Regex,
    format: &'a str,
}

struct InternalArtifactGuard {
    exact_paths: HashSet<PathBuf>,
    normalized_paths: HashSet<PathBuf>,
    file_names: HashSet<OsString>,
}

impl Scanner {
    /// Scans `rule.path` according to depth, symlink, filter, size, and `TimeBasis` rules.
    ///
    /// Internal temporary artifacts (`*.fc-tmp`, `*.fc-tmp.*`) and configured operational
    /// paths (`audit_log`, `pid_file`, `state_file`, `stop_file`) are unconditionally excluded.
    pub fn scan_rule(
        rule: &RuleConfig,
        audit_log: Option<&Path>,
        daemon: &DaemonConfig,
    ) -> Result<ScanReport> {
        let root_meta =
            fs::metadata(&rule.path).map_err(|err| FileCleanerError::io(&rule.path, err))?;
        if !root_meta.is_dir() {
            return Err(FileCleanerError::io(
                &rule.path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "rule '{}' path '{}' is not a directory",
                        rule.name,
                        rule.path.display()
                    ),
                ),
            ));
        }

        let filters = compile_filters(rule)?;
        let fn_date_parser = compile_filename_date_parser(rule)?;
        let internal_guard = build_internal_guard(audit_log, daemon);
        let norm_root = resolve_path_lexically(&rule.path);

        let mut walker = WalkDir::new(&rule.path).follow_links(rule.follow_symlinks);
        if !rule.recursive {
            walker = walker.max_depth(1);
        } else if let Some(max_depth) = rule.max_depth {
            walker = walker.max_depth(max_depth);
        }

        let mut report = ScanReport::default();

        let iter = walker.into_iter().filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if !rule.follow_symlinks && entry.file_type().is_symlink() {
                return false;
            }
            if entry.file_type().is_dir() {
                let rel_str = normalized_rel_path(&rule.path, entry.path());
                let name = entry.file_name().to_string_lossy();
                if is_dir_excluded(
                    &rel_str,
                    &name,
                    &filters.exclude_globs,
                    filters.exclude_regex.as_ref(),
                ) {
                    return false;
                }
            }
            true
        });

        for entry_res in iter {
            let entry = match entry_res {
                Ok(entry) => entry,
                Err(err) => {
                    return Err(map_walkdir_error(err, &rule.path));
                }
            };

            if entry.depth() == 0 {
                continue;
            }

            if !rule.follow_symlinks && entry.file_type().is_symlink() {
                continue;
            }

            let entry_path = entry.path();
            let metadata = entry
                .metadata()
                .map_err(|err| map_walkdir_error(err, entry_path))?;

            if !metadata.is_file() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            if is_temp_artifact_name(&name)
                || internal_guard.matches(entry_path, &rule.path, &norm_root)
            {
                continue;
            }

            let rel_path = normalized_rel_path(&rule.path, entry_path);
            if !matches_filter_algebra(&rel_path, &name, &filters) {
                continue;
            }

            let size_bytes = metadata.len();
            if let Some(min_bytes) = rule.min_file_size {
                if size_bytes < min_bytes {
                    continue;
                }
            }
            if let Some(max_bytes) = rule.max_file_size {
                if size_bytes > max_bytes {
                    continue;
                }
            }

            let modified_sys = metadata
                .modified()
                .map_err(|err| FileCleanerError::io(entry_path, err))?;
            let modified_ts = system_time_to_utc(modified_sys, entry_path)?;

            let (resolved_ts, created_ts) = match rule.time_basis {
                TimeBasis::Modified => {
                    let created_ts = metadata
                        .created()
                        .ok()
                        .and_then(|st| system_time_to_utc(st, entry_path).ok());
                    (modified_ts, created_ts)
                }
                TimeBasis::Created => {
                    let created_sys = metadata
                        .created()
                        .map_err(|err| FileCleanerError::io(entry_path, err))?;
                    let created = system_time_to_utc(created_sys, entry_path)?;
                    (created, Some(created))
                }
                TimeBasis::FilenameDate => {
                    let parser = fn_date_parser.as_ref().ok_or_else(|| {
                        FileCleanerError::ConfigValidation(format!(
                            "rule '{}' missing compiled filename_date parser",
                            rule.name
                        ))
                    })?;
                    match parse_filename_timestamp_with(parser, &name) {
                        Some(parsed_ts) => {
                            let created_ts = metadata
                                .created()
                                .ok()
                                .and_then(|st| system_time_to_utc(st, entry_path).ok());
                            (parsed_ts, created_ts)
                        }
                        None => {
                            report
                                .skipped_timestamp_parse
                                .push(entry_path.to_path_buf());
                            report.warnings.push(format!(
                                "rule '{}': skipped '{}' because filename_date failed to match or parse",
                                rule.name,
                                entry_path.display()
                            ));
                            continue;
                        }
                    }
                }
            };

            report.files.push(ScannedFile {
                path: entry_path.to_path_buf(),
                rel_path,
                name,
                size_bytes,
                resolved_ts,
                modified_ts,
                created_ts,
            });
        }

        // Order deterministically newest-first, tie-broken ascending by normalized relative path.
        report.files.sort_by(|a, b| {
            b.resolved_ts
                .cmp(&a.resolved_ts)
                .then_with(|| a.rel_path.cmp(&b.rel_path))
        });
        report.skipped_timestamp_parse.sort();

        Ok(report)
    }
}

fn compile_filters(rule: &RuleConfig) -> Result<CompiledFilters> {
    let mut include_globs = Vec::with_capacity(rule.include_globs.len());
    for pattern in &rule.include_globs {
        let glob = Glob::new(pattern).map_err(|source| FileCleanerError::InvalidGlob {
            pattern: pattern.clone(),
            source,
        })?;
        include_globs.push(glob.compile_matcher());
    }

    let mut exclude_globs = Vec::with_capacity(rule.exclude_globs.len());
    for pattern in &rule.exclude_globs {
        let glob = Glob::new(pattern).map_err(|source| FileCleanerError::InvalidGlob {
            pattern: pattern.clone(),
            source,
        })?;
        exclude_globs.push(glob.compile_matcher());
    }

    let include_regex = match &rule.include_regex {
        Some(pattern) => {
            Some(
                Regex::new(pattern).map_err(|source| FileCleanerError::InvalidRegex {
                    pattern: pattern.clone(),
                    source,
                })?,
            )
        }
        None => None,
    };

    let exclude_regex = match &rule.exclude_regex {
        Some(pattern) => {
            Some(
                Regex::new(pattern).map_err(|source| FileCleanerError::InvalidRegex {
                    pattern: pattern.clone(),
                    source,
                })?,
            )
        }
        None => None,
    };

    Ok(CompiledFilters {
        include_globs,
        exclude_globs,
        include_regex,
        exclude_regex,
    })
}

fn compile_filename_date_parser(rule: &RuleConfig) -> Result<Option<FilenameDateParser<'_>>> {
    if rule.time_basis != TimeBasis::FilenameDate {
        return Ok(None);
    }

    let regex_str = rule
        .filename_date_regex
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FileCleanerError::ConfigValidation(format!(
                "rule '{}' uses time_basis=filename_date without filename_date_regex",
                rule.name
            ))
        })?;

    let format = rule
        .filename_date_format
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FileCleanerError::ConfigValidation(format!(
                "rule '{}' uses time_basis=filename_date without filename_date_format",
                rule.name
            ))
        })?;

    if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        return Err(FileCleanerError::ConfigValidation(format!(
            "rule '{}' has invalid filename_date_format '{format}'",
            rule.name
        )));
    }

    let regex = Regex::new(regex_str).map_err(|source| FileCleanerError::InvalidRegex {
        pattern: regex_str.to_string(),
        source,
    })?;

    Ok(Some(FilenameDateParser { regex, format }))
}

fn parse_filename_timestamp_with(
    parser: &FilenameDateParser<'_>,
    name: &str,
) -> Option<DateTime<Utc>> {
    let captures = parser.regex.captures(name)?;
    let slice = captures
        .name("date")
        .or_else(|| captures.get(1))
        .or_else(|| captures.get(0))?
        .as_str();

    parse_timestamp_slice(slice, parser.format)
}

/// Parses an extracted date/time substring using RFC/offset, naive datetime, or naive date formats.
pub(crate) fn parse_timestamp_slice(slice: &str, format: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_str(slice, format) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(slice, format) {
        return Some(ndt.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(slice, format) {
        return date.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
    }
    None
}

fn build_internal_guard(audit_log: Option<&Path>, daemon: &DaemonConfig) -> InternalArtifactGuard {
    let mut exact_paths = HashSet::new();
    let mut normalized_paths = HashSet::new();
    let mut file_names = HashSet::new();

    for path in [
        audit_log,
        daemon.pid_file.as_deref(),
        daemon.state_file.as_deref(),
        daemon.stop_file.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        exact_paths.insert(path.to_path_buf());
        normalized_paths.insert(resolve_path_lexically(path));
        if let Some(name) = path.file_name() {
            file_names.insert(name.to_os_string());
        }
    }

    InternalArtifactGuard {
        exact_paths,
        normalized_paths,
        file_names,
    }
}

impl InternalArtifactGuard {
    fn matches(&self, candidate: &Path, rule_root: &Path, norm_root: &Path) -> bool {
        if self.exact_paths.is_empty() {
            return false;
        }
        if self.exact_paths.contains(candidate) {
            return true;
        }
        let Some(fname) = candidate.file_name() else {
            return false;
        };
        if !self.file_names.contains(fname) {
            return false;
        }
        if let Ok(rel) = candidate.strip_prefix(rule_root) {
            let joined = norm_root.join(rel);
            if self.normalized_paths.contains(&joined) {
                return true;
            }
        }
        let norm_candidate = resolve_path_lexically(candidate);
        self.normalized_paths.contains(&norm_candidate)
    }
}

fn is_temp_artifact_name(name: &str) -> bool {
    name.ends_with(".fc-tmp") || name.contains(".fc-tmp.")
}

fn is_dir_excluded(
    rel_path: &str,
    name: &str,
    exclude_globs: &[GlobMatcher],
    exclude_regex: Option<&Regex>,
) -> bool {
    if exclude_globs
        .iter()
        .any(|g| g.is_match(rel_path) || g.is_match(name))
    {
        return true;
    }
    if let Some(re) = exclude_regex {
        if re.is_match(rel_path) || re.is_match(name) {
            return true;
        }
    }
    false
}

fn matches_filter_algebra(rel_path: &str, name: &str, filters: &CompiledFilters) -> bool {
    if is_excluded(
        rel_path,
        name,
        &filters.exclude_globs,
        filters.exclude_regex.as_ref(),
    ) {
        return false;
    }

    if filters.include_globs.is_empty() && filters.include_regex.is_none() {
        return true;
    }

    let glob_match = filters
        .include_globs
        .iter()
        .any(|g| g.is_match(rel_path) || g.is_match(name));
    let regex_match = filters
        .include_regex
        .as_ref()
        .is_some_and(|re| re.is_match(rel_path) || re.is_match(name));

    glob_match || regex_match
}

fn is_excluded(
    rel_path: &str,
    name: &str,
    exclude_globs: &[GlobMatcher],
    exclude_regex: Option<&Regex>,
) -> bool {
    if exclude_globs
        .iter()
        .any(|g| g.is_match(rel_path) || g.is_match(name))
    {
        return true;
    }
    if let Some(re) = exclude_regex {
        if re.is_match(rel_path) || re.is_match(name) {
            return true;
        }
    }

    // Check ancestor directory prefixes so directory exclusions like ".trash" exclude descendants.
    let mut start = 0;
    while let Some(offset) = rel_path[start..].find('/') {
        let end = start + offset;
        let prefix = &rel_path[..end];
        if !prefix.is_empty() {
            if exclude_globs.iter().any(|g| g.is_match(prefix)) {
                return true;
            }
            if let Some(re) = exclude_regex {
                if re.is_match(prefix) {
                    return true;
                }
            }
        }
        start = end + 1;
    }

    false
}

fn normalized_rel_path(root: &Path, entry: &Path) -> String {
    let rel = entry.strip_prefix(root).unwrap_or(entry);
    rel.components()
        .filter_map(|comp| match comp {
            Component::Normal(os) => Some(os.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn map_walkdir_error(err: walkdir::Error, fallback_path: &Path) -> FileCleanerError {
    let err_path = err
        .path()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback_path.to_path_buf());
    let loop_msg = err.loop_ancestor().map(|ancestor| {
        format!(
            "symlink loop detected between '{}' and '{}'",
            ancestor.display(),
            err_path.display()
        )
    });
    let io_err = if let Some(msg) = loop_msg {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
    } else if let Some(inner) = err.into_io_error() {
        inner
    } else {
        std::io::Error::other("directory traversal failed")
    };
    FileCleanerError::io(err_path, io_err)
}

/// Converts a [`SystemTime`] into a [`DateTime<Utc>`] without panicking on extreme bounds.
pub(crate) fn system_time_to_utc(st: SystemTime, path: &Path) -> Result<DateTime<Utc>> {
    match st.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let secs = i64::try_from(dur.as_secs()).map_err(|_| {
                FileCleanerError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "timestamp exceeds i64 range",
                    ),
                )
            })?;
            DateTime::from_timestamp(secs, dur.subsec_nanos()).ok_or_else(|| {
                FileCleanerError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "timestamp exceeds chrono range",
                    ),
                )
            })
        }
        Err(before_epoch) => {
            let dur = before_epoch.duration();
            let secs = i64::try_from(dur.as_secs()).map_err(|_| {
                FileCleanerError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "pre-epoch timestamp exceeds i64 range",
                    ),
                )
            })?;
            let nanos = dur.subsec_nanos();
            let (adj_secs, adj_nanos) = if nanos == 0 {
                (-secs, 0)
            } else {
                (-secs - 1, 1_000_000_000 - nanos)
            };
            DateTime::from_timestamp(adj_secs, adj_nanos).ok_or_else(|| {
                FileCleanerError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "pre-epoch timestamp exceeds chrono range",
                    ),
                )
            })
        }
    }
}

fn resolve_path_lexically(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };

    let mut stack: Vec<Component<'_>> = Vec::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                }
            }
            other => stack.push(other),
        }
    }

    let mut lexical = PathBuf::new();
    for comp in stack {
        lexical.push(comp.as_os_str());
    }

    let mut existing = lexical.as_path();
    let mut tail: Vec<OsString> = Vec::new();
    while !existing.exists() {
        if let Some(name) = existing.file_name() {
            tail.push(name.to_os_string());
        } else {
            break;
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => break,
        }
    }

    if let Ok(mut canon) = fs::canonicalize(existing) {
        for seg in tail.into_iter().rev() {
            canon.push(seg);
        }
        canon
    } else {
        lexical
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ActionStep, BackupGuardConfig, CombineMode, EvictionOrder, InUseConfig};
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn set_mtime_unix(path: &Path, unix_secs: u64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(unix_secs))
            .unwrap();
    }

    fn base_rule(name: &str, path: PathBuf) -> RuleConfig {
        RuleConfig {
            name: name.to_string(),
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
            compression: crate::config::CompressionFormat::Gzip,
            compression_level: None,
            compress_remove_source: true,
            compress_overwrite: false,
            copy_truncate_to: None,
            rename_template: None,
        }
    }

    #[test]
    fn clocks_return_expected_timestamps() {
        let fixed = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        let fc = FixedClock(fixed);
        assert_eq!(fc.now(), fixed);

        let before = Utc::now();
        let sys = SystemClock.now();
        let after = Utc::now();
        assert!(sys >= before && sys <= after);
    }

    #[test]
    fn system_time_to_utc_handles_post_and_pre_epoch_and_bounds() {
        let path = Path::new("/tmp/test");
        let post = UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 123_000_000);
        let dt_post = system_time_to_utc(post, path).unwrap();
        assert_eq!(dt_post.timestamp(), 1_700_000_000);
        assert_eq!(dt_post.timestamp_subsec_nanos(), 123_000_000);

        let pre_exact = UNIX_EPOCH - std::time::Duration::from_secs(10);
        let dt_pre_exact = system_time_to_utc(pre_exact, path).unwrap();
        assert_eq!(dt_pre_exact.timestamp(), -10);
        assert_eq!(dt_pre_exact.timestamp_subsec_nanos(), 0);

        let pre_subsec = UNIX_EPOCH - std::time::Duration::new(10, 250_000_000);
        let dt_pre_subsec = system_time_to_utc(pre_subsec, path).unwrap();
        assert_eq!(dt_pre_subsec.timestamp(), -11);
        assert_eq!(dt_pre_subsec.timestamp_subsec_nanos(), 750_000_000);
    }

    #[test]
    fn parse_timestamp_slice_supports_offset_naive_datetime_and_naive_date() {
        let rfc =
            parse_timestamp_slice("2026-11-20T02:00:00+02:00", "%Y-%m-%dT%H:%M:%S%:z").unwrap();
        assert_eq!(rfc, Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap());

        let ndt = parse_timestamp_slice("20261120_021530", "%Y%m%d_%H%M%S").unwrap();
        assert_eq!(ndt, Utc.with_ymd_and_hms(2026, 11, 20, 2, 15, 30).unwrap());

        let date = parse_timestamp_slice("2026-11-20", "%Y-%m-%d").unwrap();
        assert_eq!(date, Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap());

        assert!(parse_timestamp_slice("2026-02-30", "%Y-%m-%d").is_none());
    }

    #[test]
    fn scan_rule_rejects_missing_or_non_directory_path() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let rule_missing = base_rule("missing", missing);
        assert!(matches!(
            Scanner::scan_rule(&rule_missing, None, &DaemonConfig::default()),
            Err(FileCleanerError::Io { .. })
        ));

        let regular_file = tmp.path().join("not_a_dir.txt");
        fs::write(&regular_file, b"hello").unwrap();
        let rule_file = base_rule("file-path", regular_file);
        assert!(matches!(
            Scanner::scan_rule(&rule_file, None, &DaemonConfig::default()),
            Err(FileCleanerError::Io { .. })
        ));
    }

    #[test]
    fn scan_rule_excludes_fc_tmp_and_daemon_operational_files_and_applies_filters() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("backups");
        fs::create_dir_all(root.join("pinned")).unwrap();
        fs::create_dir_all(root.join("sub/deep")).unwrap();

        let f_keep = root.join("db_a.sql");
        let f_sub = root.join("sub/db_b.sql");
        let f_deep = root.join("sub/deep/db_c.sql");
        let f_pinned = root.join("pinned/db_pinned.sql");
        let f_too_small = root.join("tiny.sql");
        let f_too_large = root.join("huge.sql");
        let f_tmp1 = root.join("db_a.sql.fc-tmp");
        let f_tmp2 = root.join("db_a.sql.fc-tmp.1234.1");
        let f_audit = root.join("audit.jsonl");
        let f_pid = root.join("fc.pid");
        let f_state = root.join("state.json");
        let f_stop = root.join("fc.stop");

        for (p, size) in [
            (&f_keep, 100usize),
            (&f_sub, 100),
            (&f_deep, 100),
            (&f_pinned, 100),
            (&f_too_small, 5),
            (&f_too_large, 500),
            (&f_tmp1, 100),
            (&f_tmp2, 100),
            (&f_audit, 100),
            (&f_pid, 100),
            (&f_state, 100),
            (&f_stop, 100),
        ] {
            fs::write(p, vec![b'x'; size]).unwrap();
        }

        // Tie timestamps on f_keep and f_sub to test deterministic rel_path ascending tie-break.
        set_mtime_unix(&f_keep, 1_700_000_000);
        set_mtime_unix(&f_sub, 1_700_000_000);

        let mut rule = base_rule("pg", root.clone());
        rule.recursive = true;
        rule.max_depth = Some(2);
        rule.include_globs = vec!["*.sql".to_string()];
        rule.exclude_globs = vec!["pinned".to_string()];
        rule.min_file_size = Some(10);
        rule.max_file_size = Some(200);

        let daemon = DaemonConfig {
            pid_file: Some(f_pid),
            state_file: Some(f_state),
            stop_file: Some(f_stop),
            ..DaemonConfig::default()
        };

        let report = Scanner::scan_rule(&rule, Some(&f_audit), &daemon).unwrap();
        let rels: Vec<&str> = report.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(rels, vec!["db_a.sql", "sub/db_b.sql"]);
        assert!(report.skipped_timestamp_parse.is_empty());
    }

    #[test]
    fn scan_rule_filename_date_extracts_named_and_positional_groups_and_skips_unparseable() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("dumps");
        fs::create_dir_all(&root).unwrap();

        fs::write(root.join("pg_20261118_010000.sql"), b"old").unwrap();
        fs::write(root.join("pg_20261120_020000.sql"), b"new").unwrap();
        fs::write(root.join("pg_20269999_000000.sql"), b"invalid-date").unwrap();
        fs::write(root.join("unmatched.sql"), b"no-date").unwrap();

        let mut rule = base_rule("fn-date", root.clone());
        rule.time_basis = TimeBasis::FilenameDate;
        rule.filename_date_regex = Some(r"pg_(?P<date>\d{8}_\d{6})".to_string());
        rule.filename_date_format = Some("%Y%m%d_%H%M%S".to_string());

        let report = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].name, "pg_20261120_020000.sql");
        assert_eq!(report.files[1].name, "pg_20261118_010000.sql");
        assert_eq!(report.skipped_timestamp_parse.len(), 2);
        assert_eq!(report.warnings.len(), 2);

        // Positional capture group 1 and full-match group 0 fallback.
        rule.filename_date_regex = Some(r"\d{8}_\d{6}".to_string());
        let report_group0 = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
        assert_eq!(report_group0.files.len(), 2);
    }

    #[test]
    fn scan_rule_time_basis_created_succeeds_or_fails_without_fallback() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("created");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("item.bak");
        fs::write(&file, b"data").unwrap();
        set_mtime_unix(&file, 1_500_000_000);

        let mut rule = base_rule("created", root);
        rule.time_basis = TimeBasis::Created;

        match Scanner::scan_rule(&rule, None, &DaemonConfig::default()) {
            Ok(report) => {
                assert_eq!(report.files.len(), 1);
                assert_eq!(
                    Some(report.files[0].resolved_ts),
                    report.files[0].created_ts
                );
                assert_ne!(report.files[0].resolved_ts, report.files[0].modified_ts);
            }
            Err(FileCleanerError::Io { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::Unsupported);
            }
            Err(other) => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn scan_rule_rejects_invalid_filters_and_filename_date_configuration() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("filters");
        fs::create_dir_all(&root).unwrap();

        let mut bad_inc_glob = base_rule("inc-glob", root.clone());
        bad_inc_glob.include_globs = vec!["[broken".to_string()];
        assert!(matches!(
            Scanner::scan_rule(&bad_inc_glob, None, &DaemonConfig::default()),
            Err(FileCleanerError::InvalidGlob { .. })
        ));

        let mut bad_exc_glob = base_rule("exc-glob", root.clone());
        bad_exc_glob.exclude_globs = vec!["[broken".to_string()];
        assert!(matches!(
            Scanner::scan_rule(&bad_exc_glob, None, &DaemonConfig::default()),
            Err(FileCleanerError::InvalidGlob { .. })
        ));

        let mut bad_inc_re = base_rule("inc-re", root.clone());
        bad_inc_re.include_regex = Some("(broken".to_string());
        assert!(matches!(
            Scanner::scan_rule(&bad_inc_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::InvalidRegex { .. })
        ));

        let mut bad_exc_re = base_rule("exc-re", root.clone());
        bad_exc_re.exclude_regex = Some("(broken".to_string());
        assert!(matches!(
            Scanner::scan_rule(&bad_exc_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::InvalidRegex { .. })
        ));

        let mut missing_fn_re = base_rule("missing-fn-re", root.clone());
        missing_fn_re.time_basis = TimeBasis::FilenameDate;
        assert!(matches!(
            Scanner::scan_rule(&missing_fn_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        missing_fn_re.filename_date_regex = Some(r"(\d{4})".to_string());
        assert!(matches!(
            Scanner::scan_rule(&missing_fn_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        missing_fn_re.filename_date_format = Some("%Q".to_string());
        assert!(matches!(
            Scanner::scan_rule(&missing_fn_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        missing_fn_re.filename_date_format = Some("%Y".to_string());
        missing_fn_re.filename_date_regex = Some("(broken".to_string());
        assert!(matches!(
            Scanner::scan_rule(&missing_fn_re, None, &DaemonConfig::default()),
            Err(FileCleanerError::InvalidRegex { .. })
        ));
    }

    #[test]
    fn system_time_overflow_and_lexical_path_resolution_handle_edge_cases() {
        let path = Path::new("/tmp/overflow");
        let huge_future = UNIX_EPOCH + std::time::Duration::from_secs(u64::MAX / 2);
        assert!(matches!(
            system_time_to_utc(huge_future, path),
            Err(FileCleanerError::Io { .. })
        ));

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        let file = root.join("keep.log");
        let audit_inside = root.join("audit.jsonl");
        fs::write(&file, b"keep").unwrap();
        fs::write(&audit_inside, b"audit").unwrap();

        // Non-existent nested path with `..` exercises lexical normalization and tail reconstruction.
        let alias_audit = root.join("sub/../audit.jsonl");
        let nonexistent_stop = root.join("sub/../missing_dir/daemon.stop");
        let daemon = DaemonConfig {
            stop_file: Some(nonexistent_stop),
            ..DaemonConfig::default()
        };
        let rule = base_rule("norm-guard", root);
        let report = Scanner::scan_rule(&rule, Some(&alias_audit), &daemon).unwrap();
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].name, "keep.log");

        // Relative path normalization works cleanly too.
        let rel = resolve_path_lexically(Path::new("sub/../nonexistent_leaf/file.log"));
        assert!(rel.ends_with("nonexistent_leaf/file.log"));
    }

    #[cfg(unix)]
    #[test]
    fn scanner_skips_non_regular_fifo_entries_and_excludes_nested_regex_dirs() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("specials");
        fs::create_dir_all(root.join("parent/blocked")).unwrap();
        fs::write(root.join("parent/blocked/data.log"), b"blocked").unwrap();
        fs::write(root.join("regular.log"), b"ok").unwrap();

        let fifo_path = root.join("pipe.fifo");
        let c_path = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0);

        let mut rule = base_rule("fifo-and-prefix", root);
        rule.recursive = true;
        rule.exclude_regex = Some(r"^parent/blocked$".to_string());
        let report = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].name, "regular.log");
    }
}
