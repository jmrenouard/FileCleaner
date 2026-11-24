//! End-to-end executable verification of FileCleaner tutorials, how-to procedures, and CLI flags.

use std::fs::{self, File};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn filecleaner_bin() -> &'static str {
    env!("CARGO_BIN_EXE_filecleaner")
}

fn set_age_days(path: &Path, days: u64) {
    let ts = SystemTime::now() - Duration::from_secs(days * 86_400);
    let file = File::options()
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    file.set_modified(ts)
        .unwrap_or_else(|e| panic!("set_modified {}: {e}", path.display()));
}

#[test]
fn tutorial_1_local_retention_sandbox_cli_workflow() {
    let tmp = TempDir::new().expect("create temp sandbox");
    let backups = tmp.path().join("backups");
    let archive = tmp.path().join("archive");
    let trash = tmp.path().join("trash");
    let audit = tmp.path().join("audit.jsonl");
    fs::create_dir_all(&backups).expect("mkdir backups");
    fs::create_dir_all(&archive).expect("mkdir archive");
    fs::create_dir_all(&trash).expect("mkdir trash");

    for day in 1..=5u64 {
        let p = backups.join(format!("app_day{day}.dump"));
        fs::write(&p, vec![b'A'; 2048]).expect("write dump");
        set_age_days(&p, day);
    }

    let make_toml = |dry_run: bool| {
        format!(
            r#"
dry_run = {dry_run}
global_trash_dir = "{trash}"

[audit]
log_path = "{audit}"
jsonl = true

[[rules]]
name = "sandbox-retention"
path = "{backups}"
enabled = true
include_globs = ["*.dump"]
min_file_size = "1KiB"
time_basis = "modified"
max_files = 3
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
move_to = "{archive}"
"#,
            dry_run = dry_run,
            trash = trash.to_string_lossy().replace('\\', "/"),
            audit = audit.to_string_lossy().replace('\\', "/"),
            backups = backups.to_string_lossy().replace('\\', "/"),
            archive = archive.to_string_lossy().replace('\\', "/"),
        )
    };

    let cfg = tmp.path().join("policy.toml");
    fs::write(&cfg, make_toml(true)).expect("write dry_run config");

    let val_out = Command::new(filecleaner_bin())
        .args(["validate", "-c"])
        .arg(&cfg)
        .output()
        .expect("run validate");
    assert!(
        val_out.status.success(),
        "validate failed: {}",
        String::from_utf8_lossy(&val_out.stderr)
    );

    let plan_out = Command::new(filecleaner_bin())
        .args(["plan", "-c"])
        .arg(&cfg)
        .args(["--verbose", "--json"])
        .output()
        .expect("run plan");
    assert!(
        plan_out.status.success(),
        "plan failed: {}",
        String::from_utf8_lossy(&plan_out.stderr)
    );
    let _plan_json: serde_json::Value =
        serde_json::from_slice(&plan_out.stdout).expect("valid JSON from plan");
    assert_eq!(fs::read_dir(&backups).unwrap().count(), 5);
    assert_eq!(fs::read_dir(&archive).unwrap().count(), 0);

    fs::write(&cfg, make_toml(false)).expect("write live config");
    let run_out = Command::new(filecleaner_bin())
        .args(["run", "-c"])
        .arg(&cfg)
        .arg("--json")
        .output()
        .expect("run live sweep");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    let mut remaining: Vec<String> = fs::read_dir(&backups)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();
    assert_eq!(
        remaining,
        vec!["app_day1.dump", "app_day2.dump", "app_day3.dump"]
    );

    let mut archived: Vec<String> = fs::read_dir(&archive)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    archived.sort();
    assert_eq!(archived, vec!["app_day4.dump.gz", "app_day5.dump.gz"]);

    let audit_text = fs::read_to_string(&audit).expect("read audit JSONL");
    assert!(
        !audit_text.trim().is_empty(),
        "expected audit JSONL records"
    );
    for line in audit_text.lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).expect("audit line must be valid JSON");
    }
}

#[test]
fn tutorial_2_backup_guard_freeze_and_check_in_use_cli_workflow() {
    let tmp = TempDir::new().expect("create guard lab");
    let backups = tmp.path().join("pg-backups");
    let trash = tmp.path().join("trash");
    fs::create_dir_all(&backups).expect("mkdir pg-backups");
    fs::create_dir_all(&trash).expect("mkdir trash");

    let valid_dump = backups.join("pg_valid_01.dump");
    let corrupt_stub = backups.join("pg_corrupt_02.dump");
    fs::write(&valid_dump, vec![b'B'; 2 * 1024 * 1024]).expect("write 2MiB valid dump");
    fs::write(&corrupt_stub, vec![b'C'; 512]).expect("write 512B stub");
    set_age_days(&valid_dump, 10);
    set_age_days(&corrupt_stub, 20);

    let cfg = tmp.path().join("guard.toml");
    fs::write(
        &cfg,
        format!(
            r#"
dry_run = false
global_trash_dir = "{trash}"

[[rules]]
name = "pg-base"
path = "{backups}"
enabled = true
include_globs = ["*.dump"]
time_basis = "modified"
max_files = 3
max_age = "5d"
combine_mode = "any"
actions = ["delete"]

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "1MiB"
require_newer_within = "24h"
freeze_on_empty_valid = true
"#,
            trash = trash.to_string_lossy().replace('\\', "/"),
            backups = backups.to_string_lossy().replace('\\', "/"),
        ),
    )
    .expect("write guard config");

    for subcmd in ["plan", "run"] {
        let out = Command::new(filecleaner_bin())
            .args([subcmd, "-c"])
            .arg(&cfg)
            .arg("--fail-on-freeze")
            .output()
            .expect("run backup guard check");
        assert_eq!(
            out.status.code(),
            Some(3),
            "{subcmd} --fail-on-freeze must exit with 3, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(valid_dump.is_file(), "valid dump must remain preserved");
    assert!(corrupt_stub.is_file(), "freeze must prevent any eviction");

    let live_dump = backups.join("pg_live.dump");
    let part_sidecar = backups.join("pg_live.dump.part");
    fs::write(&live_dump, vec![b'L'; 4096]).expect("write live dump");
    fs::write(&part_sidecar, b"").expect("create .part sidecar");

    let busy_out = Command::new(filecleaner_bin())
        .args(["check-in-use", "--idle-age", "10s", "--stability", "200ms"])
        .arg(&live_dump)
        .output()
        .expect("run check-in-use busy");
    assert_eq!(
        busy_out.status.code(),
        Some(10),
        "check-in-use with .part sidecar must exit 10"
    );

    fs::remove_file(&part_sidecar).expect("remove sidecar");
    set_age_days(&live_dump, 1);

    let free_out = Command::new(filecleaner_bin())
        .args([
            "check-in-use",
            "--idle-age",
            "5s",
            "--stability",
            "100ms",
            "--json",
        ])
        .arg(&live_dump)
        .output()
        .expect("run check-in-use idle");
    assert_eq!(
        free_out.status.code(),
        Some(0),
        "settled file check-in-use must exit 0, stderr: {}",
        String::from_utf8_lossy(&free_out.stderr)
    );
    let _: serde_json::Value =
        serde_json::from_slice(&free_out.stdout).expect("check-in-use JSON output");
}

#[test]
fn howto_init_validate_and_service_template_cli_workflow() {
    let tmp = TempDir::new().expect("create init temp");

    for (profile, fmt, fname) in [
        ("backups", "toml", "backups.toml"),
        ("dba-postgres", "toml", "dba.toml"),
        ("logs", "yaml", "logs.yaml"),
    ] {
        let out_cfg = tmp.path().join(fname);
        let init_out = Command::new(filecleaner_bin())
            .args(["init", "-o"])
            .arg(&out_cfg)
            .args(["--profile", profile, "--format", fmt])
            .output()
            .expect("run init");
        assert!(
            init_out.status.success(),
            "init {profile} failed: {}",
            String::from_utf8_lossy(&init_out.stderr)
        );

        let val_out = Command::new(filecleaner_bin())
            .args(["validate", "-c"])
            .arg(&out_cfg)
            .output()
            .expect("validate init config");
        assert!(
            val_out.status.success(),
            "validate {profile} failed: {}",
            String::from_utf8_lossy(&val_out.stderr)
        );
    }

    let tpl_out = Command::new(filecleaner_bin())
        .args([
            "daemon",
            "service-template",
            "--init-system",
            "systemd",
            "--config",
            &tmp.path().join("backups.toml").to_string_lossy(),
            "--user",
            "filecleaner",
        ])
        .output()
        .expect("generate systemd service template");
    assert!(
        tpl_out.status.success(),
        "service-template failed: {}",
        String::from_utf8_lossy(&tpl_out.stderr)
    );
    let tpl_text = String::from_utf8_lossy(&tpl_out.stdout);
    assert!(tpl_text.contains("[Unit]"));
    assert!(tpl_text.contains("[Service]"));
    assert!(tpl_text.contains("User=filecleaner"));
}

fn write_skip_42_hook(dir: &Path) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("hook-veto-42.sh");
        fs::write(&p, "#!/bin/sh\nexit 42\n").expect("write unix veto hook");
        let mut perms = fs::metadata(&p).expect("hook metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).expect("chmod +x hook");
        p
    }
    #[cfg(windows)]
    {
        let p = dir.join("hook-veto-42.cmd");
        fs::write(&p, "@exit /b 42\r\n").expect("write windows veto hook");
        p
    }
}

#[test]
fn howto_pre_delete_hook_skip_veto_and_version_cli() {
    let ver_out = Command::new(filecleaner_bin())
        .arg("--version")
        .output()
        .expect("run --version");
    assert!(ver_out.status.success());
    assert!(String::from_utf8_lossy(&ver_out.stdout).contains("filecleaner"));

    let tmp = TempDir::new().expect("create ad-hoc sandbox");
    let target = tmp.path().join("spool");
    let archive = tmp.path().join("archive");
    fs::create_dir_all(&target).expect("mkdir spool");
    fs::create_dir_all(&archive).expect("mkdir archive");
    let sample = target.join("old.dat");
    fs::write(&sample, b"hello").expect("write old.dat");
    set_age_days(&sample, 10);

    let adhoc_plan = Command::new(filecleaner_bin())
        .args(["plan", "--dir"])
        .arg(&target)
        .args([
            "--include",
            "*.dat",
            "--max-age",
            "5d",
            "--compress",
            "gzip",
            "--move-to",
        ])
        .arg(&archive)
        .arg("--json")
        .output()
        .expect("run ad-hoc plan");
    assert!(
        adhoc_plan.status.success(),
        "adhoc plan failed: {}",
        String::from_utf8_lossy(&adhoc_plan.stderr)
    );
    let _: serde_json::Value = serde_json::from_slice(&adhoc_plan.stdout).expect("adhoc plan JSON");
    assert!(sample.is_file(), "plan must never mutate files");

    let hook = write_skip_42_hook(tmp.path());
    let veto_run = Command::new(filecleaner_bin())
        .args(["run", "--dir"])
        .arg(&target)
        .args(["--include", "*.dat", "--max-age", "5d", "--pre-hook"])
        .arg(&hook)
        .arg("--json")
        .output()
        .expect("run ad-hoc sweep with exit-42 hook");
    assert_eq!(
        veto_run.status.code(),
        Some(0),
        "exit-42 skip veto must return exit 0, stderr: {}",
        String::from_utf8_lossy(&veto_run.stderr)
    );
    let veto_json: serde_json::Value =
        serde_json::from_slice(&veto_run.stdout).expect("valid JSON from hook run");
    let hook_skips = veto_json["rule_reports"][0]["skipped_hook"]
        .as_u64()
        .or_else(|| veto_json["summary"]["rules"][0]["skipped_hook"].as_u64())
        .unwrap_or_else(|| panic!("missing skipped_hook count in JSON: {veto_json}"));
    assert_eq!(hook_skips, 1, "expected 1 hook veto skip in {veto_json}");
    assert!(
        sample.is_file(),
        "exit-42 pre-delete hook must preserve candidate"
    );
}
