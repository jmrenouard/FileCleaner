//! End-to-end CLI (`assert_cmd`) and library tests for `SweepEngine`, `DaemonRunner`,
//! exit codes (`0`, `1`, `2`, `3`, `4`, `10`), ad-hoc sweeps, `check-in-use`, `init`,
//! `validate`, `plan`, `run`, `daemon`, and cooperative mid-sweep shutdown.

use assert_cmd::Command;
use chrono::{TimeZone, Utc};
use filecleaner::{
    ActionStep, AuditConfig, BackupGuardConfig, CombineMode, CompressionFormat, Config,
    DaemonConfig, DaemonMode, DaemonOptions, DaemonRunner, DaemonState, DaemonStatus,
    DaemonStatusReport, EvictionOrder, FixedClock, HookCandidate, HookConfig, HookFailurePolicy,
    HookRunner, HookVerdict, InUseConfig, OsLockProbe, RuleConfig, RunOptions, SweepEngine,
    SweepReport, TimeBasis,
};
use predicates::prelude::*;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tempfile::TempDir;

fn set_mtime_unix(path: &Path, unix_secs: i64) {
    let f = OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs as u64))
        .unwrap();
}

fn cli_bin() -> Command {
    Command::cargo_bin("filecleaner").unwrap()
}

fn write_toml_escape(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

#[test]
fn cli_init_and_validate_cover_all_profiles_formats_and_force_semantics() {
    let tmp = TempDir::new().unwrap();

    for (profile, format, ext) in [
        ("backups", "toml", "toml"),
        ("backups", "yaml", "yaml"),
        ("logs", "toml", "toml"),
        ("logs", "yaml", "yaml"),
        ("spool", "toml", "toml"),
        ("spool", "yaml", "yaml"),
    ] {
        let out_path = tmp.path().join(format!("{profile}_{format}.{ext}"));
        cli_bin()
            .args([
                "init",
                "--profile",
                profile,
                "--format",
                format,
                "--output",
                out_path.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("dry_run = true"));

        // Re-running without --force refuses to overwrite (exit 1).
        cli_bin()
            .args([
                "init",
                "--profile",
                profile,
                "--format",
                format,
                "--output",
                out_path.to_str().unwrap(),
            ])
            .assert()
            .code(1);

        // Re-running with --force and --json succeeds cleanly.
        let init_out = cli_bin()
            .args([
                "--json",
                "init",
                "--profile",
                profile,
                "--format",
                format,
                "--output",
                out_path.to_str().unwrap(),
                "--force",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let init_json: serde_json::Value = serde_json::from_slice(&init_out).unwrap();
        assert_eq!(init_json["dry_run"], true);

        // Positional `validate <PATH>` and `--json` pass cleanly.
        let val_out = cli_bin()
            .args(["--json", "validate", out_path.to_str().unwrap()])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let val_json: serde_json::Value = serde_json::from_slice(&val_out).unwrap();
        assert_eq!(val_json["valid"], true);
        assert_eq!(val_json["rules"], 1);
    }

    // Invalid config returns deterministic exit code 2.
    let bad_cfg = tmp.path().join("bad.toml");
    fs::write(&bad_cfg, "rules = []\n").unwrap();
    cli_bin()
        .args(["validate", bad_cfg.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("at least one rule"));

    cli_bin()
        .args(["--json", "validate", bad_cfg.to_str().unwrap()])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"valid\": false"));
}

#[test]
fn cli_plan_dry_run_parity_clean_alias_and_audit_log_override() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("dumps");
    let dest_dir = tmp.path().join("archive");
    let audit_override = tmp.path().join("custom-audit.jsonl");
    fs::create_dir_all(&src_dir).unwrap();

    let now_s = Utc::now().timestamp();
    let f_new = src_dir.join("db_new.sql");
    let f_mid = src_dir.join("db_mid.sql");
    let f_old = src_dir.join("db_old.sql");
    for (p, age) in [(&f_new, 600), (&f_mid, 1200), (&f_old, 1800)] {
        fs::write(p, b"SELECT 1;\n".repeat(32)).unwrap();
        set_mtime_unix(p, now_s - age);
    }

    let cfg_path = tmp.path().join("sweep.toml");
    fs::write(
        &cfg_path,
        format!(
            r#"
[[rules]]
name = "db-dumps"
path = "{}"
max_files = 1
actions = ["compress", "move"]
compression = "gzip"
move_to = "{}"

[rules.in_use]
check_os_locks = false
check_proc_fd = false
min_idle_age = "60s"
"#,
            write_toml_escape(&src_dir),
            write_toml_escape(&dest_dir),
        ),
    )
    .unwrap();

    // 1. `plan --json` performs zero writes inside monitored or destination paths.
    let plan_bytes = cli_bin()
        .args(["plan", "--config", cfg_path.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plan_report: SweepReport = serde_json::from_slice(&plan_bytes).unwrap();
    assert!(plan_report.dry_run);
    assert_eq!(plan_report.summary.total_acted, 2);
    assert!(f_old.exists() && f_mid.exists() && f_new.exists());
    assert!(!dest_dir.exists());

    // 2. `run --dry-run --json` yields identical SweepPlan decisions.
    let dry_bytes = cli_bin()
        .args([
            "run",
            "--config",
            cfg_path.to_str().unwrap(),
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dry_report: SweepReport = serde_json::from_slice(&dry_bytes).unwrap();
    assert_eq!(
        plan_report.rule_reports[0].plan.evict_files,
        dry_report.rule_reports[0].plan.evict_files
    );

    // 3. `clean` alias executes live sweep with `-v` and `--audit-log` override.
    cli_bin()
        .args([
            "-v",
            "--audit-log",
            audit_override.to_str().unwrap(),
            "clean",
            "--config",
            cfg_path.to_str().unwrap(),
            "--rule",
            "db-dumps",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Totals: 3 evaluated, 2 acted"));

    assert!(!f_old.exists());
    assert!(!f_mid.exists());
    assert!(f_new.exists());
    assert!(dest_dir.join("db_old.sql.gz").exists());
    assert!(dest_dir.join("db_mid.sql.gz").exists());

    let audit_raw = fs::read_to_string(&audit_override).unwrap();
    assert!(audit_raw.contains("\"sweep_start\""));
    assert!(audit_raw.contains("\"action_exec\""));
    assert!(audit_raw.contains("\"sweep_end\""));
}

#[test]
fn cli_adhoc_dir_cleanup_and_trash_and_strict_exit_codes() {
    let tmp = TempDir::new().unwrap();
    let spool = tmp.path().join("spool");
    let keep_sub = spool.join("keep");
    let sub = spool.join("sub");
    let dest = tmp.path().join("cold");
    let trash = tmp.path().join("trash");
    fs::create_dir_all(&keep_sub).unwrap();
    fs::create_dir_all(&sub).unwrap();

    let now_s = Utc::now().timestamp();
    let f_keep_excl = keep_sub.join("pinned.log");
    let f_new = spool.join("newest.log");
    let f_old = sub.join("older.log");
    for (p, age) in [(&f_keep_excl, 5000), (&f_new, 300), (&f_old, 2000)] {
        fs::write(p, b"log-payload-data").unwrap();
        set_mtime_unix(p, now_s - age);
    }

    // Ad-hoc single-directory sweep: recursive + include + exclude + max-files + rename + compress + move.
    cli_bin()
        .args([
            "run",
            "--dir",
            spool.to_str().unwrap(),
            "--include",
            "*.log",
            "--exclude",
            "keep/**",
            "--recursive",
            "--max-files",
            "1",
            "--min-idle-age",
            "10s",
            "--rename-by-date",
            "--compress",
            "lz4",
            "--move-to",
            dest.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(f_keep_excl.exists());
    assert!(f_new.exists());
    assert!(!f_old.exists());
    let moved_entries: Vec<_> = fs::read_dir(&dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(moved_entries.len(), 1);
    assert!(moved_entries[0].starts_with("older_") && moved_entries[0].ends_with(".log.lz4"));

    // Ad-hoc trash mode on remaining file with --max-age 100s.
    cli_bin()
        .args([
            "run",
            "--dir",
            spool.to_str().unwrap(),
            "--max-age",
            "100s",
            "--trash-dir",
            trash.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(!f_new.exists());
    assert!(trash.join("newest.log").exists());

    // Strict backup freeze (`--fail-on-freeze` -> exit 3) & strict constraints (`--strict-constraints` -> exit 3).
    let freeze_dir = tmp.path().join("freeze_vault");
    fs::create_dir_all(&freeze_dir).unwrap();
    let old_bkp = freeze_dir.join("bkp.sql");
    fs::write(&old_bkp, b"valid-bytes").unwrap();
    set_mtime_unix(&old_bkp, now_s - 72 * 3600);

    // Without --fail-on-freeze -> exit 0, file kept.
    cli_bin()
        .args([
            "run",
            "--dir",
            freeze_dir.to_str().unwrap(),
            "--max-files",
            "0",
            "--require-newer-within",
            "24h",
        ])
        .assert()
        .code(0);
    assert!(old_bkp.exists());

    // With --fail-on-freeze -> exit 3.
    cli_bin()
        .args([
            "run",
            "--dir",
            freeze_dir.to_str().unwrap(),
            "--max-files",
            "0",
            "--freeze-before-date",
            "2026-12-31T00:00:00Z",
            "--fail-on-freeze",
        ])
        .assert()
        .code(3);

    // Constraint conflict: 3 valid files, min-keep 3, max-files 3? No, max-size 5B with min-keep 3 -> exit 3 with --strict-constraints.
    let constrained_dir = tmp.path().join("constrained_vault");
    fs::create_dir_all(&constrained_dir).unwrap();
    for i in 0..3 {
        let p = constrained_dir.join(format!("b_{i}.sql"));
        fs::write(&p, b"0123456789").unwrap();
        set_mtime_unix(&p, now_s - 600 - i * 60);
    }
    cli_bin()
        .args([
            "run",
            "--dir",
            constrained_dir.to_str().unwrap(),
            "--min-keep",
            "3",
            "--max-size",
            "10B",
            "--strict-constraints",
        ])
        .assert()
        .code(3);

    // Operational error (non-existent --dir) returns exit code 1.
    cli_bin()
        .args([
            "run",
            "--dir",
            tmp.path().join("missing_dir").to_str().unwrap(),
        ])
        .assert()
        .code(1);

    // CLI usage error (neither --config nor --dir) returns exit code 2.
    cli_bin().args(["run"]).assert().code(2);
}

#[test]
fn cli_check_in_use_exits_0_when_safe_and_10_when_busy() {
    let tmp = TempDir::new().unwrap();
    let safe_file = tmp.path().join("idle.sql");
    let sidecar_target = tmp.path().join("locked.sql");
    let sidecar_marker = tmp.path().join("locked.sql.customlock");
    let recent_file = tmp.path().join("recent.sql");

    for p in [&safe_file, &sidecar_target, &sidecar_marker, &recent_file] {
        fs::write(p, b"payload").unwrap();
    }
    let old_s = Utc::now().timestamp() - 3600;
    set_mtime_unix(&safe_file, old_s);
    set_mtime_unix(&sidecar_target, old_s);

    // Safe single path -> exit 0.
    let safe_json = cli_bin()
        .args([
            "--json",
            "check-in-use",
            safe_file.to_str().unwrap(),
            "--idle-age",
            "30s",
            "--stability",
            "20ms",
            "--no-proc-fd",
            "--no-flock",
            "--no-windows-share",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let safe_parsed: serde_json::Value = serde_json::from_slice(&safe_json).unwrap();
    assert_eq!(safe_parsed["all_safe"], true);

    // Busy via custom sidecar suffix -> exit 10.
    cli_bin()
        .args([
            "check-in-use",
            sidecar_target.to_str().unwrap(),
            "--sidecar-ext",
            ".customlock",
            "--no-proc-fd",
            "--no-flock",
        ])
        .assert()
        .code(10)
        .stdout(predicate::str::contains("BUSY:"));

    // Mixed paths (safe + too-recent) -> exit 10 with busy_count == 1.
    let mixed_json = cli_bin()
        .args([
            "--json",
            "check-in-use",
            safe_file.to_str().unwrap(),
            recent_file.to_str().unwrap(),
            "--idle-age",
            "5m",
            "--no-proc-fd",
        ])
        .assert()
        .code(10)
        .get_output()
        .stdout
        .clone();
    let mixed_parsed: serde_json::Value = serde_json::from_slice(&mixed_json).unwrap();
    assert_eq!(mixed_parsed["all_safe"], false);
    assert_eq!(mixed_parsed["busy_count"], 1);
}

#[test]
fn cli_daemon_subprocess_lifecycle_notify_status_duplicate_pid_stop_and_templates() {
    let tmp = TempDir::new().unwrap();
    let watch_dir = tmp.path().join("daemon_watch");
    let pid_file = tmp.path().join("daemon.pid");
    let state_file = tmp.path().join("daemon.state.json");
    let stop_file = tmp.path().join("daemon.stop");
    let audit_file = tmp.path().join("daemon.audit.jsonl");
    fs::create_dir_all(&watch_dir).unwrap();

    let cfg_path = tmp.path().join("daemon.toml");
    fs::write(
        &cfg_path,
        format!(
            r#"
[daemon]
mode = "notify"
interval = "30s"
watch = true
debounce = "40ms"
sweep_on_start = false
pid_file = "{}"
state_file = "{}"
stop_file = "{}"

[audit]
log_path = "{}"
jsonl = true

[[rules]]
name = "watch-spool"
path = "{}"
max_files = 1
actions = ["delete"]

[rules.in_use]
check_os_locks = false
check_proc_fd = false
"#,
            write_toml_escape(&pid_file),
            write_toml_escape(&state_file),
            write_toml_escape(&stop_file),
            write_toml_escape(&audit_file),
            write_toml_escape(&watch_dir),
        ),
    )
    .unwrap();

    // 1. Bounded `daemon run --max-sweeps 1 --mode interval` via assert_cmd (fast 50ms interval).
    let interval_cfg_path = tmp.path().join("daemon_interval.toml");
    fs::write(
        &interval_cfg_path,
        format!(
            r#"
[daemon]
mode = "interval"
interval = "50ms"
watch = false
sweep_on_start = true
pid_file = "{}"
state_file = "{}"
stop_file = "{}"

[[rules]]
name = "watch-spool"
path = "{}"
max_files = 1
actions = ["delete"]

[rules.in_use]
check_os_locks = false
check_proc_fd = false
"#,
            write_toml_escape(&pid_file),
            write_toml_escape(&state_file),
            write_toml_escape(&stop_file),
            write_toml_escape(&watch_dir),
        ),
    )
    .unwrap();

    let f_seed = watch_dir.join("seed_old.dat");
    let f_keep = watch_dir.join("seed_keep.dat");
    fs::write(&f_seed, b"old-bytes").unwrap();
    fs::write(&f_keep, b"new-bytes").unwrap();
    set_mtime_unix(&f_seed, Utc::now().timestamp() - 600);

    cli_bin()
        .args([
            "--json",
            "daemon",
            "run",
            "--config",
            interval_cfg_path.to_str().unwrap(),
            "--mode",
            "interval",
            "--max-sweeps",
            "1",
        ])
        .assert()
        .success();
    assert!(!f_seed.exists());
    assert!(f_keep.exists());
    // Age `f_keep` before starting the Notify daemon so writing `new_arrival.dat`
    // triggers a race-free deterministic sweep.
    set_mtime_unix(&f_keep, Utc::now().timestamp() - 300);

    // 2. Spawn live foreground daemon in Notify mode.
    let bin_path = assert_cmd::cargo::cargo_bin("filecleaner");
    let mut child = StdCommand::new(bin_path)
        .args(["daemon", "run", "--config", cfg_path.to_str().unwrap()])
        .spawn()
        .unwrap();

    let wait_idle_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(raw) = fs::read_to_string(&state_file) {
            if let Ok(st) = serde_json::from_str::<DaemonState>(&raw) {
                if st.status == DaemonStatus::Idle && pid_file.exists() {
                    break;
                }
            }
        }
        assert!(
            Instant::now() < wait_idle_deadline,
            "daemon subprocess did not reach Idle"
        );
        thread::sleep(Duration::from_millis(25));
    }

    // 3. `daemon status --json` exits 0 when running.
    let status_out = cli_bin()
        .args([
            "--json",
            "daemon",
            "status",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let status_report: DaemonStatusReport = serde_json::from_slice(&status_out).unwrap();
    assert!(status_report.running);
    assert_eq!(status_report.pid, Some(child.id()));

    // 4. Starting a duplicate daemon on the same pid_file fails with exit code 4.
    cli_bin()
        .args([
            "daemon",
            "run",
            "--config",
            cfg_path.to_str().unwrap(),
            "--max-sweeps",
            "1",
        ])
        .assert()
        .code(4);

    // 5. Trigger Notify sweep by writing a new file (`seed_keep.dat` is older than `new_arrival.dat`).
    let new_arrival = watch_dir.join("new_arrival.dat");
    fs::write(&new_arrival, b"fresh").unwrap();

    let wait_sweep_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(raw) = fs::read_to_string(&state_file) {
            if let Ok(st) = serde_json::from_str::<DaemonState>(&raw) {
                if st.sweeps_completed >= 1 && st.status == DaemonStatus::Idle {
                    break;
                }
            }
        }
        assert!(
            Instant::now() < wait_sweep_deadline,
            "notify sweep did not complete in subprocess"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(!f_keep.exists());
    assert!(new_arrival.exists());

    // 6. `daemon stop` stops the daemon cleanly and waits for exit (`0`).
    cli_bin()
        .args([
            "--json",
            "daemon",
            "stop",
            "--config",
            cfg_path.to_str().unwrap(),
            "--timeout",
            "10s",
        ])
        .assert()
        .code(0);

    let child_exit = child.wait().unwrap();
    assert!(child_exit.success());
    assert!(!pid_file.exists());
    assert!(!stop_file.exists());

    // 7. `daemon status` after stop reports stopped with exit code 4.
    cli_bin()
        .args(["daemon", "status", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("daemon stopped"));

    // 8. `daemon service-template` generates systemd, launchd, and windows-task definitions.
    let systemd_out = tmp.path().join("filecleaner.service");
    cli_bin()
        .args([
            "daemon",
            "service-template",
            "--init-system",
            "systemd",
            "--config",
            "/etc/filecleaner/prod.toml",
            "--user",
            "filecleaner",
            "--output",
            systemd_out.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(fs::read_to_string(&systemd_out)
        .unwrap()
        .contains("User=filecleaner"));

    cli_bin()
        .args([
            "daemon",
            "service-template",
            "--init-system",
            "launchd",
            "--config",
            "/etc/filecleaner/prod.toml",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("com.filecleaner.daemon"));

    cli_bin()
        .args([
            "daemon",
            "service-template",
            "--init-system",
            "windows-task",
            "--config",
            "C:\\ProgramData\\FileCleaner\\config.toml",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("<Task version=\"1.4\""));
}

#[test]
fn engine_mid_sweep_shutdown_completes_active_file_and_cancels_remaining_queue() {
    struct ShutdownTriggerHook {
        calls: AtomicUsize,
        shutdown: Arc<AtomicBool>,
    }

    impl HookRunner for ShutdownTriggerHook {
        fn run(
            &self,
            _candidate: &HookCandidate,
            _hook: &HookConfig,
            _dry_run: bool,
        ) -> filecleaner::Result<HookVerdict> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Request shutdown while file #1 is already executing its pipeline.
            self.shutdown.store(true, Ordering::Release);
            Ok(HookVerdict::Proceed)
        }
    }

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("vault");
    let dest = tmp.path().join("archive");
    fs::create_dir_all(&dir).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let f1_oldest = dir.join("1_oldest.sql");
    let f2_second = dir.join("2_second.sql");
    let f3_keep = dir.join("3_newest.sql");

    for (p, age) in [(&f1_oldest, 300), (&f2_second, 200), (&f3_keep, 100)] {
        fs::write(p, b"important-backup-data").unwrap();
        set_mtime_unix(p, now.timestamp() - age);
    }

    let cfg = Config {
        dry_run: false,
        audit_log: None,
        daemon: DaemonConfig::default(),
        audit: AuditConfig::default(),
        global_trash_dir: None,
        rules: vec![RuleConfig {
            name: "vault".to_string(),
            path: dir.clone(),
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
            max_files: Some(1),
            max_age: None,
            max_total_size: None,
            combine_mode: CombineMode::Any,
            eviction_order: EvictionOrder::OldestFirst,
            dry_run: false,
            backup_guard: BackupGuardConfig::default(),
            in_use: InUseConfig {
                check_os_locks: false,
                check_proc_fd: false,
                ..InUseConfig::default()
            },
            pre_delete_hook: Some(HookConfig {
                command: "mock".to_string(),
                args: vec![],
                timeout: Duration::from_secs(5),
                on_failure: HookFailurePolicy::AbortFile,
                skip_exit_code: Some(42),
                run_in_dry_run: false,
                working_dir: None,
                env: BTreeMap::new(),
            }),
            actions: vec![ActionStep::Compress, ActionStep::Move],
            move_to: Some(dest.clone()),
            move_date_subdir: None,
            move_overwrite: false,
            trash_dir: None,
            compression: CompressionFormat::Gzip,
            compression_level: None,
            compress_remove_source: true,
            compress_overwrite: false,
            copy_truncate_to: None,
            rename_template: None,
        }],
    };

    let shutdown = Arc::new(AtomicBool::new(false));
    let hook = ShutdownTriggerHook {
        calls: AtomicUsize::new(0),
        shutdown: Arc::clone(&shutdown),
    };

    let report = SweepEngine::run_config(
        &cfg,
        &RunOptions {
            shutdown: Some(shutdown),
            ..RunOptions::default()
        },
        &FixedClock(now),
        &OsLockProbe,
        &hook,
    )
    .unwrap();

    // Active candidate #1 finished its full Compress -> Move pipeline; candidate #2 was queued & skipped cleanly.
    assert!(report.interrupted);
    assert_eq!(hook.calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.rule_reports[0].acted_files, 1);
    assert!(!f1_oldest.exists());
    assert!(dest.join("1_oldest.sql.gz").exists());
    assert!(f2_second.exists());
    assert!(f3_keep.exists());

    // Zero `.fc-tmp` artifacts anywhere.
    for entry in fs::read_dir(&dir)
        .unwrap()
        .chain(fs::read_dir(&dest).unwrap())
    {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.contains(".fc-tmp"));
    }
}

#[test]
fn daemon_multi_sweep_in_place_rename_and_compress_are_idempotent() {
    let tmp = TempDir::new().unwrap();
    let logs_dir = tmp.path().join("logs");
    let dumps_dir = tmp.path().join("dumps");
    fs::create_dir_all(&logs_dir).unwrap();
    fs::create_dir_all(&dumps_dir).unwrap();

    let log_file = logs_dir.join("service.log");
    let dump_file = dumps_dir.join("nightly.sql");
    fs::write(&log_file, b"line-1\nline-2\n").unwrap();
    fs::write(&dump_file, b"CREATE TABLE t(id INT);\n".repeat(16)).unwrap();

    let old_ts = Utc::now().timestamp() - 3600;
    set_mtime_unix(&log_file, old_ts);
    set_mtime_unix(&dump_file, old_ts);

    let rename_rule = RuleConfig {
        name: "rename-logs".to_string(),
        path: logs_dir.clone(),
        enabled: true,
        recursive: false,
        max_depth: None,
        follow_symlinks: false,
        include_globs: vec!["*.log".to_string()],
        exclude_globs: vec![],
        include_regex: None,
        exclude_regex: None,
        min_file_size: None,
        max_file_size: None,
        time_basis: TimeBasis::Modified,
        filename_date_regex: None,
        filename_date_format: None,
        max_files: None,
        max_age: Some(Duration::from_secs(10)),
        max_total_size: None,
        combine_mode: CombineMode::Any,
        eviction_order: EvictionOrder::OldestFirst,
        dry_run: false,
        backup_guard: BackupGuardConfig::default(),
        in_use: InUseConfig {
            check_os_locks: false,
            check_proc_fd: false,
            ..InUseConfig::default()
        },
        pre_delete_hook: None,
        actions: vec![ActionStep::RenameByDate],
        move_to: None,
        move_date_subdir: None,
        move_overwrite: false,
        trash_dir: None,
        compression: CompressionFormat::Gzip,
        compression_level: None,
        compress_remove_source: true,
        compress_overwrite: false,
        copy_truncate_to: None,
        rename_template: Some("%Y%m%d_%H%M%S".to_string()),
    };

    let mut compress_rule = rename_rule.clone();
    compress_rule.name = "compress-dumps".to_string();
    compress_rule.path = dumps_dir.clone();
    compress_rule.include_globs = vec![];
    compress_rule.actions = vec![ActionStep::Compress];
    compress_rule.rename_template = None;

    let cfg = Config {
        dry_run: false,
        audit_log: None,
        daemon: DaemonConfig {
            mode: DaemonMode::Interval,
            interval: Duration::from_millis(20),
            watch: false,
            debounce: Duration::from_millis(10),
            sweep_on_start: true,
            pid_file: Some(tmp.path().join("multi.pid")),
            state_file: Some(tmp.path().join("multi.state.json")),
            stop_file: Some(tmp.path().join("multi.stop")),
        },
        audit: AuditConfig::default(),
        global_trash_dir: None,
        rules: vec![rename_rule, compress_rule],
    };

    let final_state = DaemonRunner::run(
        &cfg,
        &DaemonOptions {
            max_sweeps: Some(3),
            ..DaemonOptions::default()
        },
    )
    .unwrap();

    assert_eq!(final_state.sweeps_completed, 3);
    assert_eq!(final_state.total_files_processed, 2);

    let log_entries: Vec<String> = fs::read_dir(&logs_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(log_entries.len(), 1);
    assert!(log_entries[0].starts_with("service_") && log_entries[0].ends_with(".log"));
    assert_eq!(
        log_entries[0].matches('_').count(),
        2,
        "unexpected repeated date stamp in {}",
        log_entries[0]
    );

    let dump_entries: Vec<String> = fs::read_dir(&dumps_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(dump_entries, vec!["nightly.sql.gz".to_string()]);
}

#[test]
fn cli_daemon_stop_refuses_foreign_non_filecleaner_pid_and_leaves_process_alive() {
    let tmp = TempDir::new().unwrap();
    let pid_file = tmp.path().join("foreign.pid");
    let stop_file = tmp.path().join("foreign.stop");

    #[cfg(unix)]
    let mut foreign = StdCommand::new("sleep").arg("5").spawn().unwrap();
    #[cfg(windows)]
    let mut foreign = StdCommand::new("ping")
        .args(["-n", "6", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    fs::write(&pid_file, format!("{}\n", foreign.id())).unwrap();

    cli_bin()
        .args([
            "daemon",
            "stop",
            "--pid-file",
            pid_file.to_str().unwrap(),
            "--stop-file",
            stop_file.to_str().unwrap(),
            "--timeout",
            "2s",
        ])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("not filecleaner"));

    assert!(!stop_file.exists());
    assert!(foreign.try_wait().unwrap().is_none());

    let _ = foreign.kill();
    let _ = foreign.wait();
}

#[test]
fn cli_check_in_use_batches_multiple_paths_with_single_stability_sleep() {
    let tmp = TempDir::new().unwrap();
    let old_s = Utc::now().timestamp() - 3600;
    let mut paths = Vec::new();
    for i in 0..4 {
        let p = tmp.path().join(format!("batch_{i}.sql"));
        fs::write(&p, b"stable-payload").unwrap();
        set_mtime_unix(&p, old_s);
        paths.push(p);
    }

    let mut args = vec![
        "--json".to_string(),
        "check-in-use".to_string(),
        "--stability".to_string(),
        "150ms".to_string(),
        "--no-proc-fd".to_string(),
        "--no-flock".to_string(),
        "--no-windows-share".to_string(),
    ];
    for p in &paths {
        args.push(p.to_str().unwrap().to_string());
    }

    let started = Instant::now();
    let output = cli_bin()
        .args(&args)
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(1500),
        "batched check-in-use took {elapsed:?}"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(parsed["all_safe"], true);
    assert_eq!(parsed["busy_count"], 0);
    assert_eq!(parsed["reports"].as_array().unwrap().len(), 4);
}
