# Error, In-Use Probe & Alert Matrix

Exhaustive failure taxonomy, active-file probe signals, safety states, and remediation runbook mapping grounded
in `src/error.rs`, `src/in_use.rs`, `src/policy.rs`, `src/s3.rs`, `src/hooks.rs`, and `src/daemon.rs`.

## 1. Domain Error Variants (`FileCleanerError`, `src/error.rs`)

| Variant | Exit Code | Trigger Condition | Remediation Runbook |
| :--- | :---: | :--- | :--- |
| `ConfigParse { path, reason }` | `2` | Malformed TOML or YAML syntax in configuration file. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `ConfigFormat { path, extension }` | `2` | Configuration extension is not `.toml`, `.yaml`, or `.yml`. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `ConfigValidation(String)` | `2` | Any `INV-CFG-01`..`INV-CFG-14` semantic rule violated, unknown `--rule` name, or conflicting ad-hoc CLI flags. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `InvalidGlob { pattern, source }` | `2` | Malformed `globset` pattern in `include_globs` or `exclude_globs`. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `InvalidRegex { pattern, source }` | `2` | Malformed Rust `regex` in `include_regex`, `exclude_regex`, or `filename_date_regex`. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `InvalidDuration(String)` | `2` | Unparseable duration string (`parse_duration_str`) or `u64` millisecond overflow. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `InvalidByteSize(String)` | `2` | Unparseable byte literal (`parse_bytes`), negative sign, `NaN`, or `u64` overflow. | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| `Io { path, source }` | `1` | Unreadable directory/file, permission denied, symlink detected when `follow_symlinks = false`, missing `birthtime` (`Created`), or overwrite blocked (`AlreadyExists`). | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| `Serialization(String)` | `1` | JSON/JSONL formatting failure or poisoned audit logger mutex. | [Run Reactive Daemon and Rotate Logs](../../02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md) |
| `HookFailed { rule, command, exit_code, stderr }` | `1` | Pre-delete hook exited non-zero (excluding `skip_exit_code`) or failed to spawn under `abort_file`/`abort_rule`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| `HookTimedOut { rule, command, timeout_secs }` | `1` | Hook exceeded `hook.timeout`; process tree killed via `-pgid` `SIGKILL` and `child.kill()`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| `InUseProbe(String)` | `1` | Sub-system in-use probe failure outside per-file `ProbeError` isolation. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| `Compression { path, reason }` | `1` | Pure-Rust `gzip`/`lz4`/`brotli`/`zip` encoder or I/O stream failure (source file preserved intact). | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| `IntegrityMismatch { path, expected, actual }` | `1` | SHA-256 or byte-length mismatch on cross-FS `Move` re-read, S3 `HeadObject` checksum, S3 `GetObject` readback, or pre-upload size mutation. | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| `BackupGuardViolation { rule, reason }` | `1` | Explicit backup safety invariant breach. | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| `Daemon(String)` | `4` | Live `pid_file` already claimed, foreign PID mismatch during `daemon stop`, invalid state JSON, or `stop` timeout exceeded. | [Run Reactive Daemon and Rotate Logs](../../02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md) |
| `S3Upload { path, uri, reason }` | `1` | Missing `access_key_env`/`secret_key_env`, TLS/HTTP error, `4xx`/`5xx` after retries, missing `<UploadId>`, or empty key. Local file preserved intact. | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |

---

## 2. Six-Layer In-Use Probe Triggers (`InUseReason`, `src/in_use.rs`)

Any trigger places the candidate into $S_{\text{busy}}$ (`DecisionKind::SkipInUse` during sweeps; exit code `10` in
`filecleaner check-in-use`).

| Layer | `InUseReason` Variant (`kind` tag) | OS Scope | Trigger Condition | Remediation / Operational Action |
| :---: | :--- | :--- | :--- | :--- |
| **1** | `sidecar_lock` (`SidecarLock`) | All | Sibling `<path><suffix>` exists (e.g., `.lock`, `.lck`, `.part`, `.tmp`, `.processing`). | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **1** | `temp_suffix` (`TempSuffix`) | All | Candidate filename ends with an entry in `in_use.sidecar_suffixes`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **2** | `os_flock` (`OsFlock`) | Unix | `libc::flock` (`LOCK_EX \| LOCK_NB` live, `LOCK_SH \| LOCK_NB` dry-run) returned `EWOULDBLOCK`/`EAGAIN`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **3** | `proc_fd_open` (`ProcFdOpen { pid }`) | Linux | Accessible `/proc/<pid>/fd/*` open descriptor resolves to candidate `(st_dev, st_ino)`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **4** | `windows_share_violation` (`WindowsShareViolation`) | Windows | `OpenOptions` (`share_mode(0)` live, `share_mode(1)` dry-run) failed with Win32 `32` (`ERROR_SHARING_VIOLATION`) or `33` (`ERROR_LOCK_VIOLATION`). | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **5** | `min_idle_age` (`MinIdleAge { age_ms, min_idle_ms }`) | All | `now - mtime < min_idle_age` or future-skewed `mtime > now` (`age_ms = 0`). | [Run Reactive Daemon and Rotate Logs](../../02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md) |
| **6** | `size_or_mtime_changed` (`SizeOrMtimeChanged`) | All | Two-point snapshot `(size, mtime)` diverged across `stability_window`. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **All** | `probe_error` (`ProbeError { reason }`) | All | Candidate stat/open/sidecar error during probe; fails closed as busy. | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |

---

## 3. Operational Alert, Freeze & Safety Matrix

| Alert / Audit Event | Severity | Observable Signal / Condition | Exit Code | Remediation Runbook |
| :--- | :--- | :--- | :---: | :--- |
| **`BackupFreeze` (`RuleOutcome::Frozen`)** | Critical | `audit.jsonl` `event: "backup_freeze"`, `state_file` `active_freezes` non-empty. Caused by zero files `>= min_valid_backup_bytes`, `require_newer_within` breach, or `freeze_if_newest_older_than` breach. | `0` (`3` with `--fail-on-freeze`) | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| **`RuleConstrained` (`RuleOutcome::Constrained`)** | Warning | `audit.jsonl` `event: "rule_constrained"`, `state_file` `constrained_rules` non-empty. `max_files` or `max_total_size` exceeded because $P_{\text{floor}}$, $S_{\text{busy}}$, or in-place actions (`upload_s3` without `delete`) blocked eviction. | `0` (`3` with `--strict-constraints`) | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| **S3 Checksum / Readback Mismatch** | Critical | `IntegrityMismatch` on `upload_s3`: missing `x-amz-checksum-sha256` header or digest divergence. Local artifact untouched; subsequent `delete`/`move` skipped. | `1` | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| **S3 Preflight Collision (`AlreadyExists`)** | Warning | `overwrite = false` detected `200 OK` on preflight `HeadObject` for target `s3://<bucket>/<key>`. | `1` | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| **S3 Credentials / Endpoint Failure** | Critical | Missing `access_key_env` / `secret_key_env`, TLS certificate mismatch, HTTP `403`, or `5xx` retries exhausted. | `1` | [Remediate Backup Freeze or S3 Failure](../../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md) |
| **Symlink-Swap / Reparse Rejection** | Critical | Pre-execute check or `open_regular_nofollow` detected symlink or `(st_dev, st_ino)` swap (`follow_symlinks = false`). | `1` | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **`FilenameDate` Parse Skip (`skip_ts_parse`)** | Warning | `audit.jsonl` `decision: "skip_ts_parse"` and `ScanReport.skipped_timestamp_parse` non-empty; file protected from eviction fail-closed. | `0` | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| **`TimeBasis::Created` Unsupported** | High | Filesystem/kernel returned `ErrorKind::Unsupported` for `birthtime`; rule aborted without falling back to `mtime`. | `1` | [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md) |
| **Hook Timeout or `AbortRule`** | High | `audit.jsonl` `event: "hook_exec"` `decision: "aborted"`; child process group killed on `hook.timeout` or non-skip failure. | `1` | [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md) |
| **Daemon PID / Stop Timeout / Audit Growth** | Warning | `daemon status` reports `stale_pid = true` or stopped (exit `4`); `daemon stop` refuses recycled non-`filecleaner` PID or hits `--timeout`. | `4` | [Run Reactive Daemon and Rotate Logs](../../02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md) · [Deploy with Docker and systemd](../../02-how-to-guides/deployment/deploy-filecleaner-docker-and-systemd.md) |

---

## Related Reference and Architecture

- Schema and validation invariants (`INV-CFG-01`..`INV-CFG-14`):
  [Configuration Schema & Invariants](../configuration/config-schema-and-invariants.md)
- Exit codes and audit schemas:
  [CLI Commands & Exit Codes](../cli-and-scripts/cli-commands-and-exit-codes.md)
- Formal pipeline & mathematical safety proofs:
  [Fail-Closed Six-Phase Pipeline](../../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
  · [Retention Calculus and TOCTOU Defenses](../../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
