//! Cross-platform integration tests for `ActionStep::UploadS3` archival pipeline,
//! fail-closed SHA-256 verification before deletion/trash/move, dry-run parity,
//! and structured JSONL audit logging (`IDEA-001`).

use assert_cmd::Command;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{TimeZone, Utc};
use filecleaner::{
    ActionExecutor, ActionStep, AuditConfig, AuditEntry, AuditEventKind, BackupGuardConfig,
    CombineMode, CompressionFormat, Config, DaemonConfig, EvictionOrder, FileCleanerError,
    FixedClock, InUseConfig, OsLockProbe, ProcessHookRunner, RuleConfig, RuleOutcome, RunOptions,
    S3Config, S3UploadReceipt, S3UploadRequest, S3Uploader, S3VerifyMode, SweepEngine, SweepReport,
    TimeBasis, WorkingTarget, EXIT_OPERATIONAL_ERROR,
};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

#[derive(Default)]
struct LoopbackS3State {
    objects: HashMap<String, (Vec<u8>, String, String)>,
    corrupt_checksum: bool,
    force_500: bool,
}

struct LoopbackS3Server {
    addr: SocketAddr,
    state: Arc<Mutex<LoopbackS3State>>,
    requests: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LoopbackS3Server {
    fn start(initial: LoopbackS3State) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(initial));
        let requests = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let st = Arc::clone(&state);
        let rq = Arc::clone(&requests);
        let sd = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            while !sd.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        rq.fetch_add(1, Ordering::SeqCst);
                        serve_s3_request(stream, &st);
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            state,
            requests,
            shutdown,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for LoopbackS3Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve_s3_request(mut stream: TcpStream, state: &Mutex<LoopbackS3State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = HashMap::<String, String>::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_len];
    if content_len > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let key = target
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let mut st = state.lock().unwrap();
    if st.force_500 {
        write_response(&mut stream, 500, &[], b"");
        return;
    }

    match method.as_str() {
        "HEAD" => {
            if let Some((data, ck, etag)) = st.objects.get(&key).cloned() {
                let effective_ck = if st.corrupt_checksum {
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string()
                } else {
                    ck
                };
                write_response(
                    &mut stream,
                    200,
                    &[
                        ("Content-Length", data.len().to_string()),
                        ("ETag", etag),
                        ("x-amz-checksum-sha256", effective_ck),
                    ],
                    b"",
                );
            } else {
                write_response(&mut stream, 404, &[], b"");
            }
        }
        "PUT" => {
            let b64 = BASE64_STANDARD.encode(Sha256::digest(&body));
            let etag = format!("\"etag-{}\"", body.len());
            st.objects.insert(key, (body, b64, etag.clone()));
            write_response(&mut stream, 200, &[("ETag", etag)], b"");
        }
        "GET" => {
            if let Some((data, _, _)) = st.objects.get(&key).cloned() {
                write_response(&mut stream, 200, &[], &data);
            } else {
                write_response(&mut stream, 404, &[], b"");
            }
        }
        _ => write_response(&mut stream, 405, &[], b""),
    }
}

fn write_response(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status} OK\r\nConnection: close\r\n");
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
    {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    if !body.is_empty() {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
}

fn cli_bin() -> Command {
    Command::cargo_bin("filecleaner").unwrap()
}

fn toml_esc(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn write_stamped(path: &Path, data: &[u8], unix_secs: i64) {
    fs::write(path, data).unwrap();
    let f = OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs as u64))
        .unwrap();
}

#[test]
fn cli_compress_upload_s3_delete_enforces_dry_run_parity_and_verified_archival() {
    let tmp = TempDir::new().unwrap();
    let backups = tmp.path().join("pg_backups");
    let audit_log = tmp.path().join("audit.jsonl");
    fs::create_dir_all(&backups).unwrap();

    let payload = b"CREATE TABLE ledger (id INT, amount NUMERIC);\n".repeat(40);
    let old_ts = Utc
        .with_ymd_and_hms(2025, 11, 10, 2, 0, 0)
        .unwrap()
        .timestamp();
    let new_ts = Utc::now().timestamp() - 60;
    let old_dump = backups.join("pg_20251110_020000.sql");
    let new_dump = backups.join("pg_recent.sql");
    write_stamped(&old_dump, &payload, old_ts);
    write_stamped(&new_dump, &payload, new_ts);

    let srv = LoopbackS3Server::start(LoopbackS3State::default());
    let cfg_path = tmp.path().join("s3_pipeline.toml");
    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run = false

[audit]
log_path = "{}"
jsonl = true

[s3]
bucket = "pg-vault"
region = "eu-west-3"
endpoint = "{}"
prefix = "prod/pg"
key_date_subdir = "%Y/%m"
force_path_style = true
allow_insecure_http = true
access_key_env = "FC_IT_S3_AK"
secret_key_env = "FC_IT_S3_SK"
timeout = "5s"
max_retries = 1

[[rules]]
name = "pg-archival"
path = "{}"
include_globs = ["*.sql"]
max_files = 1
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"

[rules.in_use]
enabled = true
check_os_locks = false
check_proc_fd = false
"#,
            toml_esc(&audit_log),
            srv.endpoint(),
            toml_esc(&backups),
        ),
    )
    .unwrap();

    // 1. `filecleaner plan --json` makes zero network calls and zero filesystem mutations.
    let plan_out = cli_bin()
        .args(["plan", "-c", cfg_path.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plan_report: SweepReport = serde_json::from_slice(&plan_out).unwrap();
    assert!(plan_report.dry_run);
    assert_eq!(plan_report.summary.total_acted, 1);
    assert_eq!(srv.request_count(), 0);
    assert!(old_dump.exists() && new_dump.exists());

    // 2. `filecleaner run --dry-run` also makes zero network calls and records projected s3_uri in audit.jsonl.
    cli_bin()
        .args(["run", "-c", cfg_path.to_str().unwrap(), "--dry-run"])
        .assert()
        .success();
    assert_eq!(srv.request_count(), 0);
    assert!(old_dump.exists());

    let dry_entries: Vec<AuditEntry> = fs::read_to_string(&audit_log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(dry_entries.iter().any(|e| {
        e.event == AuditEventKind::ActionExec
            && e.dry_run
            && e.details
                .as_ref()
                .and_then(|d| d.get("s3_uri"))
                .and_then(|v| v.as_str())
                == Some("s3://pg-vault/prod/pg/2025/11/pg_20251110_020000.sql.gz")
    }));

    // 3. Live `filecleaner run` compresses, uploads with SHA-256 verification, and deletes local file.
    cli_bin()
        .env("FC_IT_S3_AK", "it-ak")
        .env("FC_IT_S3_SK", "it-sk")
        .args(["run", "-c", cfg_path.to_str().unwrap()])
        .assert()
        .success();

    assert!(!old_dump.exists());
    assert!(!backups.join("pg_20251110_020000.sql.gz").exists());
    assert!(new_dump.exists());

    // Remote stored object decompresses to exact original payload.
    let remote_key = "pg-vault/prod/pg/2025/11/pg_20251110_020000.sql.gz";
    let (stored_gz, _, _) = srv.state.lock().unwrap().objects[remote_key].clone();
    let mut restored = Vec::new();
    GzDecoder::new(&stored_gz[..])
        .read_to_end(&mut restored)
        .unwrap();
    assert_eq!(restored, payload);

    // Audit JSONL records sha256, s3_uri, etag, and verify_mode on live ActionExec & FileDecision.
    let all_entries: Vec<AuditEntry> = fs::read_to_string(&audit_log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let live_exec = all_entries
        .iter()
        .find(|e| e.event == AuditEventKind::ActionExec && !e.dry_run)
        .expect("live ActionExec audit entry");
    assert!(live_exec.sha256.as_ref().is_some_and(|s| s.len() == 64));
    let details = live_exec.details.as_ref().expect("s3 audit details");
    assert_eq!(
        details["s3_uri"],
        "s3://pg-vault/prod/pg/2025/11/pg_20251110_020000.sql.gz"
    );
    assert_eq!(details["verify_mode"], "checksum_sha256");
    assert_eq!(details["parts_uploaded"], 1);
    assert!(details["etag"].as_str().is_some());
}

#[test]
fn fail_closed_upload_errors_preserve_local_files_before_delete_trash_and_move() {
    let tmp = TempDir::new().unwrap();
    let dir_del = tmp.path().join("del_dir");
    let dir_trash = tmp.path().join("trash_dir");
    let dir_move = tmp.path().join("move_dir");
    let trash_dest = tmp.path().join("quarantine");
    let move_dest = tmp.path().join("moved");
    let audit_log = tmp.path().join("fail_audit.jsonl");
    for d in [&dir_del, &dir_trash, &dir_move] {
        fs::create_dir_all(d).unwrap();
    }

    let old_ts = Utc::now().timestamp() - 3600;
    let f_del = dir_del.join("wal_del.bin");
    let f_trash = dir_trash.join("wal_trash.bin");
    let f_move = dir_move.join("wal_move.bin");
    for f in [&f_del, &f_trash, &f_move] {
        write_stamped(f, b"critical-wal-bytes", old_ts);
    }

    // Scenario A: Remote SHA-256 mismatch aborts before Delete, Trash, and Move; exits 1.
    let srv = LoopbackS3Server::start(LoopbackS3State {
        corrupt_checksum: true,
        ..LoopbackS3State::default()
    });
    let cfg_path = tmp.path().join("fail_closed.toml");
    fs::write(
        &cfg_path,
        format!(
            r#"
dry_run = false
global_trash_dir = "{}"

[audit]
log_path = "{}"
jsonl = true

[s3]
bucket = "wal-bkt"
region = "us-east-1"
endpoint = "{}"
force_path_style = true
allow_insecure_http = true
access_key_env = "FC_FAIL_AK"
secret_key_env = "FC_FAIL_SK"
timeout = "3s"
max_retries = 0

[[rules]]
name = "r-del"
path = "{}"
max_age = "10s"
actions = ["upload_s3", "delete"]
[rules.in_use]
enabled = false

[[rules]]
name = "r-trash"
path = "{}"
max_age = "10s"
actions = ["upload_s3", "trash"]
[rules.in_use]
enabled = false

[[rules]]
name = "r-move"
path = "{}"
max_age = "10s"
actions = ["upload_s3", "move"]
move_to = "{}"
[rules.in_use]
enabled = false
"#,
            toml_esc(&trash_dest),
            toml_esc(&audit_log),
            srv.endpoint(),
            toml_esc(&dir_del),
            toml_esc(&dir_trash),
            toml_esc(&dir_move),
            toml_esc(&move_dest),
        ),
    )
    .unwrap();

    cli_bin()
        .env("FC_FAIL_AK", "ak")
        .env("FC_FAIL_SK", "sk")
        .args(["run", "-c", cfg_path.to_str().unwrap()])
        .assert()
        .code(EXIT_OPERATIONAL_ERROR);

    assert!(
        f_del.exists(),
        "local file must survive SHA-256 mismatch before Delete"
    );
    assert!(
        f_trash.exists(),
        "local file must survive SHA-256 mismatch before Trash"
    );
    assert!(
        f_move.exists(),
        "local file must survive SHA-256 mismatch before Move"
    );
    assert!(!trash_dest.exists());
    assert!(!move_dest.exists());

    let entries: Vec<AuditEntry> = fs::read_to_string(&audit_log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(
        entries
            .iter()
            .filter(|e| e.event == AuditEventKind::Error)
            .count()
            >= 3
    );

    // Scenario B: Overwrite collision (object already exists in S3 with overwrite = false) aborts before Delete.
    srv.state.lock().unwrap().corrupt_checksum = false;
    cli_bin()
        .env("FC_FAIL_AK", "ak")
        .env("FC_FAIL_SK", "sk")
        .args(["run", "-c", cfg_path.to_str().unwrap(), "--rule", "r-del"])
        .assert()
        .code(EXIT_OPERATIONAL_ERROR);
    assert!(
        f_del.exists(),
        "local file must remain intact on overwrite=false collision"
    );

    // Scenario C: Unreachable / 500 server aborts before Delete and keeps local file intact.
    srv.state.lock().unwrap().force_500 = true;
    cli_bin()
        .env("FC_FAIL_AK", "ak")
        .env("FC_FAIL_SK", "sk")
        .args(["run", "-c", cfg_path.to_str().unwrap(), "--rule", "r-del"])
        .assert()
        .code(EXIT_OPERATIONAL_ERROR);
    assert!(f_del.exists(), "local file must remain intact on 500 fault");
}

struct StubUploader {
    fail: bool,
    calls: AtomicUsize,
}

impl S3Uploader for StubUploader {
    fn upload_and_verify(&self, req: &S3UploadRequest) -> filecleaner::Result<S3UploadReceipt> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let uri = filecleaner::build_s3_uri(&req.config.bucket, &req.key);
        if self.fail {
            return Err(FileCleanerError::S3Upload {
                path: req.local_path.clone(),
                uri,
                reason: "injected uploader failure".to_string(),
            });
        }
        Ok(S3UploadReceipt {
            uri,
            etag: Some("\"stub-etag\"".to_string()),
            verified_sha256: req.sha256_hex.clone(),
            verified_size: req.content_length,
            verify_mode: req.config.verify_mode,
            parts_uploaded: 1,
        })
    }
}

#[test]
fn injectable_engine_and_pipeline_handle_upload_only_constrained_and_failure_details() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("spools");
    let audit_log = tmp.path().join("inject_audit.jsonl");
    fs::create_dir_all(&dir).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0).unwrap();
    let f1 = dir.join("a.wal");
    let f2 = dir.join("b.wal");
    write_stamped(&f1, b"wal-data-1", now.timestamp() - 200);
    write_stamped(&f2, b"wal-data-2", now.timestamp() - 100);

    let s3_cfg = S3Config {
        bucket: "inject-bkt".to_string(),
        region: "us-east-1".to_string(),
        verify_mode: S3VerifyMode::ChecksumSha256,
        ..S3Config::default()
    };

    let rule = RuleConfig {
        name: "upload-only".to_string(),
        path: dir.clone(),
        enabled: true,
        recursive: false,
        max_depth: None,
        follow_symlinks: false,
        include_globs: vec!["*.wal".to_string()],
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
            enabled: false,
            ..InUseConfig::default()
        },
        pre_delete_hook: None,
        actions: vec![ActionStep::UploadS3],
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
        s3: Some(s3_cfg.clone()),
    };

    let cfg = Config {
        dry_run: false,
        audit_log: None,
        daemon: DaemonConfig::default(),
        audit: AuditConfig {
            log_path: Some(audit_log.clone()),
            jsonl: true,
        },
        global_trash_dir: None,
        s3: None,
        rules: vec![rule.clone()],
    };

    let ok_up = StubUploader {
        fail: false,
        calls: AtomicUsize::new(0),
    };
    let report = SweepEngine::run_config_with_s3(
        &cfg,
        &RunOptions::default(),
        &FixedClock(now),
        &OsLockProbe,
        &ProcessHookRunner,
        &ok_up,
    )
    .unwrap();
    assert_eq!(report.exit_code, 0);
    assert_eq!(
        report.rule_reports[0].plan.outcome,
        RuleOutcome::Constrained
    );
    assert_eq!(ok_up.calls.load(Ordering::SeqCst), 2);
    assert!(f1.exists() && f2.exists());

    // Injected S3Upload failure records details.s3_uri on AuditEventKind::Error and returns exit 1.
    let fail_up = StubUploader {
        fail: true,
        calls: AtomicUsize::new(0),
    };
    let fail_report = SweepEngine::run_config_with_s3(
        &cfg,
        &RunOptions::default(),
        &FixedClock(now),
        &OsLockProbe,
        &ProcessHookRunner,
        &fail_up,
    )
    .unwrap();
    assert_eq!(fail_report.exit_code, EXIT_OPERATIONAL_ERROR);
    let log_text = fs::read_to_string(&audit_log).unwrap();
    assert!(log_text.contains("\"s3_uri\":\"s3://inject-bkt/a.wal\""));

    // Direct execute_pipeline_with_s3 dry-run projects remote_uri with zero uploader calls.
    let dry_up = StubUploader {
        fail: true,
        calls: AtomicUsize::new(0),
    };
    let target = WorkingTarget {
        path: PathBuf::from(&f1),
        size_bytes: 10,
        mtime: now,
        atime: None,
    };
    let dry_rep = ActionExecutor::execute_pipeline_with_s3(
        target,
        &rule,
        None,
        Some(&s3_cfg),
        Some(&dry_up),
        true,
    )
    .unwrap();
    assert_eq!(dry_up.calls.load(Ordering::SeqCst), 0);
    assert_eq!(dry_rep.remote_uri.as_deref(), Some("s3://inject-bkt/a.wal"));
    assert!(dry_rep.s3_receipt.is_none());

    // Compress (compress_remove_source = false) -> UploadS3 uploads the `.gz` archive, not raw source.
    let keep_src_file = dir.join("keep_me.sql");
    write_stamped(&keep_src_file, b"SELECT 1;\n", now.timestamp() - 300);
    let mut keep_rule = rule.clone();
    keep_rule.actions = vec![ActionStep::Compress, ActionStep::UploadS3];
    keep_rule.compress_remove_source = false;
    let keep_up = StubUploader {
        fail: false,
        calls: AtomicUsize::new(0),
    };
    let keep_rep = ActionExecutor::execute_pipeline_with_s3(
        WorkingTarget {
            path: keep_src_file.clone(),
            size_bytes: 10,
            mtime: now,
            atime: None,
        },
        &keep_rule,
        None,
        None,
        Some(&keep_up),
        false,
    )
    .unwrap();
    assert!(keep_src_file.exists());
    assert_eq!(
        keep_rep.remote_uri.as_deref(),
        Some("s3://inject-bkt/keep_me.sql.gz")
    );
    assert!(keep_rep.s3_receipt.unwrap().verified_size > 0);

    // CopyTruncate -> UploadS3 uploads the rotated archive rather than the 0-byte truncated live file.
    let ct_dir = tmp.path().join("ct_rotated");
    let live_log = dir.join("service.log");
    write_stamped(&live_log, b"important-log-payload", now.timestamp() - 300);
    let mut ct_rule = rule;
    ct_rule.actions = vec![ActionStep::CopyTruncate, ActionStep::UploadS3];
    ct_rule.copy_truncate_to = Some(ct_dir);
    let ct_up = StubUploader {
        fail: false,
        calls: AtomicUsize::new(0),
    };
    let ct_rep = ActionExecutor::execute_pipeline_with_s3(
        WorkingTarget {
            path: live_log.clone(),
            size_bytes: 21,
            mtime: now,
            atime: None,
        },
        &ct_rule,
        None,
        None,
        Some(&ct_up),
        false,
    )
    .unwrap();
    assert_eq!(fs::metadata(&live_log).unwrap().len(), 0);
    assert_eq!(
        ct_rep.remote_uri.as_deref(),
        Some("s3://inject-bkt/service_20261122_120000.log")
    );
    assert_eq!(ct_rep.s3_receipt.unwrap().verified_size, 21);
}
