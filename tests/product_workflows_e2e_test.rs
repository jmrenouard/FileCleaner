//! End-to-end product workflow tests matching the FileCleaner 5-minute HOWTO journeys:
//! zero-config ad-hoc cleanup, DB backup dead-man's-switch & floor protection,
//! log rotation (RenameByDate -> Compress -> Move vs CopyTruncate), and ETL spool
//! in-use + pre-delete hook exit-42 veto to trash quarantine.

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{TimeZone, Utc};
use filecleaner::SweepReport;
use flate2::read::GzDecoder;
use lz4_flex::frame::FrameDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn cli_bin() -> Command {
    Command::cargo_bin("filecleaner").unwrap()
}

fn toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn write_stamped(path: &Path, content: &[u8], unix_secs: i64) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
    let f = OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs as u64))
        .unwrap();
}

#[test]
fn howto_workflow_1_zero_config_adhoc_plan_and_clean_by_count_age_and_size() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("downloads");
    let quarantine = tmp.path().join("quarantine");
    fs::create_dir_all(&dir).unwrap();

    let now_s = Utc::now().timestamp();
    let keep_new = dir.join("recent.bin");
    let old_big = dir.join("old_large.bin");
    let old_mid = dir.join("old_mid.bin");

    write_stamped(&keep_new, &[b'a'; 64], now_s - 60);
    write_stamped(&old_mid, &[b'b'; 200], now_s - 7200);
    write_stamped(&old_big, &[b'c'; 200], now_s - 10800);

    // 1. Preview safely with `plan --json`: nothing modified or created.
    let plan_bytes = cli_bin()
        .args([
            "plan",
            "--dir",
            dir.to_str().unwrap(),
            "--include",
            "*.bin",
            "--max-files",
            "1",
            "--trash-dir",
            quarantine.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plan: SweepReport = serde_json::from_slice(&plan_bytes).unwrap();
    assert!(plan.dry_run);
    assert_eq!(plan.summary.total_acted, 2);
    assert!(old_big.exists() && old_mid.exists() && !quarantine.exists());

    // 2. Run live cleanup moving evicted files to quarantine trash.
    cli_bin()
        .args([
            "run",
            "--dir",
            dir.to_str().unwrap(),
            "--include",
            "*.bin",
            "--max-files",
            "1",
            "--trash-dir",
            quarantine.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(keep_new.exists());
    assert!(!old_mid.exists() && !old_big.exists());
    assert!(quarantine.join("old_mid.bin").exists());
    assert!(quarantine.join("old_large.bin").exists());
}

#[test]
fn howto_workflow_2_database_backups_min_keep_corrupt_stub_filter_and_staleness_freeze() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("pg_backups");
    let cold = tmp.path().join("cold_nas");
    fs::create_dir_all(&vault).unwrap();

    // Scenario A: stale vault (newest valid backup > 48h old, plus a 0-byte failed stub today).
    let today_fmt = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let broken_today = vault.join(format!("pg_{today_fmt}.sql"));
    fs::write(&broken_today, b"").unwrap(); // 0-byte failed cron dump

    let old_1 = vault.join("pg_20250101_020000.sql");
    let old_2 = vault.join("pg_20250102_020000.sql");
    let old_3 = vault.join("pg_20250103_020000.sql");
    for p in [&old_1, &old_2, &old_3] {
        fs::write(p, vec![b'S'; 256]).unwrap();
    }

    let cfg_path = tmp.path().join("pg-backups.toml");
    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run = false

[[rules]]
name = "postgres-daily"
path = "{}"
include_globs = ["pg_*.sql"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{{8}}_\d{{6}})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 2
actions = ["compress", "move"]
compression = "gzip"
move_to = "{}"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "100B"
require_newer_within = "48h"
freeze_on_empty_valid = true

[rules.in_use]
check_os_locks = false
check_proc_fd = false
"#,
            toml_path(&vault),
            toml_path(&cold),
        ),
    )
    .unwrap();

    // Freeze triggers exit 3 under `--fail-on-freeze`; zero files evicted despite max_files=2.
    cli_bin()
        .args([
            "run",
            "--config",
            cfg_path.to_str().unwrap(),
            "--fail-on-freeze",
        ])
        .assert()
        .code(3);
    assert!(old_1.exists() && old_2.exists() && old_3.exists() && !cold.exists());

    // Scenario B: once a fresh valid backup lands within 48h, the oldest excess dump compresses & moves.
    let fresh_valid = vault.join(format!("pg_{today_fmt}_valid.sql"));
    fs::write(vault.join(format!("pg_{today_fmt}.sql")), vec![b'V'; 512]).unwrap();
    drop(fresh_valid);

    cli_bin()
        .args([
            "run",
            "--config",
            cfg_path.to_str().unwrap(),
            "--fail-on-freeze",
        ])
        .assert()
        .code(0);

    // Keeps newest 2 valid backups locally (`min_keep_files = 2`: today + 20250103), archives older 2.
    assert!(vault.join(format!("pg_{today_fmt}.sql")).exists());
    assert!(old_3.exists());
    assert!(!old_1.exists() && !old_2.exists());

    let mut archived = Vec::new();
    GzDecoder::new(File::open(cold.join("pg_20250101_020000.sql.gz")).unwrap())
        .read_to_end(&mut archived)
        .unwrap();
    assert_eq!(archived.len(), 256);
}

#[test]
fn howto_workflow_3_log_rotation_rename_compress_move_and_copy_truncate() {
    let tmp = TempDir::new().unwrap();
    let app_logs = tmp.path().join("app_logs");
    let app_rotated = tmp.path().join("app_rotated");
    let reopened_logs = tmp.path().join("nginx_logs");
    let cold_logs = tmp.path().join("nginx_cold");

    let old_s = Utc
        .with_ymd_and_hms(2025, 6, 15, 14, 30, 0)
        .unwrap()
        .timestamp();
    let live_log = app_logs.join("app.log");
    let nginx_old = reopened_logs.join("access.log");
    write_stamped(&live_log, b"2025-06-15 app started\n", old_s);
    write_stamped(&nginx_old, b"GET /healthz 200\n", old_s);

    let cfg_path = tmp.path().join("logs.yaml");
    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run: false

rules:
  - name: legacy-copy-truncate
    path: "{}"
    include_globs: ["*.log"]
    max_age: 1d
    in_use:
      enabled: true
      check_os_locks: false
      check_proc_fd: false
    actions: ["copy_truncate"]
    copy_truncate_to: "{}"
    compression: lz4

  - name: reopened-rename-compress-move
    path: "{}"
    include_globs: ["*.log"]
    max_age: 1d
    in_use:
      enabled: false
    actions: ["rename_by_date", "compress", "move"]
    rename_template: "%Y%m%d"
    compression: gzip
    move_to: "{}"
    move_date_subdir: "%Y/%m"
"#,
            toml_path(&app_logs),
            toml_path(&app_rotated),
            toml_path(&reopened_logs),
            toml_path(&cold_logs),
        ),
    )
    .unwrap();

    cli_bin()
        .args(["run", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .success();

    // CopyTruncate zeroes app.log in place and saves LZ4 archive.
    assert_eq!(fs::metadata(&live_log).unwrap().len(), 0);
    let lz4_archive = app_rotated.join("app_20250615_143000.log.lz4");
    assert!(lz4_archive.exists());
    let mut lz4_plain = Vec::new();
    FrameDecoder::new(File::open(&lz4_archive).unwrap())
        .read_to_end(&mut lz4_plain)
        .unwrap();
    assert_eq!(lz4_plain, b"2025-06-15 app started\n");

    // RenameByDate + Gzip + Move date subdir creates archive under 2025/06/.
    let moved_gz = cold_logs.join("2025/06/access_20250615.log.gz");
    assert!(!nginx_old.exists());
    assert!(moved_gz.exists());
}

#[test]
fn howto_workflow_4_incoming_spool_check_in_use_and_hook_veto_to_trash() {
    let tmp = TempDir::new().unwrap();
    let spool = tmp.path().join("spool");
    let trash = tmp.path().join("trash");
    fs::create_dir_all(&spool).unwrap();

    let now_s = Utc::now().timestamp();
    let active_part = spool.join("upload.csv");
    let active_marker = spool.join("upload.csv.part");
    let ready_no_receipt = spool.join("pending.csv");
    let ready_verified = spool.join("done.csv");

    write_stamped(&active_part, b"col1\n1\n", now_s - 600);
    fs::write(&active_marker, b"1").unwrap();
    write_stamped(&ready_no_receipt, b"col1\n2\n", now_s - 600);
    write_stamped(&ready_verified, b"col1\n3\n", now_s - 600);
    fs::write(spool.join("done.csv.ok"), b"ok").unwrap();

    // 1. check-in-use spots sidecar `.part` lock (`exit 10`), confirms settled file (`exit 0`).
    cli_bin()
        .args([
            "check-in-use",
            active_part.to_str().unwrap(),
            "--no-flock",
            "--no-proc-fd",
            "--no-windows-share",
        ])
        .assert()
        .code(10);
    cli_bin()
        .args([
            "check-in-use",
            ready_verified.to_str().unwrap(),
            "--no-flock",
            "--no-proc-fd",
            "--no-windows-share",
        ])
        .assert()
        .code(0);

    // 2. Hook returns exit 42 unless `<file>.ok` exists (cross-platform inline helper script).
    #[cfg(unix)]
    let (hook_cmd, hook_args) = {
        use std::os::unix::fs::PermissionsExt;
        let script = tmp.path().join("check_ok.sh");
        fs::write(
            &script,
            "#!/usr/bin/env sh\nif [ -f \"$FILECLEANER_PATH.ok\" ]; then exit 0; fi\nexit 42\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (script.to_string_lossy().into_owned(), Vec::<String>::new())
    };
    #[cfg(windows)]
    let (hook_cmd, hook_args) = {
        let script = tmp.path().join("check_ok.ps1");
        fs::write(
            &script,
            "if (Test-Path \"$env:FILECLEANER_PATH.ok\") { exit 0 }\nexit 42\n",
        )
        .unwrap();
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                script.to_string_lossy().into_owned(),
            ],
        )
    };

    let cfg_path = tmp.path().join("spool.toml");
    let args_toml = serde_json::to_string(&hook_args).unwrap();
    let hook_cmd_esc = toml_path(Path::new(&hook_cmd));

    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run = false
global_trash_dir = "{}"

[[rules]]
name = "etl-spool"
path = "{}"
include_globs = ["*.csv"]
max_age = "60s"
actions = ["trash"]

[rules.in_use]
enabled = true
check_os_locks = false
check_proc_fd = false

[rules.pre_delete_hook]
command = "{}"
args = {args_toml}
skip_exit_code = 42
"#,
            toml_path(&trash),
            toml_path(&spool),
            hook_cmd_esc,
        ),
    )
    .unwrap();

    cli_bin()
        .args(["run", "--config", cfg_path.to_str().unwrap()])
        .assert()
        .success();

    assert!(active_part.exists(), "sidecar-locked upload must stay");
    assert!(
        ready_no_receipt.exists(),
        "hook exit-42 veto must keep unacknowledged file"
    );
    assert!(!ready_verified.exists());
    assert!(trash.join("done.csv").exists());
}

struct DbaMockS3 {
    endpoint: String,
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for DbaMockS3 {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn start_dba_mock_s3() -> DbaMockS3 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let objects = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let objs = Arc::clone(&objects);
    let sd = Arc::clone(&shutdown);

    let handle = thread::spawn(move || {
        while !sd.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
                        continue;
                    }
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let target = parts.next().unwrap_or("/").to_string();
                    let mut content_len = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let t = line.trim_end_matches(['\r', '\n']);
                        if t.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = t.split_once(':') {
                            if k.eq_ignore_ascii_case("content-length") {
                                content_len = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                    let mut body = vec![0u8; content_len];
                    if content_len > 0 && reader.read_exact(&mut body).is_err() {
                        continue;
                    }
                    let key = target
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    let mut map = objs.lock().unwrap();
                    let resp = match method.as_str() {
                        "HEAD" => match map.get(&key) {
                            Some(data) => {
                                let b64 = BASE64_STANDARD.encode(Sha256::digest(data));
                                format!(
                                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nETag: \"dba\"\r\nx-amz-checksum-sha256: {b64}\r\n\r\n",
                                    data.len()
                                )
                            }
                            None => "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_string(),
                        },
                        "PUT" => {
                            map.insert(key, body);
                            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\nETag: \"dba\"\r\n\r\n".to_string()
                        }
                        _ => "HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_string(),
                    };
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    DbaMockS3 {
        endpoint: format!("http://{addr}"),
        objects,
        shutdown,
        handle: Some(handle),
    }
}

#[test]
fn howto_workflow_5_dba_postgres_wal_and_base_backups_s3_archival_and_pitr_guards() {
    let tmp = TempDir::new().unwrap();

    // 1. `filecleaner init --profile dba-postgres` generates valid TOML and YAML configs.
    let init_toml = tmp.path().join("dba.toml");
    let init_yaml = tmp.path().join("dba.yaml");
    cli_bin()
        .args([
            "init",
            "--profile",
            "dba-postgres",
            "--format",
            "toml",
            "--output",
            init_toml.to_str().unwrap(),
        ])
        .assert()
        .success();
    cli_bin()
        .args(["validate", init_toml.to_str().unwrap()])
        .assert()
        .success();

    cli_bin()
        .args([
            "init",
            "--profile",
            "dba-postgres",
            "--format",
            "yaml",
            "--output",
            init_yaml.to_str().unwrap(),
        ])
        .assert()
        .success();
    cli_bin()
        .args(["validate", init_yaml.to_str().unwrap()])
        .assert()
        .success();

    // 2. Build DBA base-backup + WAL archive layout.
    let base_dir = tmp.path().join("pg_base");
    let wal_dir = tmp.path().join("pg_wal");
    fs::create_dir_all(&base_dir).unwrap();
    fs::create_dir_all(wal_dir.join("archive_status")).unwrap();

    let now_s = Utc::now().timestamp();
    let today_fmt = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let broken_today = base_dir.join(format!("pg_{today_fmt}.sql"));
    fs::write(&broken_today, b"").unwrap(); // 0-byte failed dump stub

    let old_base_1 = base_dir.join("pg_20250101_020000.sql");
    let old_base_2 = base_dir.join("pg_20250102_020000.sql");
    write_stamped(
        &old_base_1,
        b"-- pg_dump base 1\n".repeat(32).as_slice(),
        now_s - 9000,
    );
    write_stamped(
        &old_base_2,
        b"-- pg_dump base 2\n".repeat(32).as_slice(),
        now_s - 8000,
    );

    // WAL spool: complete 24-hex segments + protected .history, .partial, and archive_status/*.ready.
    let history_file = wal_dir.join("00000002.history");
    let partial_file = wal_dir.join("000000010000000000000005.partial");
    let ready_file = wal_dir.join("archive_status/000000010000000000000001.ready");
    write_stamped(
        &history_file,
        b"1\t0/3000000\tno recovery target\n",
        now_s - 9000,
    );
    write_stamped(&partial_file, &[b'P'; 512], now_s - 9000);
    write_stamped(&ready_file, b"", now_s - 9000);

    let wal_names = [
        "000000010000000000000001",
        "000000010000000000000002",
        "000000010000000000000003",
        "000000010000000000000004",
    ];
    for (i, w) in wal_names.iter().enumerate() {
        let p = wal_dir.join(w);
        write_stamped(
            &p,
            format!("WAL-SEGMENT-{w}\n").repeat(64).as_bytes(),
            now_s - 5000 + (i as i64) * 100,
        );
    }

    // Hook vetoes WAL >= 000000010000000000000003 (`exit 42`).
    #[cfg(unix)]
    let (hook_cmd, hook_args) = {
        let repo_hook =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/hooks/verify_pg_wal_safe.sh");
        (
            "bash".to_string(),
            vec![repo_hook.to_string_lossy().into_owned()],
        )
    };
    #[cfg(windows)]
    let (hook_cmd, hook_args) = {
        let script = tmp.path().join("wal_veto.ps1");
        fs::write(
            &script,
            "if ($env:FILECLEANER_NAME -ge $env:FILECLEANER_PG_MIN_KEEP_WAL) { exit 42 }\nexit 0\n",
        )
        .unwrap();
        (
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                script.to_string_lossy().into_owned(),
            ],
        )
    };

    let mock_s3 = start_dba_mock_s3();
    let endpoint = mock_s3.endpoint.clone();
    let objects = Arc::clone(&mock_s3.objects);
    let cfg_path = tmp.path().join("dba_live.toml");
    let args_toml = serde_json::to_string(&hook_args).unwrap();
    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run = false

[s3]
bucket = "pg-dba-bkt"
region = "eu-west-3"
endpoint = "{endpoint}"
force_path_style = true
allow_insecure_http = true
access_key_env = "FC_DBA_AK"
secret_key_env = "FC_DBA_SK"
timeout = "5s"
max_retries = 1

[[rules]]
name = "pg-base-backups"
path = "{}"
include_globs = ["pg_*.sql"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{{8}}_\d{{6}})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 2
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"
[rules.s3]
bucket = "pg-dba-bkt"
region = "eu-west-3"
endpoint = "{endpoint}"
prefix = "base"
force_path_style = true
allow_insecure_http = true
access_key_env = "FC_DBA_AK"
secret_key_env = "FC_DBA_SK"
[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "200B"
require_newer_within = "36h"
freeze_on_empty_valid = true
[rules.in_use]
enabled = false

[[rules]]
name = "pg-wal-archive"
path = "{}"
include_regex = '^[0-9A-F]{{24}}$'
exclude_globs = ["*.history", "*.partial", "*.ready", "*.done", "*.tmp", "archive_status/**"]
min_file_size = "100B"
max_files = 1
actions = ["compress", "upload_s3", "delete"]
compression = "lz4"
[rules.s3]
bucket = "pg-dba-bkt"
region = "eu-west-3"
endpoint = "{endpoint}"
prefix = "wal"
force_path_style = true
allow_insecure_http = true
access_key_env = "FC_DBA_AK"
secret_key_env = "FC_DBA_SK"
[rules.backup_guard]
enabled = true
min_keep_files = 1
min_valid_backup_bytes = "100B"
freeze_on_empty_valid = true
[rules.in_use]
enabled = false
[rules.pre_delete_hook]
command = "{}"
args = {args_toml}
skip_exit_code = 42
[rules.pre_delete_hook.env]
FILECLEANER_PG_MIN_KEEP_WAL = "000000010000000000000003"
"#,
            toml_path(&base_dir),
            toml_path(&wal_dir),
            toml_path(Path::new(&hook_cmd)),
        ),
    )
    .unwrap();

    // 3. With only a 0-byte today stub + stale valid dumps, base-backup rule freezes (`exit 3`) with zero uploads.
    cli_bin()
        .env("FC_DBA_AK", "ak")
        .env("FC_DBA_SK", "sk")
        .args([
            "run",
            "-c",
            cfg_path.to_str().unwrap(),
            "--rule",
            "pg-base-backups",
            "--fail-on-freeze",
        ])
        .assert()
        .code(3);
    assert!(old_base_1.exists() && old_base_2.exists());
    assert!(objects.lock().unwrap().is_empty());

    // 4. Write a valid fresh base backup today, then run both DBA rules live.
    fs::write(&broken_today, vec![b'D'; 512]).unwrap();
    cli_bin()
        .env("FC_DBA_AK", "ak")
        .env("FC_DBA_SK", "sk")
        .args(["run", "-c", cfg_path.to_str().unwrap(), "--fail-on-freeze"])
        .assert()
        .code(0);

    // Base backups: oldest dump compressed + uploaded + deleted; newest 2 kept locally.
    assert!(!old_base_1.exists());
    assert!(old_base_2.exists() && broken_today.exists());

    // WAL spool: .history, .partial, archive_status/*.ready, floor (...0004), and hook-vetoed (...0003) remain untouched.
    assert!(history_file.exists());
    assert!(partial_file.exists());
    assert!(ready_file.exists());
    assert!(wal_dir.join("000000010000000000000003").exists());
    assert!(wal_dir.join("000000010000000000000004").exists());

    // Eligible older WAL segments (...0001 and ...0002) archived as LZ4 to S3 and deleted locally.
    assert!(!wal_dir.join("000000010000000000000001").exists());
    assert!(!wal_dir.join("000000010000000000000002").exists());

    let uploaded = objects.lock().unwrap().clone();
    assert!(uploaded.contains_key("pg-dba-bkt/base/pg_20250101_020000.sql.gz"));
    assert!(uploaded.contains_key("pg-dba-bkt/wal/000000010000000000000001.lz4"));
    assert!(uploaded.contains_key("pg-dba-bkt/wal/000000010000000000000002.lz4"));
    assert!(!uploaded.contains_key("pg-dba-bkt/wal/000000010000000000000003.lz4"));

    // Decompress archived LZ4 WAL segment and verify exact content.
    let mut wal_plain = Vec::new();
    FrameDecoder::new(&uploaded["pg-dba-bkt/wal/000000010000000000000001.lz4"][..])
        .read_to_end(&mut wal_plain)
        .unwrap();
    assert_eq!(
        wal_plain,
        "WAL-SEGMENT-000000010000000000000001\n"
            .repeat(64)
            .as_bytes()
    );
}
