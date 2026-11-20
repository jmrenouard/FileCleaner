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
}
