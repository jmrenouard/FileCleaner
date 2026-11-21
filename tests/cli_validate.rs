//! Integration tests for the Wave 1 `filecleaner validate` CLI entrypoint.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn cli_validate_succeeds_for_valid_toml_and_fails_for_invalid_config() {
    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("dumps");
    fs::create_dir_all(&target_dir).unwrap();

    let valid_toml = tmp.path().join("valid.toml");
    fs::write(
        &valid_toml,
        format!(
            r#"
[[rules]]
name = "dumps"
path = "{}"
max_files = 5
"#,
            target_dir.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();

    Command::cargo_bin("filecleaner")
        .unwrap()
        .args(["validate", "--config", valid_toml.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("configuration valid: 1 rule(s)"));

    let invalid_yaml = tmp.path().join("invalid.yaml");
    fs::write(&invalid_yaml, "rules: []\n").unwrap();

    Command::cargo_bin("filecleaner")
        .unwrap()
        .args(["validate", "--config", invalid_yaml.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one rule"));

    // Positional `<PATH>` validation rejects invalid `rename_template` eagerly (Fix #2).
    let bad_rename_toml = tmp.path().join("bad_rename.toml");
    fs::write(
        &bad_rename_toml,
        format!(
            r#"
[[rules]]
name = "logs"
path = "{}"
actions = ["rename_by_date"]
rename_template = "%Q"
"#,
            target_dir.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();

    Command::cargo_bin("filecleaner")
        .unwrap()
        .args(["validate", bad_rename_toml.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("rename_template"));

    // Valid S3 TOML and YAML configs pass validation; insecure http:// without opt-in fails with exit 2.
    let s3_toml = tmp.path().join("s3_valid.toml");
    let target_esc = target_dir.display().to_string().replace('\\', "\\\\");
    fs::write(
        &s3_toml,
        format!(
            r#"
[s3]
bucket = "db-archive"
region = "eu-west-3"
endpoint = "https://s3.internal:9000"
force_path_style = true

[[rules]]
name = "pg-wal"
path = "{target_esc}"
actions = ["compress", "upload_s3", "delete"]
"#
        ),
    )
    .unwrap();
    Command::cargo_bin("filecleaner")
        .unwrap()
        .args(["validate", s3_toml.to_str().unwrap()])
        .assert()
        .success();

    let s3_bad_yaml = tmp.path().join("s3_bad.yaml");
    fs::write(
        &s3_bad_yaml,
        format!(
            r#"
rules:
  - name: pg-wal
    path: "{target_esc}"
    actions:
      - upload_s3
    s3:
      bucket: db-archive
      region: eu-west-3
      endpoint: http://minio.local:9000
"#
        ),
    )
    .unwrap();
    Command::cargo_bin("filecleaner")
        .unwrap()
        .args(["validate", s3_bad_yaml.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("allow_insecure_http"));
}
