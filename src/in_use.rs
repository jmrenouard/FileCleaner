//! Multi-probe in-use file detection matrix (Phase 2).

use crate::config::InUseConfig;
use crate::scanner::{system_time_to_utc, Clock, ScannedFile, SystemClock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Specific safety probe trigger explaining why a candidate file was classified as in-use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InUseReason {
    /// A companion sidecar lock/temp marker file exists (`<path><suffix>`).
    SidecarLock {
        /// Path of the detected companion sidecar file.
        lock_path: PathBuf,
    },
    /// The candidate filename itself carries an active temporary/lock suffix.
    TempSuffix {
        /// Matching suffix from `in_use.sidecar_suffixes`.
        suffix: String,
    },
    /// Unix cooperative advisory `flock(LOCK_EX | LOCK_NB)` is currently held by another holder.
    OsFlock,
    /// Linux `/proc/<pid>/fd` scan detected an open file descriptor pointing to the candidate inode.
    ProcFdOpen {
        /// Process identifier holding an open descriptor to the candidate.
        pid: u32,
    },
    /// Windows exclusive `share_mode(0)` open failed due to a sharing or lock violation.
    WindowsShareViolation,
    /// Candidate `mtime` is newer than `now - min_idle_age` (or future-skewed relative to `now`).
    MinIdleAge {
        /// Observed idle age in milliseconds (0 when future-skewed).
        age_ms: u64,
        /// Configured minimum idle age threshold in milliseconds.
        min_idle_ms: u64,
    },
    /// Candidate byte size or modification timestamp changed during `stability_window`.
    SizeOrMtimeChanged,
    /// Direct candidate stat/open probe failed; treated fail-closed as busy.
    ProbeError {
        /// Diagnostic description of the probe failure.
        reason: String,
    },
}

impl fmt::Display for InUseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SidecarLock { lock_path } => {
                write!(f, "sidecar lock exists: {}", lock_path.display())
            }
            Self::TempSuffix { suffix } => {
                write!(f, "filename carries active temp/lock suffix '{suffix}'")
            }
            Self::OsFlock => write!(f, "exclusive OS flock is held"),
            Self::ProcFdOpen { pid } => {
                write!(f, "open file descriptor held by pid {pid}")
            }
            Self::WindowsShareViolation => {
                write!(f, "Windows share_mode(0) violation")
            }
            Self::MinIdleAge {
                age_ms,
                min_idle_ms,
            } => write!(
                f,
                "idle age {age_ms}ms is below minimum threshold {min_idle_ms}ms"
            ),
            Self::SizeOrMtimeChanged => {
                write!(f, "file size or mtime changed during stability window")
            }
            Self::ProbeError { reason } => write!(f, "probe error (fail-closed): {reason}"),
        }
    }
}

/// Per-file outcome emitted by the multi-probe in-use detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InUseReport {
    /// Candidate path evaluated.
    pub path: PathBuf,
    /// `true` if any active-file probe triggered or a candidate probe failed fail-closed.
    pub is_busy: bool,
    /// Triggered probe reasons (empty when safe/idle).
    pub reasons: Vec<InUseReason>,
}

/// Batch evaluation results across all candidates in a rule sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InUseBatchResult {
    /// Map of candidate path to its individual [`InUseReport`].
    pub reports: BTreeMap<PathBuf, InUseReport>,
    /// Non-fatal global probe diagnostics (e.g., unprivileged `/proc/*/fd` `EACCES` summary).
    pub diagnostics: Vec<String>,
}

impl InUseBatchResult {
    /// Returns `true` if `path` was reported busy in this batch result.
    pub fn is_busy(&self, path: &Path) -> bool {
        self.reports.get(path).is_some_and(|report| report.is_busy)
    }
}

/// Outcome returned by a `/proc/*/fd` (or platform FD table) probe for one candidate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcFdProbeOutcome {
    /// Process IDs found holding open file descriptors to the candidate.
    pub open_pids: Vec<u32>,
    /// Informational/permission diagnostics encountered during inspection (not candidate-fatal).
    pub diagnostics: Vec<String>,
    /// Direct candidate stat/open error (causes fail-closed `ProbeError`).
    pub fatal_error: Option<String>,
}

/// Injectable OS lock and file-descriptor probe interface enabling deterministic unit testing.
pub trait LockProbe: Send + Sync {
    /// Checks native OS file locks (`flock` on Unix, `share_mode(0)` on Windows).
    ///
    /// Returns `Ok(Some(reason))` if locked, `Ok(None)` if unlocked, or `Err(msg)` if probing
    /// the candidate itself failed (treated fail-closed as [`InUseReason::ProbeError`]).
    fn check_os_lock(&self, path: &Path) -> std::result::Result<Option<InUseReason>, String>;

    /// Checks native OS file locks using non-exclusive read-only probing (`LOCK_SH` on Unix,
    /// `FILE_SHARE_READ` on Windows) so dry-run/plan inspections do not contend with readers.
    fn check_os_lock_readonly(
        &self,
        path: &Path,
    ) -> std::result::Result<Option<InUseReason>, String> {
        self.check_os_lock(path)
    }

    /// Checks whether external processes hold open descriptors referencing `path`.
    fn check_proc_fd(&self, path: &Path) -> ProcFdProbeOutcome;

    /// Batch variant of [`LockProbe::check_proc_fd`] allowing single-pass `/proc` scans.
    fn check_proc_fd_batch(&self, paths: &[&Path]) -> BTreeMap<PathBuf, ProcFdProbeOutcome> {
        paths
            .iter()
            .map(|&path| (path.to_path_buf(), self.check_proc_fd(path)))
            .collect()
    }
}

/// Production OS lock and file-descriptor probe implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsLockProbe;

impl LockProbe for OsLockProbe {
    fn check_os_lock(&self, path: &Path) -> std::result::Result<Option<InUseReason>, String> {
        #[cfg(unix)]
        {
            unix_check_flock_flags(path, libc::LOCK_EX | libc::LOCK_NB)
        }
        #[cfg(windows)]
        {
            windows_check_share_mode_with(path, false)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Ok(None)
        }
    }

    fn check_os_lock_readonly(
        &self,
        path: &Path,
    ) -> std::result::Result<Option<InUseReason>, String> {
        #[cfg(unix)]
        {
            unix_check_flock_flags(path, libc::LOCK_SH | libc::LOCK_NB)
        }
        #[cfg(windows)]
        {
            windows_check_share_mode_with(path, true)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Ok(None)
        }
    }

    fn check_proc_fd(&self, path: &Path) -> ProcFdProbeOutcome {
        let mut batch = self.check_proc_fd_batch(&[path]);
        batch.remove(path).unwrap_or_default()
    }

    fn check_proc_fd_batch(&self, paths: &[&Path]) -> BTreeMap<PathBuf, ProcFdProbeOutcome> {
        #[cfg(target_os = "linux")]
        {
            linux_check_proc_fd_batch(paths, None)
        }
        #[cfg(not(target_os = "linux"))]
        {
            paths
                .iter()
                .map(|&p| (p.to_path_buf(), ProcFdProbeOutcome::default()))
                .collect()
        }
    }
}

/// Sweep-scoped probe caching the Linux `/proc/*/fd` inode map across rules in a single sweep.
#[derive(Debug, Default)]
pub(crate) struct SweepLockProbe {
    #[cfg(target_os = "linux")]
    proc_snapshot: std::sync::OnceLock<ProcInodeSnapshot>,
}

impl LockProbe for SweepLockProbe {
    fn check_os_lock(&self, path: &Path) -> std::result::Result<Option<InUseReason>, String> {
        OsLockProbe.check_os_lock(path)
    }

    fn check_os_lock_readonly(
        &self,
        path: &Path,
    ) -> std::result::Result<Option<InUseReason>, String> {
        OsLockProbe.check_os_lock_readonly(path)
    }

    fn check_proc_fd(&self, path: &Path) -> ProcFdProbeOutcome {
        let mut batch = self.check_proc_fd_batch(&[path]);
        batch.remove(path).unwrap_or_default()
    }

    fn check_proc_fd_batch(&self, paths: &[&Path]) -> BTreeMap<PathBuf, ProcFdProbeOutcome> {
        #[cfg(target_os = "linux")]
        {
            linux_check_proc_fd_batch(paths, Some(&self.proc_snapshot))
        }
        #[cfg(not(target_os = "linux"))]
        {
            OsLockProbe.check_proc_fd_batch(paths)
        }
    }
}

/// Multi-probe active-writer detector enforcing the 6-layer matrix in `SPECIFICATIONS.md` §5.
#[derive(Debug, Clone, Copy, Default)]
pub struct InUseDetector;

impl InUseDetector {
    /// Runs all configured in-use detection probes across a slice of scanned candidate files.
    ///
    /// Layers 1–5 are evaluated first; Layer 6 (`stability_window`) snapshots `(size, mtime)`
    /// across all surviving candidates, sleeps **once** for `stability_window`, and verifies
    /// none changed. Any direct candidate I/O or stat error fails closed (`is_busy = true`).
    pub fn check_batch(
        files: &[ScannedFile],
        cfg: &InUseConfig,
        clock: &dyn Clock,
        probe: &dyn LockProbe,
    ) -> InUseBatchResult {
        Self::check_batch_scoped(files, cfg, clock, probe, &HashSet::new(), false)
    }

    /// Scoped batch probe allowing callers to skip OS lock probing on protected floor candidates
    /// and to request non-exclusive read-only OS lock checks during dry-run/plan passes.
    pub(crate) fn check_batch_scoped(
        files: &[ScannedFile],
        cfg: &InUseConfig,
        clock: &dyn Clock,
        probe: &dyn LockProbe,
        skip_os_locks_for: &HashSet<PathBuf>,
        read_only: bool,
    ) -> InUseBatchResult {
        let mut result = InUseBatchResult::default();
        if files.is_empty() {
            return result;
        }

        if !cfg.enabled {
            for file in files {
                result.reports.insert(
                    file.path.clone(),
                    InUseReport {
                        path: file.path.clone(),
                        is_busy: false,
                        reasons: Vec::new(),
                    },
                );
            }
            return result;
        }

        let proc_fd_map = if cfg.check_proc_fd {
            let paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
            probe.check_proc_fd_batch(&paths)
        } else {
            BTreeMap::new()
        };

        let mut seen_diags: HashSet<String> = HashSet::new();
        let mut per_file_reasons: Vec<Vec<InUseReason>> = vec![Vec::new(); files.len()];
        let now = clock.now();

        for (idx, file) in files.iter().enumerate() {
            let reasons = &mut per_file_reasons[idx];

            // Layer 1: Sidecar lock & temp suffix markers.
            if cfg.check_sidecar_locks {
                check_sidecars(&file.path, &file.name, &cfg.sidecar_suffixes, reasons);
            }

            // Layer 3: Linux /proc/*/fd open descriptor scan.
            if cfg.check_proc_fd {
                if let Some(outcome) = proc_fd_map.get(&file.path) {
                    for diag in &outcome.diagnostics {
                        if seen_diags.insert(diag.clone()) {
                            result.diagnostics.push(diag.clone());
                        }
                    }
                    if let Some(err_msg) = &outcome.fatal_error {
                        reasons.push(InUseReason::ProbeError {
                            reason: err_msg.clone(),
                        });
                    }
                    for &pid in &outcome.open_pids {
                        reasons.push(InUseReason::ProcFdOpen { pid });
                    }
                }
            }

            // Layer 5: Minimum idle age cooldown using live filesystem mtime.
            if let Some(min_idle) = cfg.min_idle_age {
                check_min_idle_age(&file.path, now, min_idle, reasons);
            }

            // Layer 2 / 4: OS-native lock probes run last and skip files already classified or floored.
            if cfg.check_os_locks && reasons.is_empty() && !skip_os_locks_for.contains(&file.path) {
                let lock_res = if read_only {
                    probe.check_os_lock_readonly(&file.path)
                } else {
                    probe.check_os_lock(&file.path)
                };
                match lock_res {
                    Ok(Some(reason)) => reasons.push(reason),
                    Ok(None) => {}
                    Err(err_msg) => reasons.push(InUseReason::ProbeError { reason: err_msg }),
                }
            }
        }

        // Layer 6: Batched two-point stability window across Probes 1-5 survivors.
        if let Some(stab_window) = cfg.stability_window.filter(|d| !d.is_zero()) {
            let survivors: Vec<usize> = per_file_reasons
                .iter()
                .enumerate()
                .filter_map(|(idx, r)| r.is_empty().then_some(idx))
                .collect();

            if !survivors.is_empty() {
                let mut snapshots_p0: Vec<(usize, u64, SystemTime)> =
                    Vec::with_capacity(survivors.len());

                for idx in survivors {
                    let path = &files[idx].path;
                    match snapshot_size_mtime(path) {
                        Ok((size, mtime)) => snapshots_p0.push((idx, size, mtime)),
                        Err(reason) => {
                            per_file_reasons[idx].push(InUseReason::ProbeError { reason });
                        }
                    }
                }

                if !snapshots_p0.is_empty() {
                    std::thread::sleep(stab_window);

                    for (idx, size0, mtime0) in snapshots_p0 {
                        let path = &files[idx].path;
                        match snapshot_size_mtime(path) {
                            Ok((size1, mtime1)) => {
                                if size1 != size0 || mtime1 != mtime0 {
                                    per_file_reasons[idx].push(InUseReason::SizeOrMtimeChanged);
                                }
                            }
                            Err(reason) => {
                                per_file_reasons[idx].push(InUseReason::ProbeError { reason });
                            }
                        }
                    }
                }
            }
        }

        for (file, reasons) in files.iter().zip(per_file_reasons) {
            let is_busy = !reasons.is_empty();
            result.reports.insert(
                file.path.clone(),
                InUseReport {
                    path: file.path.clone(),
                    is_busy,
                    reasons,
                },
            );
        }

        result
    }

    /// Evaluates multiple paths in a single batched pass using [`SystemClock`] and [`OsLockProbe`].
    pub fn check_paths(paths: &[PathBuf], cfg: &InUseConfig) -> Vec<InUseReport> {
        Self::check_paths_with(paths, cfg, &SystemClock, &OsLockProbe)
    }

    fn check_paths_with(
        paths: &[PathBuf],
        cfg: &InUseConfig,
        clock: &dyn Clock,
        probe: &dyn LockProbe,
    ) -> Vec<InUseReport> {
        if !cfg.enabled {
            return paths
                .iter()
                .map(|path| InUseReport {
                    path: path.clone(),
                    is_busy: false,
                    reasons: Vec::new(),
                })
                .collect();
        }

        let mut reports: Vec<Option<InUseReport>> = vec![None; paths.len()];
        let mut valid_files = Vec::with_capacity(paths.len());
        let mut valid_indices = Vec::with_capacity(paths.len());

        for (idx, path) in paths.iter().enumerate() {
            let metadata = match fs::metadata(path) {
                Ok(meta) => meta,
                Err(err) => {
                    reports[idx] = Some(probe_error_report(
                        path,
                        format!("failed to stat '{}': {err}", path.display()),
                    ));
                    continue;
                }
            };

            if !metadata.is_file() {
                reports[idx] = Some(probe_error_report(
                    path,
                    format!("'{}' is not a regular file", path.display()),
                ));
                continue;
            }

            let modified_ts = match metadata
                .modified()
                .map_err(|err| format!("failed to read mtime for '{}': {err}", path.display()))
                .and_then(|st| system_time_to_utc(st, path).map_err(|e| e.to_string()))
            {
                Ok(ts) => ts,
                Err(reason) => {
                    reports[idx] = Some(probe_error_report(path, reason));
                    continue;
                }
            };

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            valid_files.push(ScannedFile {
                path: path.clone(),
                rel_path: name.clone(),
                name,
                size_bytes: metadata.len(),
                resolved_ts: modified_ts,
                modified_ts,
                created_ts: metadata
                    .created()
                    .ok()
                    .and_then(|st| system_time_to_utc(st, path).ok()),
            });
            valid_indices.push(idx);
        }

        if !valid_files.is_empty() {
            let batch = Self::check_batch(&valid_files, cfg, clock, probe);
            for idx in valid_indices {
                let path = &paths[idx];
                reports[idx] = Some(batch.reports.get(path).cloned().unwrap_or_else(|| {
                    probe_error_report(path, "missing batch report for target path".to_string())
                }));
            }
        }

        paths
            .iter()
            .zip(reports)
            .map(|(path, rep)| {
                rep.unwrap_or_else(|| {
                    probe_error_report(path, "missing report for target path".to_string())
                })
            })
            .collect()
    }

    /// Evaluates a single path against [`InUseConfig`] using [`SystemClock`] and [`OsLockProbe`].
    pub fn check_single(path: &Path, cfg: &InUseConfig) -> InUseReport {
        Self::check_single_with(path, cfg, &SystemClock, &OsLockProbe)
    }

    /// Evaluates a single path with injectable [`Clock`] and [`LockProbe`] implementations.
    pub fn check_single_with(
        path: &Path,
        cfg: &InUseConfig,
        clock: &dyn Clock,
        probe: &dyn LockProbe,
    ) -> InUseReport {
        Self::check_paths_with(&[path.to_path_buf()], cfg, clock, probe)
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                probe_error_report(path, "missing batch report for target path".to_string())
            })
    }
}

fn probe_error_report(path: &Path, reason: String) -> InUseReport {
    InUseReport {
        path: path.to_path_buf(),
        is_busy: true,
        reasons: vec![InUseReason::ProbeError { reason }],
    }
}

fn check_sidecars(path: &Path, name: &str, suffixes: &[String], reasons: &mut Vec<InUseReason>) {
    for suffix in suffixes {
        if suffix.is_empty() {
            continue;
        }
        if name.ends_with(suffix.as_str()) {
            reasons.push(InUseReason::TempSuffix {
                suffix: suffix.clone(),
            });
        }
        let mut sidecar_os = path.as_os_str().to_os_string();
        sidecar_os.push(suffix);
        let lock_path = PathBuf::from(sidecar_os);
        match lock_path.try_exists() {
            Ok(true) => reasons.push(InUseReason::SidecarLock { lock_path }),
            Ok(false) => {}
            Err(err) => reasons.push(InUseReason::ProbeError {
                reason: format!("failed to probe sidecar '{}': {err}", lock_path.display()),
            }),
        }
    }
}

fn check_min_idle_age(
    path: &Path,
    now: chrono::DateTime<chrono::Utc>,
    min_idle: Duration,
    reasons: &mut Vec<InUseReason>,
) {
    let min_idle_ms = u64::try_from(min_idle.as_millis()).unwrap_or(u64::MAX);
    let mtime_utc = match fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map_err(|err| format!("failed to read mtime for '{}': {err}", path.display()))
        .and_then(|st| system_time_to_utc(st, path).map_err(|e| e.to_string()))
    {
        Ok(ts) => ts,
        Err(reason) => {
            reasons.push(InUseReason::ProbeError { reason });
            return;
        }
    };

    let age = now.signed_duration_since(mtime_utc);
    if age < chrono::Duration::zero() {
        // Protect future-skewed timestamps fail-closed.
        reasons.push(InUseReason::MinIdleAge {
            age_ms: 0,
            min_idle_ms,
        });
        return;
    }

    let age_ms = u64::try_from(age.num_milliseconds().max(0)).unwrap_or(u64::MAX);
    match chrono::Duration::from_std(min_idle) {
        Ok(min_idle_chrono) => {
            if age < min_idle_chrono {
                reasons.push(InUseReason::MinIdleAge {
                    age_ms,
                    min_idle_ms,
                });
            }
        }
        Err(_) => {
            // Threshold exceeds chrono::Duration::MAX (~292B years); any finite age is below it.
            reasons.push(InUseReason::MinIdleAge {
                age_ms,
                min_idle_ms,
            });
        }
    }
}

fn snapshot_size_mtime(path: &Path) -> std::result::Result<(u64, SystemTime), String> {
    let meta = fs::metadata(path).map_err(|err| {
        format!(
            "failed to stat '{}' for stability check: {err}",
            path.display()
        )
    })?;
    let mtime = meta.modified().map_err(|err| {
        format!(
            "failed to read mtime of '{}' for stability check: {err}",
            path.display()
        )
    })?;
    Ok((meta.len(), mtime))
}

#[cfg(unix)]
fn unix_check_flock_flags(
    path: &Path,
    flags: libc::c_int,
) -> std::result::Result<Option<InUseReason>, String> {
    use std::os::unix::io::AsRawFd;

    let file = fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|err| format!("failed to open '{}' for flock probe: {err}", path.display()))?;
    let fd = file.as_raw_fd();

    loop {
        // SAFETY: `fd` is a valid open descriptor owned by `file` for the duration of this loop.
        let rc = unsafe { libc::flock(fd, flags) };
        if rc == 0 {
            // SAFETY: `fd` holds the lock just acquired; release immediately before close.
            let _ = unsafe { libc::flock(fd, libc::LOCK_UN) };
            return Ok(None);
        }

        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if matches!(
            err.raw_os_error(),
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
        ) {
            return Ok(Some(InUseReason::OsFlock));
        }
        return Err(format!("flock probe failed on '{}': {err}", path.display()));
    }
}

#[cfg(windows)]
fn windows_check_share_mode_with(
    path: &Path,
    read_only: bool,
) -> std::result::Result<Option<InUseReason>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    let mut opts = fs::OpenOptions::new();
    if read_only {
        opts.access_mode(0).share_mode(1);
    } else {
        opts.read(true).share_mode(0);
    }

    match opts.open(path) {
        Ok(_file) => Ok(None),
        Err(err) => match err.raw_os_error() {
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => {
                Ok(Some(InUseReason::WindowsShareViolation))
            }
            _ => Err(format!(
                "Windows {} probe failed on '{}': {err}",
                if read_only {
                    "share_mode(1)"
                } else {
                    "share_mode(0)"
                },
                path.display()
            )),
        },
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct ProcInodeSnapshot {
    inode_pids: std::collections::HashMap<(u64, u64), Vec<u32>>,
    diagnostics: Vec<String>,
}

#[cfg(target_os = "linux")]
impl ProcInodeSnapshot {
    fn capture(filter: Option<&HashSet<(u64, u64)>>) -> Self {
        use std::os::unix::fs::MetadataExt;

        let mut snapshot = Self::default();
        let proc_entries = match fs::read_dir("/proc") {
            Ok(entries) => entries,
            Err(err) => {
                snapshot
                    .diagnostics
                    .push(format!("unable to read /proc for fd inspection: {err}"));
                return snapshot;
            }
        };

        let self_pid = std::process::id();
        let mut permission_denied_pids = 0usize;

        for proc_entry in proc_entries.flatten() {
            let Ok(pid_str) = proc_entry.file_name().into_string() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }

            let fd_dir = proc_entry.path().join("fd");
            let fd_entries = match fs::read_dir(&fd_dir) {
                Ok(entries) => entries,
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        permission_denied_pids += 1;
                    }
                    continue;
                }
            };

            for fd_entry in fd_entries.flatten() {
                if let Ok(fd_meta) = fs::metadata(fd_entry.path()) {
                    // Regular-file check excludes pipes, sockets, and anon_inodes in one stat call.
                    if !fd_meta.is_file() {
                        continue;
                    }
                    let key = (fd_meta.dev(), fd_meta.ino());
                    if filter.is_none_or(|wanted| wanted.contains(&key)) {
                        let pids = snapshot.inode_pids.entry(key).or_default();
                        if !pids.contains(&pid) {
                            pids.push(pid);
                        }
                    }
                }
            }
        }

        if permission_denied_pids > 0 {
            snapshot.diagnostics.push(format!(
                "skipped {permission_denied_pids} unprivileged /proc/<pid>/fd directories (EACCES/EPERM)"
            ));
        }

        snapshot
    }
}

#[cfg(target_os = "linux")]
fn linux_check_proc_fd_batch(
    paths: &[&Path],
    cache: Option<&std::sync::OnceLock<ProcInodeSnapshot>>,
) -> BTreeMap<PathBuf, ProcFdProbeOutcome> {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;

    let mut outcomes: BTreeMap<PathBuf, ProcFdProbeOutcome> = BTreeMap::new();
    let mut target_inodes: HashMap<(u64, u64), Vec<PathBuf>> = HashMap::new();

    for &path in paths {
        let mut outcome = ProcFdProbeOutcome::default();
        match fs::metadata(path) {
            Ok(meta) => {
                target_inodes
                    .entry((meta.dev(), meta.ino()))
                    .or_default()
                    .push(path.to_path_buf());
            }
            Err(err) => {
                outcome.fatal_error = Some(format!(
                    "failed to stat '{}' for /proc fd probe: {err}",
                    path.display()
                ));
            }
        }
        outcomes.insert(path.to_path_buf(), outcome);
    }

    if target_inodes.is_empty() {
        return outcomes;
    }

    let owned_snapshot;
    let snapshot = match cache {
        Some(lock) => lock.get_or_init(|| ProcInodeSnapshot::capture(None)),
        None => {
            let filter: HashSet<(u64, u64)> = target_inodes.keys().copied().collect();
            owned_snapshot = ProcInodeSnapshot::capture(Some(&filter));
            &owned_snapshot
        }
    };

    for diag in &snapshot.diagnostics {
        for outcome in outcomes.values_mut() {
            outcome.diagnostics.push(diag.clone());
        }
    }

    for (key, matched_paths) in target_inodes {
        if let Some(pids) = snapshot.inode_pids.get(&key) {
            for matched_path in matched_paths {
                if let Some(outcome) = outcomes.get_mut(&matched_path) {
                    for &pid in pids {
                        if !outcome.open_pids.contains(&pid) {
                            outcome.open_pids.push(pid);
                        }
                    }
                }
            }
        }
    }

    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::FixedClock;
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    fn set_mtime_unix(path: &Path, unix_secs: i64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs as u64))
            .unwrap();
    }

    struct MockLockProbe {
        os_locks: HashMap<PathBuf, std::result::Result<Option<InUseReason>, String>>,
        proc_fds: HashMap<PathBuf, ProcFdProbeOutcome>,
    }

    impl MockLockProbe {
        fn new() -> Self {
            Self {
                os_locks: HashMap::new(),
                proc_fds: HashMap::new(),
            }
        }
    }

    impl LockProbe for MockLockProbe {
        fn check_os_lock(&self, path: &Path) -> std::result::Result<Option<InUseReason>, String> {
            self.os_locks.get(path).cloned().unwrap_or(Ok(None))
        }

        fn check_proc_fd(&self, path: &Path) -> ProcFdProbeOutcome {
            self.proc_fds.get(path).cloned().unwrap_or_default()
        }
    }

    fn make_scanned(path: &Path, mtime_secs: i64) -> ScannedFile {
        let ts = Utc.timestamp_opt(mtime_secs, 0).unwrap();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ScannedFile {
            path: path.to_path_buf(),
            rel_path: name.clone(),
            name,
            size_bytes: 128,
            resolved_ts: ts,
            modified_ts: ts,
            created_ts: None,
        }
    }

    #[test]
    fn reason_display_and_disabled_in_use_config_behave_cleanly() {
        let reason = InUseReason::MinIdleAge {
            age_ms: 500,
            min_idle_ms: 5000,
        };
        assert!(reason.to_string().contains("500ms"));
        assert!(InUseReason::SidecarLock {
            lock_path: PathBuf::from("/tmp/a.lock")
        }
        .to_string()
        .contains("a.lock"));
        assert!(InUseReason::TempSuffix {
            suffix: ".part".to_string()
        }
        .to_string()
        .contains(".part"));
        assert!(InUseReason::OsFlock.to_string().contains("flock"));
        assert!(InUseReason::WindowsShareViolation
            .to_string()
            .contains("share_mode"));
        assert!(InUseReason::SizeOrMtimeChanged
            .to_string()
            .contains("stability"));
        assert!(InUseReason::ProcFdOpen { pid: 42 }
            .to_string()
            .contains("42"));
        assert!(InUseReason::ProbeError {
            reason: "io failed".to_string()
        }
        .to_string()
        .contains("io failed"));

        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("ok.log");
        fs::write(&p, b"abc").unwrap();
        let cfg = InUseConfig {
            enabled: false,
            ..InUseConfig::default()
        };
        let report = InUseDetector::check_single(&p, &cfg);
        assert!(!report.is_busy);
        assert!(report.reasons.is_empty());

        let empty_batch =
            InUseDetector::check_batch(&[], &cfg, &SystemClock, &MockLockProbe::new());
        assert!(empty_batch.reports.is_empty());

        let scanned = vec![make_scanned(&p, 1_700_000_000)];
        let disabled_batch =
            InUseDetector::check_batch(&scanned, &cfg, &SystemClock, &MockLockProbe::new());
        assert!(!disabled_batch.is_busy(&p));
    }

    #[test]
    fn check_batch_detects_sidecar_temp_suffix_mock_errors_and_future_mtime() {
        let tmp = TempDir::new().unwrap();
        let f_sidecar = tmp.path().join("dump.sql");
        let f_sidecar_marker = tmp.path().join("dump.sql.lock");
        let f_temp = tmp.path().join("upload.part");
        let f_future = tmp.path().join("future.sql");
        let f_probe_err = tmp.path().join("err.sql");

        fs::write(&f_sidecar, b"data").unwrap();
        fs::write(&f_sidecar_marker, b"").unwrap();
        fs::write(&f_temp, b"partial").unwrap();
        fs::write(&f_future, b"future").unwrap();
        fs::write(&f_probe_err, b"err").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        set_mtime_unix(&f_sidecar, now.timestamp() - 3600);
        set_mtime_unix(&f_temp, now.timestamp() - 3600);
        set_mtime_unix(&f_probe_err, now.timestamp() - 3600);
        // Set mtime 60 seconds in the future relative to FixedClock.
        set_mtime_unix(&f_future, now.timestamp() + 60);

        let files = vec![
            make_scanned(&f_sidecar, now.timestamp() - 3600),
            make_scanned(&f_temp, now.timestamp() - 3600),
            make_scanned(&f_future, now.timestamp() + 60),
            make_scanned(&f_probe_err, now.timestamp() - 3600),
        ];

        let mut mock = MockLockProbe::new();
        mock.os_locks
            .insert(f_probe_err.clone(), Err("permission denied".to_string()));
        mock.proc_fds.insert(
            f_sidecar.clone(),
            ProcFdProbeOutcome {
                open_pids: vec![],
                diagnostics: vec!["skipped 1 unprivileged /proc/<pid>/fd".to_string()],
                fatal_error: None,
            },
        );

        let cfg = InUseConfig {
            min_idle_age: Some(Duration::from_secs(10)),
            ..InUseConfig::default()
        };

        let res = InUseDetector::check_batch(&files, &cfg, &FixedClock(now), &mock);
        assert!(res.is_busy(&f_sidecar));
        assert!(res.reports[&f_sidecar]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::SidecarLock { .. })));

        assert!(res.is_busy(&f_temp));
        assert!(res.reports[&f_temp]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::TempSuffix { suffix } if suffix == ".part")));

        assert!(res.is_busy(&f_future));
        assert!(res.reports[&f_future]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::MinIdleAge { age_ms: 0, .. })));

        assert!(res.is_busy(&f_probe_err));
        assert!(res.reports[&f_probe_err]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::ProbeError { .. })));

        assert_eq!(res.diagnostics.len(), 1);
    }

    #[test]
    fn check_single_fails_closed_on_missing_or_directory_path() {
        let tmp = TempDir::new().unwrap();
        let cfg = InUseConfig::default();

        let dir_report = InUseDetector::check_single(tmp.path(), &cfg);
        assert!(dir_report.is_busy);
        assert!(matches!(
            dir_report.reasons.first(),
            Some(InUseReason::ProbeError { .. })
        ));

        let missing_report = InUseDetector::check_single(&tmp.path().join("missing.sql"), &cfg);
        assert!(missing_report.is_busy);
        assert!(matches!(
            missing_report.reasons.first(),
            Some(InUseReason::ProbeError { .. })
        ));
    }

    #[test]
    fn check_batch_handles_proc_fatal_error_vanished_stability_file_and_huge_idle_age() {
        let tmp = TempDir::new().unwrap();
        let p_fatal = tmp.path().join("fatal.dat");
        let p_vanish_p0 = tmp.path().join("vanish_p0.dat");
        let p_vanish_p1 = tmp.path().join("vanish_p1.dat");
        let p_huge_idle = tmp.path().join("huge_idle.dat");

        fs::write(&p_fatal, b"a").unwrap();
        fs::write(&p_vanish_p1, b"b").unwrap();
        fs::write(&p_huge_idle, b"c").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        set_mtime_unix(&p_huge_idle, now.timestamp() - 60);

        let mut mock = MockLockProbe::new();
        mock.proc_fds.insert(
            p_fatal.clone(),
            ProcFdProbeOutcome {
                open_pids: vec![],
                diagnostics: vec!["diag-a".to_string()],
                fatal_error: Some("stat failed in /proc probe".to_string()),
            },
        );

        let files = vec![
            make_scanned(&p_fatal, now.timestamp() - 60),
            make_scanned(&p_vanish_p0, now.timestamp() - 60),
        ];
        let cfg_fatal = InUseConfig {
            check_sidecar_locks: false,
            check_os_locks: false,
            check_proc_fd: true,
            min_idle_age: None,
            stability_window: Some(Duration::from_millis(20)),
            ..InUseConfig::default()
        };
        let res = InUseDetector::check_batch(&files, &cfg_fatal, &FixedClock(now), &mock);
        assert!(res.is_busy(&p_fatal));
        assert!(res.is_busy(&p_vanish_p0));
        assert!(res.reports[&p_vanish_p0]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::ProbeError { .. })));

        // Vanishing between p0 and p1 in stability window fails closed with ProbeError.
        let p1_clone = p_vanish_p1.clone();
        let deleter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(15));
            let _ = fs::remove_file(&p1_clone);
        });
        let res_p1 = InUseDetector::check_batch(
            &[make_scanned(&p_vanish_p1, now.timestamp() - 60)],
            &InUseConfig {
                check_sidecar_locks: false,
                check_os_locks: false,
                check_proc_fd: false,
                min_idle_age: None,
                stability_window: Some(Duration::from_millis(80)),
                ..InUseConfig::default()
            },
            &FixedClock(now),
            &mock,
        );
        deleter.join().unwrap();
        assert!(res_p1.is_busy(&p_vanish_p1));
        assert!(res_p1.reports[&p_vanish_p1]
            .reasons
            .iter()
            .any(|r| matches!(r, InUseReason::ProbeError { .. })));

        // Huge min_idle_age exceeding chrono::Duration::MAX still safely marks finite-age file busy.
        let res_huge = InUseDetector::check_batch(
            &[make_scanned(&p_huge_idle, now.timestamp() - 60)],
            &InUseConfig {
                check_sidecar_locks: false,
                check_os_locks: false,
                check_proc_fd: false,
                min_idle_age: Some(Duration::from_secs(u64::MAX)),
                stability_window: None,
                ..InUseConfig::default()
            },
            &FixedClock(now),
            &mock,
        );
        assert!(res_huge.is_busy(&p_huge_idle));

        // Direct OsLockProbe check_proc_fd on missing file fails closed on Linux and returns
        // an empty outcome on platforms without `/proc/*/fd` inspection.
        let direct_missing = OsLockProbe.check_proc_fd(&tmp.path().join("nope.dat"));
        #[cfg(target_os = "linux")]
        assert!(direct_missing.fatal_error.is_some());
        #[cfg(not(target_os = "linux"))]
        assert!(direct_missing.fatal_error.is_none() && direct_missing.open_pids.is_empty());
    }

    #[test]
    fn check_batch_scoped_skips_floor_and_recent_files_and_uses_readonly_probe() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct CountingProbe {
            ex_calls: Mutex<Vec<PathBuf>>,
            ro_calls: Mutex<Vec<PathBuf>>,
        }

        impl LockProbe for CountingProbe {
            fn check_os_lock(
                &self,
                path: &Path,
            ) -> std::result::Result<Option<InUseReason>, String> {
                self.ex_calls.lock().unwrap().push(path.to_path_buf());
                Ok(None)
            }

            fn check_os_lock_readonly(
                &self,
                path: &Path,
            ) -> std::result::Result<Option<InUseReason>, String> {
                self.ro_calls.lock().unwrap().push(path.to_path_buf());
                Ok(None)
            }

            fn check_proc_fd(&self, _path: &Path) -> ProcFdProbeOutcome {
                ProcFdProbeOutcome::default()
            }
        }

        let tmp = TempDir::new().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

        let p_floor = tmp.path().join("floor.sql");
        let p_recent = tmp.path().join("recent.sql");
        let p_idle = tmp.path().join("idle.sql");
        for (p, age) in [(&p_floor, 600), (&p_recent, 5), (&p_idle, 600)] {
            fs::write(p, b"data").unwrap();
            set_mtime_unix(p, now.timestamp() - age);
        }

        let files = vec![
            make_scanned(&p_floor, now.timestamp() - 600),
            make_scanned(&p_recent, now.timestamp() - 5),
            make_scanned(&p_idle, now.timestamp() - 600),
        ];
        let mut skip_floor = HashSet::new();
        skip_floor.insert(p_floor.clone());

        let cfg = InUseConfig {
            check_sidecar_locks: false,
            check_proc_fd: false,
            check_os_locks: true,
            min_idle_age: Some(Duration::from_secs(30)),
            stability_window: None,
            ..InUseConfig::default()
        };

        let ro_probe = CountingProbe::default();
        let ro_res = InUseDetector::check_batch_scoped(
            &files,
            &cfg,
            &FixedClock(now),
            &ro_probe,
            &skip_floor,
            true,
        );
        assert!(!ro_res.is_busy(&p_floor));
        assert!(ro_res.is_busy(&p_recent));
        assert!(!ro_res.is_busy(&p_idle));
        assert!(ro_probe.ex_calls.lock().unwrap().is_empty());
        assert_eq!(*ro_probe.ro_calls.lock().unwrap(), vec![p_idle.clone()]);

        let ex_probe = CountingProbe::default();
        let _ex_res = InUseDetector::check_batch_scoped(
            &files,
            &cfg,
            &FixedClock(now),
            &ex_probe,
            &skip_floor,
            false,
        );
        assert!(ex_probe.ro_calls.lock().unwrap().is_empty());
        assert_eq!(*ex_probe.ex_calls.lock().unwrap(), vec![p_idle]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sweep_lock_probe_reuses_single_proc_snapshot_across_batches_and_fails_closed_on_missing() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("open1.log");
        let f2 = tmp.path().join("closed2.log");
        fs::write(&f1, b"one").unwrap();
        fs::write(&f2, b"two").unwrap();

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exec 3<\"$1\"; printf 'READY\\n'; exec sleep 10")
            .arg("sh")
            .arg(&f1)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "READY");

        let probe = SweepLockProbe::default();
        let batch1 = probe.check_proc_fd_batch(&[f1.as_path()]);
        assert!(batch1[&f1].open_pids.contains(&child.id()));

        let _ = child.kill();
        let _ = child.wait();

        // Second call uses the cached snapshot in `OnceLock` (so f1 still reflects the snapshot
        // from rule 1 rather than re-walking `/proc`), while missing paths still fail closed.
        let missing = tmp.path().join("missing.log");
        let batch2 = probe.check_proc_fd_batch(&[f1.as_path(), f2.as_path(), missing.as_path()]);
        assert!(batch2[&f1].open_pids.contains(&child.id()));
        assert!(batch2[&f2].open_pids.is_empty());
        assert!(batch2[&missing].fatal_error.is_some());

        // Single-path SweepLockProbe::check_proc_fd and disabled check_paths coverage.
        assert!(probe.check_proc_fd(&f2).open_pids.is_empty());
        let disabled_paths = InUseDetector::check_paths(
            &[f1.clone(), tmp.path().to_path_buf(), missing.clone()],
            &InUseConfig {
                enabled: false,
                ..InUseConfig::default()
            },
        );
        assert_eq!(disabled_paths.len(), 3);
        assert!(disabled_paths.iter().all(|r| !r.is_busy));

        let mixed = InUseDetector::check_paths(
            &[f2.clone(), tmp.path().to_path_buf(), missing.clone()],
            &InUseConfig {
                check_sidecar_locks: true,
                sidecar_suffixes: vec!["".to_string(), ".lock".to_string()],
                check_os_locks: false,
                check_proc_fd: false,
                ..InUseConfig::default()
            },
        );
        assert_eq!(mixed.len(), 3);
        assert!(!mixed[0].is_busy);
        assert!(mixed[1].is_busy && mixed[2].is_busy);

        // Self-held open file descriptor + missing-only target_inodes batch probe.
        let _self_open = fs::File::open(&f2).unwrap();
        let _ = probe.check_proc_fd(&f2);
        let _ = probe.check_proc_fd(&missing);
    }
}
