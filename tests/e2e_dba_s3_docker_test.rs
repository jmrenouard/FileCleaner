//! Docker E2E harness verification for PostgreSQL (`multi-db-docker-env`) + MinIO S3 archival.
//!
//! - Always-on test validates `scripts/e2e-dba-s3-lab.sh` structure, Bash syntax (on Unix),
//!   security hygiene, and embedded TOML configuration template via `Config::validate()`.
//! - Ignored live test runs the full 6-scenario Docker lab (`cargo test --test e2e_dba_s3_docker_test -- --ignored`).

use filecleaner::Config;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn harness_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/e2e-dba-s3-lab.sh")
}

#[test]
fn docker_e2e_harness_script_and_embedded_config_template_are_valid() {
    let script_path = harness_script_path();
    let content = fs::read_to_string(&script_path).expect("scripts/e2e-dba-s3-lab.sh must exist");

    // Verify strict Bash safety headers, cleanup trap, and all 6 scenarios.
    assert!(content.starts_with("#!/usr/bin/env bash"));
    assert!(content.contains("set -euo pipefail"));
    assert!(content.contains("export LC_ALL=C"));
    assert!(content.contains("trap cleanup EXIT"));
    for scenario in [
        "Scenario 1:",
        "Scenario 2:",
        "Scenario 3:",
        "Scenario 4:",
        "Scenario 5:",
        "Scenario 6:",
    ] {
        assert!(
            content.contains(scenario),
            "harness script missing '{scenario}'"
        );
    }

    // Zero hardcoded AWS production access keys or unpinned `:latest` container images.
    assert!(!content.contains("AKIA"));
    assert!(!content.contains(":latest"));

    // Extract embedded FileCleaner TOML template and validate via Config::validate().
    let begin = "# BEGIN_FILECLEANER_E2E_CONFIG";
    let end = "# END_FILECLEANER_E2E_CONFIG";
    let start_idx = content
        .find(begin)
        .expect("missing BEGIN_FILECLEANER_E2E_CONFIG marker")
        + begin.len();
    let end_idx = content[start_idx..]
        .find(end)
        .expect("missing END_FILECLEANER_E2E_CONFIG marker");
    let raw_tpl = &content[start_idx..start_idx + end_idx];
    let rendered = raw_tpl
        .replace("${AUDIT_LOG}", "/tmp/e2e-audit.jsonl")
        .replace("${BUCKET}", "fc-e2e-pg-archive")
        .replace("${MINIO_ENDPOINT}", "http://127.0.0.1:9000")
        .replace("${BASE_DIR}", "/tmp/e2e-pg-base")
        .replace("${WAL_DIR}", "/tmp/e2e-pg-wal");
    let parsed: Config = toml::from_str(&rendered)
        .unwrap_or_else(|e| panic!("embedded E2E TOML failed to parse: {e}\n{rendered}"));
    parsed
        .validate()
        .unwrap_or_else(|e| panic!("embedded E2E TOML failed validation: {e}\n{rendered}"));
    assert_eq!(parsed.rules.len(), 2);

    // Syntax-check shell script with `bash -n` on Unix runners.
    #[cfg(unix)]
    {
        let status = Command::new("bash")
            .args(["-n", script_path.to_str().unwrap()])
            .status()
            .expect("bash -n must execute");
        assert!(
            status.success(),
            "bash -n failed on scripts/e2e-dba-s3-lab.sh"
        );
    }
}

#[test]
#[ignore = "requires Docker with multi-db-docker-env and MinIO"]
fn live_docker_e2e_postgres_minio_six_scenarios() {
    let script_path = harness_script_path();
    let bin_path = env!("CARGO_BIN_EXE_filecleaner");

    let status = Command::new("bash")
        .arg(&script_path)
        .env("FC_E2E_BIN", bin_path)
        .status()
        .expect("failed to launch scripts/e2e-dba-s3-lab.sh");

    assert!(
        status.success(),
        "live Docker E2E harness failed with status {status}"
    );
}
