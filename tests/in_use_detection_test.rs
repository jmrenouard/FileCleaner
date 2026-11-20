//! Integration tests for the 6-layer multi-probe in-use detector.

use chrono::{TimeZone, Utc};
use filecleaner::{
    FixedClock, InUseBatchResult, InUseConfig, InUseDetector, InUseReason, LockProbe, OsLockProbe,
    ProcFdProbeOutcome, ScannedFile, SystemClock,
};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tempfile::TempDir;

fn set_mtime_unix(path: &Path, unix_secs: i64) {
    let f = OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs as u64))
        .unwrap();
}

fn make_scanned_from_disk(path: &Path) -> ScannedFile {
    let meta = fs::metadata(path).unwrap();
    let ts = Utc::now();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    ScannedFile {
        path: path.to_path_buf(),
        rel_path: name.clone(),
        name,
        size_bytes: meta.len(),
        resolved_ts: ts,
        modified_ts: ts,
        created_ts: None,
    }
}

struct FailingMockProbe {
    os_errors: HashMap<PathBuf, String>,
    proc_outcomes: HashMap<PathBuf, ProcFdProbeOutcome>,
}

impl LockProbe for FailingMockProbe {
    fn check_os_lock(&self, path: &Path) -> Result<Option<InUseReason>, String> {
        if let Some(err) = self.os_errors.get(path) {
            Err(err.clone())
        } else {
            Ok(None)
        }
    }

    fn check_proc_fd(&self, path: &Path) -> ProcFdProbeOutcome {
        self.proc_outcomes.get(path).cloned().unwrap_or_default()
    }
}

#[test]
fn detects_sidecars_temp_suffixes_and_min_idle_age_cooldown() {
    let tmp = TempDir::new().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

    let locked = tmp.path().join("backup.tar");
    let lock_sidecar = tmp.path().join("backup.tar.lock");
    let active_tmp = tmp.path().join("stream.tmp");
    let recent = tmp.path().join("recent.tar");
    let old_idle = tmp.path().join("idle.tar");

    for p in [&locked, &lock_sidecar, &active_tmp, &recent, &old_idle] {
        fs::write(p, b"payload").unwrap();
    }

    set_mtime_unix(&locked, now.timestamp() - 600);
    set_mtime_unix(&active_tmp, now.timestamp() - 600);
    set_mtime_unix(&recent, now.timestamp() - 5);
    set_mtime_unix(&old_idle, now.timestamp() - 600);

    let files = vec![
        make_scanned_from_disk(&locked),
        make_scanned_from_disk(&active_tmp),
        make_scanned_from_disk(&recent),
        make_scanned_from_disk(&old_idle),
    ];

    let cfg = InUseConfig {
        check_os_locks: false,
        check_proc_fd: false,
        min_idle_age: Some(Duration::from_secs(30)),
        ..InUseConfig::default()
    };

    let res: InUseBatchResult =
        InUseDetector::check_batch(&files, &cfg, &FixedClock(now), &OsLockProbe);

    assert!(res.is_busy(&locked));
    assert!(res.reports[&locked].reasons.iter().any(
        |r| matches!(r, InUseReason::SidecarLock { lock_path } if lock_path == &lock_sidecar)
    ));

    assert!(res.is_busy(&active_tmp));
    assert!(res.reports[&active_tmp]
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::TempSuffix { suffix } if suffix == ".tmp")));

    assert!(res.is_busy(&recent));
    assert!(res.reports[&recent].reasons.iter().any(|r| matches!(
        r,
        InUseReason::MinIdleAge {
            age_ms: 5_000,
            min_idle_ms: 30_000
        }
    )));

    assert!(!res.is_busy(&old_idle));
    assert!(res.reports[&old_idle].reasons.is_empty());
}

#[test]
fn batched_stability_window_detects_growing_file_in_single_sleep() {
    let tmp = TempDir::new().unwrap();
    let growing = tmp.path().join("growing.log");
    let stable_a = tmp.path().join("stable_a.log");
    let stable_b = tmp.path().join("stable_b.log");
    let stable_c = tmp.path().join("stable_c.log");

    for p in [&growing, &stable_a, &stable_b, &stable_c] {
        fs::write(p, b"initial").unwrap();
    }

    let files = vec![
        make_scanned_from_disk(&growing),
        make_scanned_from_disk(&stable_a),
        make_scanned_from_disk(&stable_b),
        make_scanned_from_disk(&stable_c),
    ];

    let cfg = InUseConfig {
        check_sidecar_locks: false,
        check_os_locks: false,
        check_proc_fd: false,
        min_idle_age: None,
        stability_window: Some(Duration::from_millis(200)),
        ..InUseConfig::default()
    };

    let growing_bg = growing.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        let mut f = OpenOptions::new().append(true).open(&growing_bg).unwrap();
        use std::io::Write;
        f.write_all(b"-more-bytes").unwrap();
        f.sync_all().unwrap();
    });

    let started = Instant::now();
    let res = InUseDetector::check_batch(&files, &cfg, &SystemClock, &OsLockProbe);
    let elapsed = started.elapsed();
    writer.join().unwrap();

    // Batched check across 4 survivors sleeps once (~200ms), well below 4 * 200ms = 800ms.
    assert!(elapsed >= Duration::from_millis(150));
    assert!(elapsed < Duration::from_millis(650));

    assert!(res.is_busy(&growing));
    assert!(res.reports[&growing]
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::SizeOrMtimeChanged)));

    assert!(!res.is_busy(&stable_a));
    assert!(!res.is_busy(&stable_b));
    assert!(!res.is_busy(&stable_c));
}

#[test]
fn mock_lock_probe_errors_fail_closed_and_preserve_proc_diagnostics() {
    let tmp = TempDir::new().unwrap();
    let err_file = tmp.path().join("locked_err.dat");
    let diag_only = tmp.path().join("healthy.dat");
    fs::write(&err_file, b"x").unwrap();
    fs::write(&diag_only, b"y").unwrap();

    let files = vec![
        make_scanned_from_disk(&err_file),
        make_scanned_from_disk(&diag_only),
    ];

    let mut os_errors = HashMap::new();
    os_errors.insert(err_file.clone(), "simulated EIO on flock".to_string());

    let mut proc_outcomes = HashMap::new();
    proc_outcomes.insert(
        diag_only.clone(),
        ProcFdProbeOutcome {
            open_pids: vec![],
            diagnostics: vec!["skipped 12 unprivileged /proc/<pid>/fd directories".to_string()],
            fatal_error: None,
        },
    );

    let probe = FailingMockProbe {
        os_errors,
        proc_outcomes,
    };

    let cfg = InUseConfig {
        check_sidecar_locks: false,
        min_idle_age: None,
        stability_window: None,
        ..InUseConfig::default()
    };

    let res = InUseDetector::check_batch(&files, &cfg, &SystemClock, &probe);
    assert!(res.is_busy(&err_file));
    assert!(res.reports[&err_file]
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::ProbeError { .. })));

    // Unprivileged /proc EACCES diagnostics do NOT falsely block healthy files.
    assert!(!res.is_busy(&diag_only));
    assert_eq!(res.diagnostics.len(), 1);
}

#[cfg(unix)]
#[test]
fn real_unix_flock_blocks_eviction_until_released() {
    use std::os::unix::io::AsRawFd;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("flocked.sql");
    fs::write(&target, b"dump").unwrap();

    let holder = OpenOptions::new().read(true).open(&target).unwrap();
    let rc = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "test setup failed to acquire exclusive flock");

    let cfg = InUseConfig {
        check_sidecar_locks: false,
        check_os_locks: true,
        check_proc_fd: false,
        min_idle_age: None,
        stability_window: None,
        ..InUseConfig::default()
    };

    let busy_report = InUseDetector::check_single(&target, &cfg);
    assert!(busy_report.is_busy);
    assert!(busy_report
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::OsFlock)));

    let rc_un = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) };
    assert_eq!(rc_un, 0);
    drop(holder);

    let free_report = InUseDetector::check_single(&target, &cfg);
    assert!(!free_report.is_busy);
    assert!(free_report.reasons.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn real_linux_proc_fd_detects_child_process_holding_open_descriptor() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("active_writer.log");
    fs::write(&target, b"streaming").unwrap();

    let mut child = Command::new("sh")
        .arg("-c")
        .arg("exec 3<\"$1\"; printf 'READY\\n'; exec sleep 10")
        .arg("sh")
        .arg(&target)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "READY");

    let child_pid = child.id();
    let cfg = InUseConfig {
        check_sidecar_locks: false,
        check_os_locks: false,
        check_proc_fd: true,
        min_idle_age: None,
        stability_window: None,
        ..InUseConfig::default()
    };

    let busy_report = InUseDetector::check_single(&target, &cfg);
    assert!(
        busy_report.is_busy,
        "expected /proc/{child_pid}/fd probe to detect open descriptor"
    );
    assert!(busy_report
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::ProcFdOpen { pid } if *pid == child_pid)));

    let _ = child.kill();
    let _ = child.wait();

    let clean_report = InUseDetector::check_single(&target, &cfg);
    assert!(!clean_report.is_busy);
    assert!(clean_report.reasons.is_empty());
}

#[cfg(windows)]
#[test]
fn real_windows_share_mode_blocks_eviction_until_released() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("locked.sql");
    fs::write(&target, b"dump").unwrap();

    let holder = OpenOptions::new().read(true).open(&target).unwrap();
    let cfg = InUseConfig {
        check_sidecar_locks: false,
        check_os_locks: true,
        check_proc_fd: false,
        min_idle_age: None,
        stability_window: None,
        ..InUseConfig::default()
    };

    let busy_report = InUseDetector::check_single(&target, &cfg);
    assert!(busy_report.is_busy);
    assert!(busy_report
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::WindowsShareViolation)));

    drop(holder);

    let free_report = InUseDetector::check_single(&target, &cfg);
    assert!(!free_report.is_busy);
    assert!(free_report.reasons.is_empty());
}

#[test]
fn stability_window_detects_mtime_only_touch_without_size_change() {
    let tmp = TempDir::new().unwrap();
    let touched = tmp.path().join("touched.log");
    fs::write(&touched, b"constant-size").unwrap();
    let files = vec![make_scanned_from_disk(&touched)];

    let touched_bg = touched.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(35));
        set_mtime_unix(&touched_bg, 1_700_000_123);
    });

    let cfg = InUseConfig {
        check_sidecar_locks: false,
        check_os_locks: false,
        check_proc_fd: false,
        min_idle_age: None,
        stability_window: Some(Duration::from_millis(200)),
        ..InUseConfig::default()
    };

    let res = InUseDetector::check_batch(&files, &cfg, &SystemClock, &OsLockProbe);
    handle.join().unwrap();

    assert!(res.is_busy(&touched));
    assert!(res.reports[&touched]
        .reasons
        .iter()
        .any(|r| matches!(r, InUseReason::SizeOrMtimeChanged)));
}

#[test]
fn non_intrusive_probe_order_skips_os_lock_when_sidecar_or_idle_age_triggers() {
    use std::sync::Mutex;

    #[derive(Default)]
    struct CountingProbe {
        os_lock_calls: Mutex<Vec<PathBuf>>,
    }

    impl LockProbe for CountingProbe {
        fn check_os_lock(&self, path: &Path) -> Result<Option<InUseReason>, String> {
            self.os_lock_calls.lock().unwrap().push(path.to_path_buf());
            Ok(None)
        }

        fn check_proc_fd(&self, _path: &Path) -> ProcFdProbeOutcome {
            ProcFdProbeOutcome::default()
        }
    }

    let tmp = TempDir::new().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

    let f_sidecar = tmp.path().join("sidecar.sql");
    let f_sidecar_lock = tmp.path().join("sidecar.sql.lock");
    let f_recent = tmp.path().join("recent.sql");
    let f_idle = tmp.path().join("idle.sql");

    for p in [&f_sidecar, &f_sidecar_lock, &f_recent, &f_idle] {
        fs::write(p, b"payload").unwrap();
    }
    set_mtime_unix(&f_sidecar, now.timestamp() - 600);
    set_mtime_unix(&f_recent, now.timestamp() - 5);
    set_mtime_unix(&f_idle, now.timestamp() - 600);

    let files = vec![
        make_scanned_from_disk(&f_sidecar),
        make_scanned_from_disk(&f_recent),
        make_scanned_from_disk(&f_idle),
    ];

    let cfg = InUseConfig {
        check_sidecar_locks: true,
        check_os_locks: true,
        check_proc_fd: false,
        min_idle_age: Some(Duration::from_secs(30)),
        stability_window: None,
        ..InUseConfig::default()
    };

    let probe = CountingProbe::default();
    let res = InUseDetector::check_batch(&files, &cfg, &FixedClock(now), &probe);
    assert!(res.is_busy(&f_sidecar));
    assert!(res.is_busy(&f_recent));
    assert!(!res.is_busy(&f_idle));
    assert_eq!(*probe.os_lock_calls.lock().unwrap(), vec![f_idle]);
}

#[cfg(unix)]
#[test]
fn unix_readonly_flock_probe_allows_shared_readers_and_blocks_exclusive_writers() {
    use std::os::unix::io::AsRawFd;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("shared_vs_exclusive.sql");
    fs::write(&target, b"dump").unwrap();

    let holder = OpenOptions::new().read(true).open(&target).unwrap();

    // Shared reader lock: read-only probe does NOT contend, while exclusive probe does.
    let rc_sh = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    assert_eq!(rc_sh, 0);
    assert_eq!(OsLockProbe.check_os_lock_readonly(&target).unwrap(), None);
    assert_eq!(
        OsLockProbe.check_os_lock(&target).unwrap(),
        Some(InUseReason::OsFlock)
    );

    // Exclusive writer lock: both read-only and exclusive probes report OsFlock.
    let rc_ex = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc_ex, 0);
    assert_eq!(
        OsLockProbe.check_os_lock_readonly(&target).unwrap(),
        Some(InUseReason::OsFlock)
    );
    assert_eq!(
        OsLockProbe.check_os_lock(&target).unwrap(),
        Some(InUseReason::OsFlock)
    );

    let _ = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) };
}

#[test]
fn check_paths_batches_five_files_with_single_stability_sleep_and_preserves_order() {
    let tmp = TempDir::new().unwrap();
    let safe_a = tmp.path().join("safe_a.log");
    let growing = tmp.path().join("growing.log");
    let sidecar = tmp.path().join("sidecar.log");
    let sidecar_lock = tmp.path().join("sidecar.log.lock");
    let safe_b = tmp.path().join("safe_b.log");
    let missing = tmp.path().join("missing.log");

    for p in [&safe_a, &growing, &sidecar, &sidecar_lock, &safe_b] {
        fs::write(p, b"steady").unwrap();
    }

    let growing_bg = growing.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(35));
        let mut f = OpenOptions::new().append(true).open(&growing_bg).unwrap();
        use std::io::Write;
        f.write_all(b"-append").unwrap();
        f.sync_all().unwrap();
    });

    let cfg = InUseConfig {
        check_sidecar_locks: true,
        check_os_locks: false,
        check_proc_fd: false,
        min_idle_age: None,
        stability_window: Some(Duration::from_millis(150)),
        ..InUseConfig::default()
    };

    let paths = vec![
        safe_a.clone(),
        growing.clone(),
        sidecar.clone(),
        missing.clone(),
        safe_b.clone(),
    ];

    let started = Instant::now();
    let reports = InUseDetector::check_paths(&paths, &cfg);
    let elapsed = started.elapsed();
    writer.join().unwrap();

    assert!(
        elapsed < Duration::from_millis(550),
        "expected single batched stability sleep (< 550ms), elapsed: {elapsed:?}"
    );
    assert_eq!(reports.len(), 5);
    assert_eq!(reports[0].path, safe_a);
    assert!(!reports[0].is_busy);
    assert_eq!(reports[1].path, growing);
    assert!(reports[1].is_busy);
    assert_eq!(reports[2].path, sidecar);
    assert!(reports[2].is_busy);
    assert_eq!(reports[3].path, missing);
    assert!(reports[3].is_busy);
    assert_eq!(reports[4].path, safe_b);
    assert!(!reports[4].is_busy);
}
