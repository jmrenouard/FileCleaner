//! Integration tests for FileCleaner pre-delete hook execution: direct argv
//! command-injection safety, `FILECLEANER_*` environment variables, timeout process
//! termination & zombie reaping, exit `42` veto, failure policies, dry-run gating,
//! and live execution of `examples/hooks/*.sh`.

use chrono::{TimeZone, Utc};
use filecleaner::{
    evaluate_hook, format_action_summary, ActionStep, CompressionFormat, FileCleanerError,
    HookCandidate, HookConfig, HookFailureDisposition, HookFailurePolicy, HookRunner, HookVerdict,
    ProcessHookRunner,
};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
#[cfg(any(unix, windows))]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
#[cfg(any(unix, windows))]
use std::time::Instant;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(any(unix, windows))]
fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn make_hook_config(command: String, args: Vec<String>) -> HookConfig {
    HookConfig {
        command,
        args,
        timeout: Duration::from_secs(5),
        on_failure: HookFailurePolicy::AbortFile,
        skip_exit_code: Some(42),
        run_in_dry_run: false,
        working_dir: None,
        env: BTreeMap::new(),
    }
}

fn make_candidate(path: PathBuf, rule_name: &str) -> HookCandidate {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    HookCandidate {
        path,
        name,
        size_bytes: 1234,
        mtime: Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap(),
        rule_name: rule_name.to_string(),
        action_summary: format_action_summary(
            &[ActionStep::Compress, ActionStep::Move],
            CompressionFormat::Gzip,
        ),
    }
}

#[cfg(unix)]
#[test]
fn hook_direct_argv_blocks_shell_injection_and_passes_all_env_vars() {
    let tmp = TempDir::new().unwrap();
    let pwned_sentinel = tmp.path().join("PWNED");
    let out_dump = tmp.path().join("hook_capture.txt");

    // Hostile filename containing shell metacharacters: semicolon, command substitution, backticks, quotes, spaces.
    let hostile_name =
        "backup; touch PWNED $(touch PWNED) `touch PWNED`; 'quoted' \"dq\" | cat .sql";
    let candidate_dir = tmp.path().join("spool dir");
    fs::create_dir_all(&candidate_dir).unwrap();
    let candidate_path = candidate_dir.join(hostile_name);
    fs::write(&candidate_path, b"payload").unwrap();

    let script = write_executable_script(
        tmp.path(),
        "inspect.sh",
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
{{
  printf 'ARG1=%s\n' "$1"
  printf 'ARG2=%s\n' "$2"
  printf 'ARG3=%s\n' "$3"
  printf 'PWD=%s\n' "$(pwd)"
  printf 'PATH=%s\n' "$FILECLEANER_PATH"
  printf 'NAME=%s\n' "$FILECLEANER_NAME"
  printf 'DIR=%s\n' "$FILECLEANER_DIR"
  printf 'SIZE=%s\n' "$FILECLEANER_SIZE"
  printf 'MTIME=%s\n' "$FILECLEANER_MTIME_ISO"
  printf 'RULE=%s\n' "$FILECLEANER_RULE"
  printf 'ACTION=%s\n' "$FILECLEANER_ACTION"
  printf 'DRY=%s\n' "$FILECLEANER_DRY_RUN"
  printf 'CUSTOM=%s\n' "$CUSTOM_VAR"
}} > "{}"
"#,
            out_dump.display()
        ),
    );

    let workdir = tmp.path().join("hook_wd");
    fs::create_dir_all(&workdir).unwrap();
    let pwned_in_wd = workdir.join("PWNED");

    let mut hook = make_hook_config(
        script.to_string_lossy().into_owned(),
        vec![
            "{path}".to_string(),
            "{name}".to_string(),
            "{rule}:{size}:{mtime_iso}".to_string(),
        ],
    );
    hook.working_dir = Some(workdir.clone());
    hook.env
        .insert("CUSTOM_VAR".to_string(), "expected-val".to_string());
    // Ensure static hook.env cannot spoof authoritative FILECLEANER_* variables.
    hook.env
        .insert("FILECLEANER_RULE".to_string(), "spoofed-rule".to_string());

    let candidate = make_candidate(candidate_path.clone(), "incoming-etl");
    let runner = ProcessHookRunner::new();
    let verdict = runner.run(&candidate, &hook, false).unwrap();
    assert_eq!(verdict, HookVerdict::Proceed);
    assert!(
        !pwned_sentinel.exists() && !pwned_in_wd.exists(),
        "shell metacharacters in filename must never execute commands"
    );

    let captured = fs::read_to_string(&out_dump).unwrap();
    assert!(captured.contains(&format!("ARG1={}", candidate_path.display())));
    assert!(captured.contains(&format!("ARG2={hostile_name}")));
    assert!(captured.contains("ARG3=incoming-etl:1234:2026-11-20T09:05:00Z"));
    assert!(captured.contains(&format!("PWD={}", workdir.display())));
    assert!(captured.contains(&format!("PATH={}", candidate_path.display())));
    assert!(captured.contains(&format!("NAME={hostile_name}")));
    assert!(captured.contains(&format!("DIR={}", candidate_dir.display())));
    assert!(captured.contains("SIZE=1234"));
    assert!(captured.contains("MTIME=2026-11-20T09:05:00Z"));
    assert!(captured.contains("RULE=incoming-etl"));
    assert!(captured.contains("ACTION=compress(Gzip)->move"));
    assert!(captured.contains("DRY=false"));
    assert!(captured.contains("CUSTOM=expected-val"));
}

#[cfg(unix)]
#[test]
fn hook_timeout_kills_process_and_honors_failure_policies() {
    let tmp = TempDir::new().unwrap();
    let candidate_path = tmp.path().join("slow.dump");
    fs::write(&candidate_path, b"x").unwrap();
    let candidate = make_candidate(candidate_path, "nightly");

    let sleeper = write_executable_script(
        tmp.path(),
        "sleep_hook.sh",
        "#!/usr/bin/env bash\nsleep 5\n",
    );

    let mut hook = make_hook_config(sleeper.to_string_lossy().into_owned(), vec![]);
    hook.timeout = Duration::from_millis(120);
    hook.on_failure = HookFailurePolicy::AbortFile;

    let runner = ProcessHookRunner::new();
    let start = Instant::now();
    let err = runner.run(&candidate, &hook, false).unwrap_err();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout kill must return rapidly instead of waiting 5s (took {elapsed:?})"
    );
    assert!(matches!(err, FileCleanerError::HookTimedOut { .. }));

    // Timeout with WarnAndContinue converts into Ok(WarnAndContinue).
    hook.on_failure = HookFailurePolicy::WarnAndContinue;
    let verdict = runner.run(&candidate, &hook, false).unwrap();
    assert!(matches!(verdict, HookVerdict::WarnAndContinue { .. }));
}

#[cfg(unix)]
#[test]
fn hook_exit_codes_skip_veto_failure_policies_and_dry_run_mode() {
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("hook_ran.marker");
    let candidate_path = tmp.path().join("data.bin");
    fs::write(&candidate_path, b"abc").unwrap();
    let candidate = make_candidate(candidate_path, "spool");

    // 1. Dry-run skips execution when run_in_dry_run = false, executes when true.
    let marker_script = write_executable_script(
        tmp.path(),
        "marker.sh",
        &format!(
            "#!/usr/bin/env bash\nprintf '%s' \"$FILECLEANER_DRY_RUN\" > \"{}\"\nexit 0\n",
            marker.display()
        ),
    );
    let mut dry_hook = make_hook_config(marker_script.to_string_lossy().into_owned(), vec![]);
    let runner = ProcessHookRunner::new();
    assert_eq!(
        runner.run(&candidate, &dry_hook, true).unwrap(),
        HookVerdict::SimulatedDryRun
    );
    assert!(!marker.exists());

    dry_hook.run_in_dry_run = true;
    assert_eq!(
        runner.run(&candidate, &dry_hook, true).unwrap(),
        HookVerdict::Proceed
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "true");

    // 2. Exit 42 veto and custom skip exit code.
    let veto_script = write_executable_script(
        tmp.path(),
        "veto.sh",
        "#!/usr/bin/env bash\nexit \"${1:-42}\"\n",
    );
    let mut veto_hook = make_hook_config(
        veto_script.to_string_lossy().into_owned(),
        vec!["42".to_string()],
    );
    assert_eq!(
        runner.run(&candidate, &veto_hook, false).unwrap(),
        HookVerdict::SkippedByCode(42)
    );

    veto_hook.args = vec!["77".to_string()];
    veto_hook.skip_exit_code = Some(77);
    assert_eq!(
        runner.run(&candidate, &veto_hook, false).unwrap(),
        HookVerdict::SkippedByCode(77)
    );

    // 3. Non-zero failure policies: AbortFile, AbortRule, WarnAndContinue, and spawn error.
    let fail_script = write_executable_script(
        tmp.path(),
        "fail.sh",
        "#!/usr/bin/env bash\necho 'checksum bad' >&2\nexit 17\n",
    );
    let mut fail_hook = make_hook_config(fail_script.to_string_lossy().into_owned(), vec![]);
    fail_hook.on_failure = HookFailurePolicy::AbortFile;
    let (err_file, disp_file) = evaluate_hook(&runner, &candidate, &fail_hook, false).unwrap_err();
    assert_eq!(disp_file, HookFailureDisposition::AbortFile);
    assert!(matches!(
        err_file,
        FileCleanerError::HookFailed {
            exit_code: Some(17),
            ..
        }
    ));

    fail_hook.on_failure = HookFailurePolicy::AbortRule;
    let (err_rule, disp_rule) = evaluate_hook(&runner, &candidate, &fail_hook, false).unwrap_err();
    assert_eq!(disp_rule, HookFailureDisposition::AbortRule);
    assert!(matches!(
        err_rule,
        FileCleanerError::HookFailed {
            exit_code: Some(17),
            ..
        }
    ));

    fail_hook.on_failure = HookFailurePolicy::WarnAndContinue;
    let warn_verdict = evaluate_hook(&runner, &candidate, &fail_hook, false).unwrap();
    assert!(matches!(warn_verdict, HookVerdict::WarnAndContinue { .. }));

    // Non-existent executable triggers spawn failure respecting policy.
    let mut spawn_err_hook =
        make_hook_config("/nonexistent/filecleaner_missing_hook".to_string(), vec![]);
    spawn_err_hook.on_failure = HookFailurePolicy::AbortRule;
    let (_, spawn_disp) = evaluate_hook(&runner, &candidate, &spawn_err_hook, false).unwrap_err();
    assert_eq!(spawn_disp, HookFailureDisposition::AbortRule);

    // Signal-killed child has exit_code: None and maps to HookFailed.
    let sig_script = write_executable_script(
        tmp.path(),
        "killed.sh",
        "#!/usr/bin/env bash\nkill -KILL $$\n",
    );
    let sig_hook = make_hook_config(sig_script.to_string_lossy().into_owned(), vec![]);
    let sig_err = runner.run(&candidate, &sig_hook, false).unwrap_err();
    assert!(matches!(
        sig_err,
        FileCleanerError::HookFailed {
            exit_code: None,
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn example_hook_scripts_execute_veto_and_verify_checksums() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let s3_script = manifest_dir.join("examples/hooks/archive_to_s3_mock.sh");
    let cksum_script = manifest_dir.join("examples/hooks/verify_checksum_before_delete.sh");

    let tmp = TempDir::new().unwrap();
    let s3_bucket = tmp.path().join("s3-mock");
    let data_file = tmp.path().join("etl_batch.csv");
    let payload = b"id,amount\n1,42.50\n2,99.00\n";
    fs::write(&data_file, payload).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(payload);
    let hex_digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let candidate = HookCandidate {
        path: data_file.clone(),
        name: "etl_batch.csv".to_string(),
        size_bytes: payload.len() as u64,
        mtime: Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap(),
        rule_name: "etl-spool".to_string(),
        action_summary: "compress(Zip)->move".to_string(),
    };
    let runner = ProcessHookRunner::new();

    // 1. Test verify_checksum_before_delete.sh: missing manifest in --strict -> exit 42 veto.
    let cksum_hook = make_hook_config(
        "bash".to_string(),
        vec![
            cksum_script.to_string_lossy().into_owned(),
            "--strict".to_string(),
            "{path}".to_string(),
            "{size}".to_string(),
        ],
    );
    assert_eq!(
        runner.run(&candidate, &cksum_hook, false).unwrap(),
        HookVerdict::SkippedByCode(42)
    );

    // Matching manifest -> exit 0 Proceed.
    let manifest_path = tmp.path().join("etl_batch.csv.sha256");
    fs::write(&manifest_path, format!("{hex_digest}  etl_batch.csv\n")).unwrap();
    assert_eq!(
        runner.run(&candidate, &cksum_hook, false).unwrap(),
        HookVerdict::Proceed
    );

    // Corrupted manifest -> exit 1 HookFailed.
    fs::write(&manifest_path, "0000badhash  etl_batch.csv\n").unwrap();
    assert!(matches!(
        runner.run(&candidate, &cksum_hook, false),
        Err(FileCleanerError::HookFailed {
            exit_code: Some(1),
            ..
        })
    ));

    // 2. Test archive_to_s3_mock.sh: uploads file and respects .hold veto.
    let mut s3_hook = make_hook_config(
        "bash".to_string(),
        vec![s3_script.to_string_lossy().into_owned()],
    );
    s3_hook.env.insert(
        "FILECLEANER_S3_MOCK_DIR".to_string(),
        s3_bucket.to_string_lossy().into_owned(),
    );
    assert_eq!(
        runner.run(&candidate, &s3_hook, false).unwrap(),
        HookVerdict::Proceed
    );
    assert_eq!(
        fs::read(s3_bucket.join("etl-spool/etl_batch.csv")).unwrap(),
        payload
    );

    let hold_marker = tmp.path().join("etl_batch.csv.hold");
    fs::write(&hold_marker, b"hold").unwrap();
    assert_eq!(
        runner.run(&candidate, &s3_hook, false).unwrap(),
        HookVerdict::SkippedByCode(42)
    );
}

#[test]
fn hook_dry_run_empty_and_missing_command_honor_failure_policies() {
    let tmp = TempDir::new().unwrap();
    let candidate_path = tmp.path().join("artifact.sql");
    fs::write(&candidate_path, b"payload").unwrap();
    let candidate = make_candidate(candidate_path, "nightly");
    let runner = ProcessHookRunner::new();

    let missing_bin = tmp.path().join("nonexistent_filecleaner_hook_bin");
    let mut hook = make_hook_config(missing_bin.to_string_lossy().into_owned(), vec![]);
    assert_eq!(
        runner.run(&candidate, &hook, true).unwrap(),
        HookVerdict::SimulatedDryRun
    );

    hook.on_failure = HookFailurePolicy::AbortRule;
    let (err, disp) = evaluate_hook(&runner, &candidate, &hook, false).unwrap_err();
    assert_eq!(disp, HookFailureDisposition::AbortRule);
    assert!(matches!(
        err,
        FileCleanerError::HookFailed {
            exit_code: None,
            ..
        }
    ));

    hook.on_failure = HookFailurePolicy::WarnAndContinue;
    let verdict = evaluate_hook(&runner, &candidate, &hook, false).unwrap();
    assert!(matches!(verdict, HookVerdict::WarnAndContinue { .. }));

    let empty_hook = make_hook_config("   ".to_string(), vec![]);
    let (empty_err, empty_disp) =
        evaluate_hook(&runner, &candidate, &empty_hook, false).unwrap_err();
    assert_eq!(empty_disp, HookFailureDisposition::AbortFile);
    assert!(matches!(empty_err, FileCleanerError::HookFailed { .. }));
}

#[cfg(windows)]
#[test]
fn hook_windows_direct_argv_env_vars_exit_codes_and_timeout() {
    let tmp = TempDir::new().unwrap();
    let pwned_sentinel = tmp.path().join("PWNED");
    let out_dump = tmp.path().join("hook_capture.txt");

    // Hostile NTFS-valid filename containing `;`, `&`, `$()`, quotes, and spaces.
    let hostile_name = "backup; & touch PWNED $(id) 'quoted' .sql";
    let candidate_dir = tmp.path().join("spool dir");
    fs::create_dir_all(&candidate_dir).unwrap();
    let candidate_path = candidate_dir.join(hostile_name);
    fs::write(&candidate_path, b"payload").unwrap();

    let script = write_executable_script(
        tmp.path(),
        "inspect.ps1",
        r#"param([string]$a1, [string]$a2, [string]$a3)
$lines = @(
  "ARG1=$a1"
  "ARG2=$a2"
  "ARG3=$a3"
  "PWD=$((Get-Location).Path)"
  "PATH=$env:FILECLEANER_PATH"
  "NAME=$env:FILECLEANER_NAME"
  "DIR=$env:FILECLEANER_DIR"
  "SIZE=$env:FILECLEANER_SIZE"
  "MTIME=$env:FILECLEANER_MTIME_ISO"
  "RULE=$env:FILECLEANER_RULE"
  "ACTION=$env:FILECLEANER_ACTION"
  "DRY=$env:FILECLEANER_DRY_RUN"
  "CUSTOM=$env:CUSTOM_VAR"
)
[System.IO.File]::WriteAllLines($env:TEST_OUT_DUMP, $lines)
"#,
    );

    let workdir = tmp.path().join("hook_wd");
    fs::create_dir_all(&workdir).unwrap();
    let pwned_in_wd = workdir.join("PWNED");

    let mut hook = make_hook_config(
        "powershell.exe".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            script.to_string_lossy().into_owned(),
            "{path}".to_string(),
            "{name}".to_string(),
            "{rule}:{size}:{mtime_iso}".to_string(),
        ],
    );
    hook.timeout = Duration::from_secs(15);
    hook.working_dir = Some(workdir.clone());
    hook.env
        .insert("CUSTOM_VAR".to_string(), "expected-val".to_string());
    hook.env.insert(
        "TEST_OUT_DUMP".to_string(),
        out_dump.to_string_lossy().into_owned(),
    );
    hook.env
        .insert("FILECLEANER_RULE".to_string(), "spoofed-rule".to_string());

    let candidate = make_candidate(candidate_path.clone(), "incoming-etl");
    let runner = ProcessHookRunner::new();
    let verdict = runner.run(&candidate, &hook, false).unwrap();
    assert_eq!(verdict, HookVerdict::Proceed);
    assert!(
        !pwned_sentinel.exists() && !pwned_in_wd.exists(),
        "metacharacters in filename must never execute commands"
    );

    let captured = fs::read_to_string(&out_dump).unwrap();
    assert!(captured.contains(&format!("ARG1={}", candidate_path.display())));
    assert!(captured.contains(&format!("ARG2={hostile_name}")));
    assert!(captured.contains("ARG3=incoming-etl:1234:2026-11-20T09:05:00Z"));
    let pwd_line = captured
        .lines()
        .find_map(|line| line.strip_prefix("PWD="))
        .expect("PWD line missing");
    assert_eq!(
        fs::canonicalize(pwd_line.trim()).unwrap(),
        fs::canonicalize(&workdir).unwrap()
    );
    assert!(captured.contains(&format!("PATH={}", candidate_path.display())));
    assert!(captured.contains(&format!("NAME={hostile_name}")));
    assert!(captured.contains(&format!("DIR={}", candidate_dir.display())));
    assert!(captured.contains("SIZE=1234"));
    assert!(captured.contains("MTIME=2026-11-20T09:05:00Z"));
    assert!(captured.contains("RULE=incoming-etl"));
    assert!(captured.contains("ACTION=compress(Gzip)->move"));
    assert!(captured.contains("DRY=false"));
    assert!(captured.contains("CUSTOM=expected-val"));

    // Exit 42 veto and failure policies via PowerShell subprocesses.
    let mut veto_hook = make_hook_config(
        "powershell.exe".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            "exit 42".to_string(),
        ],
    );
    veto_hook.timeout = Duration::from_secs(15);
    assert_eq!(
        runner.run(&candidate, &veto_hook, false).unwrap(),
        HookVerdict::SkippedByCode(42)
    );

    let mut fail_hook = make_hook_config(
        "powershell.exe".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            "exit 17".to_string(),
        ],
    );
    fail_hook.timeout = Duration::from_secs(15);
    fail_hook.on_failure = HookFailurePolicy::AbortRule;
    let (fail_err, disp) = evaluate_hook(&runner, &candidate, &fail_hook, false).unwrap_err();
    assert_eq!(disp, HookFailureDisposition::AbortRule);
    assert!(matches!(
        fail_err,
        FileCleanerError::HookFailed {
            exit_code: Some(17),
            ..
        }
    ));

    // Timeout enforcement kills child process promptly.
    let mut timeout_hook = make_hook_config(
        "ping.exe".to_string(),
        vec!["-n".to_string(), "10".to_string(), "127.0.0.1".to_string()],
    );
    timeout_hook.timeout = Duration::from_millis(150);
    timeout_hook.on_failure = HookFailurePolicy::AbortFile;
    let start = Instant::now();
    let timeout_err = runner.run(&candidate, &timeout_hook, false).unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(matches!(timeout_err, FileCleanerError::HookTimedOut { .. }));
}
