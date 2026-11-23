# Configuration Schema & Validation Invariants

Normative schema specification for FileCleaner `v1.1.0` configuration files (`src/config.rs`, `src/scanner.rs`,
`src/s3.rs`, `src/actions.rs`).

## 1. File Format Detection & Loaders

`Config::from_file(path)` selects the parser exclusively from the lowercase filename extension;
`Config::load_validated(path)` chains `Config::from_file(path)` with `Config::validate()`.

| Extension | Parser Crate | Failure Mode on Unknown / Missing Extension |
| :--- | :--- | :--- |
| `.toml` | `toml::from_str` | — |
| `.yaml`, `.yml` | `serde_yaml_ng::from_str` | — |
| Any other / empty | None (no heuristic fallback) | `FileCleanerError::ConfigFormat { path, extension }` (exit code `2`) |

All enumerations serialize as `snake_case` by default and accept `PascalCase` `serde` aliases during deserialization.

---

## 2. Top-Level `Config` Keys

| Field | Rust Type | Default | Description & Resolution Rules |
| :--- | :--- | :--- | :--- |
| `dry_run` | `bool` | `false` | Global read-only simulation flag preventing filesystem and network mutations across all rules. |
| `audit_log` | `Option<PathBuf>` | `None` | Top-level shorthand JSONL audit path. `Config::effective_audit_log()` prefers `[audit].log_path` over `audit_log`; CLI `--audit-log` overrides both. |
| `daemon` | `DaemonConfig` | `DaemonConfig::default()` | Daemon scheduling mode, polling interval, FS watcher debounce, and PID/state/stop file paths. |
| `audit` | `AuditConfig` | `AuditConfig::default()` | Structured UTF-8 JSONL audit logger settings. |
| `global_trash_dir` | `Option<PathBuf>` | `None` | Fallback quarantine directory used when a rule selects `trash` without setting rule-local `trash_dir`. |
| `s3` | `Option<S3Config>` | `None` | Fallback S3 / S3-compatible archival sink block. `Config::effective_s3(rule)` prefers `rule.s3` over global `s3`. |
| `rules` | `Vec<RuleConfig>` | **Required** (non-empty) | Ordered list of independent directory retention and lifecycle rules (`INV-CFG-01`). |

---

## 3. `[daemon]` (`DaemonConfig`) and `DaemonMode`

| Field | Rust Type | Default | Alias | Constraints & Semantics |
| :--- | :--- | :--- | :--- | :--- |
| `mode` | `DaemonMode` | `"hybrid"` | — | Allowed: `"interval"` (`Interval`), `"notify"` (`Notify`), `"hybrid"` (`Hybrid`). |
| `interval` | `Duration` | `"60s"` | `poll_interval` | Must satisfy `interval > 0` (`INV-CFG-12`). Acts as sweep period in `interval` and fallback tick in `hybrid`. |
| `watch` | `bool` | `false` | — | Setting `watch = true` when `mode = "interval"` promotes effective runtime mode to `Hybrid`. |
| `debounce` | `Duration` | `"2s"` | — | Coalescing window batching filesystem watcher bursts before triggering a sweep. |
| `sweep_on_start` | `bool` | `true` | — | Executes an immediate sweep on daemon startup before waiting for the first tick or watcher event. |
| `pid_file` | `Option<PathBuf>` | `None` | — | Atomic `0o600` PID lock file path checked on startup and removed on clean shutdown; excluded from rule scans. |
| `state_file` | `Option<PathBuf>` | `None` | — | Atomic `0o600` live JSON status output path; excluded from rule scans. |
| `stop_file` | `Option<PathBuf>` | `None` | — | Cooperative shutdown trigger file polled every `50 ms` during run and `200 ms` during `daemon stop`; excluded from rule scans. |

---

## 4. `[audit]` (`AuditConfig`)

| Field | Rust Type | Default | Constraints & Semantics |
| :--- | :--- | :--- | :--- |
| `log_path` | `Option<PathBuf>` | `None` | Destination path for synchronous UTF-8 JSONL audit records (`0o600` Unix mode). Takes precedence over top-level `audit_log`. |
| `jsonl` | `bool` | `true` | Enables JSON Lines emission when `log_path` or `audit_log` is configured. |

Compiled rotation constant (`src/audit.rs`): `DEFAULT_AUDIT_ROTATE_BYTES = 52_428_800` (`50 MiB`), rotating
single-generation to `<audit_log>.1` (`0o600`). Both the primary log and `<audit_log>.1` are excluded from rule
discovery.

---

## 5. `[[rules]]` (`RuleConfig`)

| Field | Rust Type | Default | Constraints & Semantics |
| :--- | :--- | :--- | :--- |
| `name` | `String` | **Required** | Trimmed value must be non-empty and unique across all rules (`INV-CFG-02`). |
| `path` | `PathBuf` | **Required** | Non-empty root directory monitored by this rule (`INV-CFG-03`). |
| `enabled` | `bool` | `true` | Disabled rules (`false`) are skipped during sweeps while still undergoing static `Config::validate()` checks. |
| `recursive` | `bool` | `false` | Traverses subdirectories when `true`; restricts `WalkDir` to `max_depth(1)` (immediate children) when `false`. |
| `max_depth` | `Option<usize>` | `None` | Maximum directory recursion depth (applied only when `recursive = true`). |
| `follow_symlinks` | `bool` | `false` | Symbolic link traversal flag. Defaults to `false` (fail-closed symlink skip and `O_NOFOLLOW` / reparse-point guards). |
| `include_globs` | `Vec<String>` | `[]` | `globset` inclusion patterns matched against normalized `/` relative paths and basenames (empty selects all files). |
| `exclude_globs` | `Vec<String>` | `[]` | `globset` exclusion patterns; unconditionally override includes and prune matching directories during traversal. |
| `include_regex` | `Option<String>` | `None` | Rust `regex::Regex` inclusion pattern matched against normalized relative paths or basenames. |
| `exclude_regex` | `Option<String>` | `None` | Rust `regex::Regex` exclusion pattern; unconditionally overrides includes. |
| `min_file_size` | `Option<u64>` | `None` | Lower file-size filter (`parse_bytes`). Files with `size < min_file_size` are excluded from candidate selection. |
| `max_file_size` | `Option<u64>` | `None` | Upper file-size filter (`parse_bytes`). Must satisfy `min_file_size <= max_file_size` (`INV-CFG-07`). |
| `time_basis` | `TimeBasis` | `"modified"` | Timestamp source: `"modified"` (`mtime`), `"created"` (`birthtime`, no fallback), or `"filename_date"` (`FilenameDate`). |
| `filename_date_regex` | `Option<String>` | `None` | Compiled whenever non-empty; required for `time_basis = "filename_date"`. Uses named group `date`, else group `1`, else `0`. |
| `filename_date_format` | `Option<String>` | `None` | `chrono` `strftime` format string; required and non-empty when `time_basis = "filename_date"` (`INV-CFG-06`). |
| `max_files` | `Option<usize>` | `None` | Maximum retained candidate count. Must satisfy `max_files >= backup_guard.min_keep_files` (`INV-CFG-07`). |
| `max_age` | `Option<Duration>` | `None` | Maximum candidate age (`now - t(f) > max_age` triggers eviction eligibility). |
| `max_total_size` | `Option<u64>` | `None` | Maximum aggregate candidate byte budget (`parse_bytes`) across the rule pool. |
| `combine_mode` | `CombineMode` | `"any"` | `"any"` (evict if any active threshold matches) or `"all"` (evict only when every active threshold matches). |
| `eviction_order` | `EvictionOrder` | `"oldest_first"` | `"oldest_first"` ($t(f)$ ascending) or `"largest_first"` (`size(f)` descending, ties oldest first). |
| `dry_run` | `bool` | `false` | Forces dry-run simulation for this rule regardless of global `dry_run`. |
| `backup_guard` | `BackupGuardConfig` | Default | Safety floor and staleness dead-man's-switch freeze settings. |
| `in_use` | `InUseConfig` | Default | 6-layer active-writer and lock-detection settings. |
| `pre_delete_hook` | `Option<HookConfig>` | `None` | Synchronous external veto command executed prior to actions (alias: `pre_hook`). |
| `actions` | `Vec<ActionStep>` | `["delete"]` | Non-empty ordered pipeline: `"rename_by_date"`, `"compress"`, `"move"`, `"copy_truncate"`, `"upload_s3"`, `"trash"`, `"delete"`. |
| `move_to` | `Option<PathBuf>` | `None` | Required destination directory when `actions` includes `"move"`; fallback destination for `"copy_truncate"`. |
| `move_date_subdir` | `Option<String>` | `None` | Validated `chrono` `strftime` subdirectory template appended to `move_to` (e.g., `"%Y/%m"`). Must not resolve `..`. |
| `move_overwrite` | `bool` | `false` | Permits overwriting an existing target during `"move"`. |
| `trash_dir` | `Option<PathBuf>` | `None` | Rule-local quarantine directory for `"trash"` (overrides `global_trash_dir`). |
| `compression` | `CompressionFormat` | `"gzip"` | Pure-Rust codec for `"compress"`: `"gzip"`, `"lz4"`, `"brotli"`, `"zip"`. |
| `compression_level` | `Option<u32>` | `None` | Codec level: clamped to `0..=9` (`gzip`, `zip`), `0..=11` default `6` (`brotli`); ignored by `lz4`. |
| `compress_remove_source` | `bool` | `true` | Removes original source file only after staged compression atomically commits. |
| `compress_overwrite` | `bool` | `false` | Permits overwriting an existing compressed destination archive. |
| `copy_truncate_to` | `Option<PathBuf>` | `None` | Destination directory for `"copy_truncate"` (falls back to `move_to`). |
| `rename_template` | `Option<String>` | `None` | Validated `chrono` `strftime` stem suffix template for `"rename_by_date"` and `"copy_truncate"` (runtime default `"%Y%m%d_%H%M%S"`). |
| `s3` | `Option<S3Config>` | `None` | Rule-local S3 archival sink configuration (overrides global `[s3]`). |

---

## 6. `[rules.backup_guard]` (`BackupGuardConfig`)

| Field | Rust Type | Default | Alias | Constraints & Semantics |
| :--- | :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `false` | — | Master switch. Note: configuring any non-default threshold (`min_keep_files > 0`, `min_valid_backup_bytes > 0`, `require_newer_within`, or `freeze_if_newest_older_than`) also activates guard evaluation. |
| `min_keep_files` | `usize` | `0` | — | Number of newest valid backups (`size >= min_valid_backup_bytes`) strictly protected in $P_{\text{floor}}$. |
| `min_valid_backup_bytes` | `u64` | `0` | `min_valid_size` | Minimum byte size (`parse_bytes`) required to enter $\mathcal{F}_{\text{valid}}$; excludes 0-byte or stub files from freshness and floor checks. |
| `require_newer_within` | `Option<Duration>` | `None` | `max_newest_age` | Dead-man's-switch freshness window; triggers `RuleOutcome::Frozen` if `now - newest_valid_ts > require_newer_within`. |
| `freeze_if_newest_older_than` | `Option<DateTime<Utc>>` | `None` | `min_newest_timestamp` | RFC 3339 UTC timestamp cutoff; triggers `RuleOutcome::Frozen` if `newest_valid_ts < cutoff`. |
| `freeze_on_empty_valid` | `bool` | `true` | `freeze_on_empty` | Triggers `RuleOutcome::Frozen` when guard is active and zero candidates satisfy `min_valid_backup_bytes`. |

---

## 7. `[rules.in_use]` (`InUseConfig`)

| Field | Rust Type | Default | Constraints & Semantics |
| :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `true` | Master toggle for the 6-layer active-writer detection matrix. |
| `check_sidecar_locks` | `bool` | `true` | Enables Layer 1 matching of active suffixes on candidate basenames and `<path><suffix>` sibling lock files. |
| `sidecar_suffixes` | `Vec<String>` | `[".lock", ".lck", ".part", ".tmp", ".processing"]` | Suffix list checked by Layer 1 (empty strings ignored). |
| `check_os_locks` | `bool` | `true` | Enables Layer 2 (Unix `flock` via `O_RDONLY`: `LOCK_EX \| LOCK_NB` live, `LOCK_SH \| LOCK_NB` dry-run) and Layer 4 (Windows `share_mode(0)` live, `share_mode(1)` dry-run). Skipped on $P_{\text{floor}}$ or already-flagged candidates. |
| `check_proc_fd` | `bool` | `true` | Enables Layer 3 (Linux `/proc/<pid>/fd` single-pass sweep inode snapshot matching `(st_dev, st_ino)`). |
| `min_idle_age` | `Option<Duration>` | `None` | Enables Layer 5 cooldown requiring `now - mtime >= min_idle_age` (also protects future-skewed `mtime > now` fail-closed). |
| `stability_window` | `Option<Duration>` | `None` | Enables Layer 6 batched two-point `(size, mtime)` stability verification across Layer 1–5 survivors when `> 0`. |

---

## 8. `[rules.pre_delete_hook]` (`HookConfig`) and `HookFailurePolicy`

| Field | Rust Type | Default | Constraints & Semantics |
| :--- | :--- | :--- | :--- |
| `command` | `String` | **Required** | Trimmed non-empty executable path or binary name (`INV-CFG-11`). Invoked directly via `Command::new` (`argv`, never a shell). |
| `args` | `Vec<String>` | `[]` | Argument list supporting single-pass substitution tokens `{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`. |
| `timeout` | `Duration` | `"30s"` | Must be `> 0` (`INV-CFG-11`). Timed-out processes are terminated via Unix `-pgid` `SIGKILL` + `child.kill()` + `child.wait()`. |
| `on_failure` | `HookFailurePolicy` | `"abort_file"` | Allowed: `"abort_file"` (skip file, record error), `"abort_rule"` (halt rule, exit `1`), `"warn_and_continue"` (log warning, run actions). |
| `skip_exit_code` | `Option<i32>` | `Some(42)` | Child exit code treated as a clean per-file skip veto (`HookVerdict::SkippedByCode`, exit `0`). |
| `run_in_dry_run` | `bool` | `false` | Executes hook during `plan` or `--dry-run` when `true`; otherwise records `SimulatedDryRun` without spawning. |
| `working_dir` | `Option<PathBuf>` | `None` | Optional child working directory. |
| `env` | `BTreeMap<String, String>` | `{}` | Static environment variables merged with dynamic `FILECLEANER_*` variables (which override static keys on collision). |

### Injected `FILECLEANER_*` Environment Variables (`src/hooks.rs`)

| Variable | Value Format |
| :--- | :--- |
| `FILECLEANER_PATH` | Full candidate path |
| `FILECLEANER_NAME` | Candidate file basename |
| `FILECLEANER_DIR` | Candidate parent directory path (empty string if absent) |
| `FILECLEANER_SIZE` | File size in decimal bytes |
| `FILECLEANER_MTIME_ISO` | Candidate `mtime` formatted in RFC 3339 UTC (`SecondsFormat::Secs`, `Z` suffix) |
| `FILECLEANER_RULE` | Evaluating rule `name` |
| `FILECLEANER_ACTION` | Arrow-joined pipeline summary (`"compress(Gzip)->move"`, `"delete"`, etc.) |
| `FILECLEANER_DRY_RUN` | `"true"` or `"false"` |

---

## 9. `[s3]` and `[rules.s3]` (`S3Config`) & `S3VerifyMode`

Plaintext credential fields are structurally absent from `S3Config`; keys resolve strictly from environment variables
(`INV-CFG-14`).

| Field | Rust Type | Default | Constraints & Semantics |
| :--- | :--- | :--- | :--- |
| `bucket` | `String` | **Required** | Non-empty bucket name; forbids `/`, `\`, `..`, whitespace, and control characters (`INV-CFG-13`). |
| `region` | `String` | **Required** | Non-empty AWS or S3-compatible signing region; forbids `/`, whitespace, and control characters (`INV-CFG-13`). |
| `endpoint` | `Option<String>` | `None` | Custom S3 endpoint URL. Must start with `https://` unless `allow_insecure_http = true`; forbids `@`, `?`, `#`, whitespace, control chars, and empty host. |
| `prefix` | `Option<String>` | `None` | Static key prefix; forbids `\`, `..` segments, and control characters. |
| `key_date_subdir` | `Option<String>` | `None` | Validated non-empty `chrono` `strftime` segment appended after `prefix`; forbids `\`, `..`, and control characters. |
| `force_path_style` | `bool` | `false` | Uses `<endpoint>/<bucket>/<key>` (or `https://s3.<region>.amazonaws.com/<bucket>/<key>`) when `true`; virtual-hosted style when `false`. |
| `allow_insecure_http` | `bool` | `false` | Explicit opt-in permitting `http://` custom endpoints (TLS enforced when `false`, `max_redirects(0)`). |
| `overwrite` | `bool` | `false` | When `false`, runs signed `HeadObject` preflight and fails with `Io(AlreadyExists)` if the remote key returns `200 OK`. |
| `storage_class` | `Option<String>` | `None` | Non-empty `x-amz-storage-class` header value without control characters (e.g., `"STANDARD_IA"`, `"GLACIER"`). |
| `verify_mode` | `S3VerifyMode` | `"checksum_sha256"` | `"checksum_sha256"` (`HeadObject` `x-amz-checksum-mode: ENABLED` + `Content-Length`) or `"readback_sha256"` (streaming `GetObject` SHA-256 verification). |
| `access_key_env` | `String` | `"AWS_ACCESS_KEY_ID"` | Environment variable name holding S3 access key ID (non-empty; no `=`, whitespace, or control chars). |
| `secret_key_env` | `String` | `"AWS_SECRET_ACCESS_KEY"` | Environment variable name holding S3 secret access key (wrapped in `RedactedSecret` at runtime). |
| `session_token_env` | `Option<String>` | `Some("AWS_SESSION_TOKEN")` | Optional environment variable name holding temporary `x-amz-security-token`. |
| `timeout` | `Duration` | `"60s"` | Global HTTP/TLS request timeout (`> 0`). |
| `max_retries` | `u32` | `3` | Maximum retry attempts on `5xx` or transport errors (`200 ms` exponential backoff capped at `5 s`). Never retries `4xx` or `IntegrityMismatch`. |
| `multipart_part_size` | `u64` | `"16MiB"` | Single-part vs multipart threshold and part byte size; bounded to `[5 MiB, 5 GiB]` (`MIN_S3_MULTIPART_PART_SIZE`..`MAX_S3_MULTIPART_PART_SIZE`). |

---

## 10. Enumerations Reference

| Enum | Serialized Values (`snake_case`) | Accepted `PascalCase` Aliases |
| :--- | :--- | :--- |
| `DaemonMode` | `interval`, `notify`, `hybrid` | `Interval`, `Notify`, `Hybrid` |
| `TimeBasis` | `modified`, `created`, `filename_date` | `Modified`, `Created`, `FilenameDate` |
| `CombineMode` | `any`, `all` | `Any`, `All` |
| `EvictionOrder` | `oldest_first`, `largest_first` | `OldestFirst`, `LargestFirst` |
| `HookFailurePolicy` | `abort_file`, `abort_rule`, `warn_and_continue` | `AbortFile`, `AbortRule`, `WarnAndContinue` |
| `ActionStep` | `rename_by_date`, `compress`, `move`, `copy_truncate`, `upload_s3`, `trash`, `delete` | `RenameByDate`, `Compress`, `Move`, `CopyTruncate`, `UploadS3`, `Trash`, `Delete` |
| `CompressionFormat` | `gzip` (`.gz`), `lz4` (`.lz4`), `brotli` (`.br`), `zip` (`.zip`) | `Gzip`, `Lz4`, `Brotli`, `Zip` |
| `S3VerifyMode` | `checksum_sha256`, `readback_sha256` | `ChecksumSha256`, `ReadbackSha256` |

---

## 11. Human-Readable Unit Grammars

### Byte Size Grammar (`parse_bytes`)

Accepts unsigned integer literals (`0`, `1048576`) or `<number>\s*<unit>` strings (case-insensitive; decimal
fractions such as `"1.5 GiB"` rounded to nearest `u64` with overflow check against `u64::MAX`). Rejects negative
values, empty strings, `NaN`/`inf`, multiple decimal points, or unknown suffixes with
`FileCleanerError::InvalidByteSize`.

| Suffix | Standard | Multiplier (Bytes) |
| :--- | :--- | :--- |
| *(none)*, `B` | Raw Bytes | `1` |
| `KB` | SI Decimal | `1,000` ($10^3$) |
| `K`, `KiB` | IEC Binary | `1,024` ($2^{10}$) |
| `MB` | SI Decimal | `1,000,000` ($10^6$) |
| `M`, `MiB` | IEC Binary | `1,048,576` ($2^{20}$) |
| `GB` | SI Decimal | `1,000,000,000` ($10^9$) |
| `G`, `GiB` | IEC Binary | `1,073,741,824` ($2^{30}$) |
| `TB` | SI Decimal | `1,000,000,000,000` ($10^{12}$) |
| `T`, `TiB` | IEC Binary | `1,099,511,627,776` ($2^{40}$) |
| `PB` | SI Decimal | `1,000,000,000,000,000` ($10^{15}$) |
| `P`, `PiB` | IEC Binary | `1,125,899,906,842,624` ($2^{50}$) |

### Duration Grammar (`parse_duration_str`)

Accepts bare non-negative integer seconds (`"60"`) or single/compound `<uint><unit>` sequences (`"250ms"`,
`"2m30s"`, `"1d 12h"`). Checked overflow against `u64::MAX` milliseconds; rejects negative signs, empty strings, or
unknown units with `FileCleanerError::InvalidDuration`.

| Token | Unit | Milliseconds |
| :--- | :--- | :--- |
| `ms` | Milliseconds | `1` |
| `s` | Seconds | `1,000` |
| `m` | Minutes | `60,000` |
| `h` | Hours | `3,600,000` |
| `d` | Days (`24h`) | `86,400,000` |
| `w` | Weeks (`7d`) | `604,800,000` |

---

## 12. Static Validation Invariants (`Config::validate`)

| Invariant ID | Enforced Condition (`src/config.rs`) | Returned Error Variant |
| :--- | :--- | :--- |
| `INV-CFG-01` | `rules` non-empty; `audit_log` and `global_trash_dir` (when `Some`) non-empty. | `ConfigValidation` |
| `INV-CFG-02` | Every `rule.name.trim()` is non-empty and unique across `rules`. | `ConfigValidation` |
| `INV-CFG-03` | `rule.path` non-empty and `rule.actions` non-empty. | `ConfigValidation` |
| `INV-CFG-04` | Non-empty `include_regex`, `exclude_regex`, and `filename_date_regex` compile via `regex::Regex::new`. | `InvalidRegex` |
| `INV-CFG-05` | All entries in `include_globs` and `exclude_globs` compile via `globset::Glob::new`. | `InvalidGlob` |
| `INV-CFG-06` | `time_basis = "filename_date"` has both `filename_date_regex` and non-empty `filename_date_format` without `Item::Error`; `move_date_subdir` and `rename_template` (when `Some`) are non-empty valid `strftime` patterns. | `ConfigValidation` |
| `INV-CFG-07` | If `max_files` is `Some`, `max_files >= backup_guard.min_keep_files`; if both `min_file_size` and `max_file_size` are `Some`, `min_file_size <= max_file_size`. | `ConfigValidation` |
| `INV-CFG-08` | `Move` requires `move_to`; `CopyTruncate` requires `copy_truncate_to` or `move_to`; `Trash` requires `trash_dir` or `global_trash_dir`; `UploadS3` requires `rule.s3` or global `s3`. | `ConfigValidation` |
| `INV-CFG-09` | **Self-Containment Guard**: When `rule.recursive = true`, `move_to`, `copy_truncate_to`, and effective `trash_dir` must not equal or nest inside `rule.path` (resolved lexically/canonically) unless matched by `exclude_globs` or `exclude_regex`. | `ConfigValidation` |
| `INV-CFG-10` | **Terminal Action Ordering**: Terminal steps (`Delete`, `Trash`) must appear last in `rule.actions`. | `ConfigValidation` |
| `INV-CFG-11` | `pre_delete_hook` (when `Some`) has non-empty trimmed `command` and `timeout > 0`. | `ConfigValidation` |
| `INV-CFG-12` | `daemon.interval > 0`. (`CopyTruncate` opt-in rules additionally emit an explicit advisory warning in `filecleaner validate`.) | `ConfigValidation` |
| `INV-CFG-13` | `s3.bucket` and `s3.region` non-empty without unsafe characters; custom `s3.endpoint` enforces `https://` unless `allow_insecure_http = true` and forbids `@`, `?`, `#`, whitespace, or empty host; `prefix` and `key_date_subdir` forbid `\`, `..`, control chars; `key_date_subdir` is valid `strftime`; `storage_class` non-empty without control chars; `s3.timeout > 0`; `5 MiB <= s3.multipart_part_size <= 5 GiB`. | `ConfigValidation` |
| `INV-CFG-14` | `s3.access_key_env`, `s3.secret_key_env`, and optional `s3.session_token_env` are non-empty strings without `=`, whitespace, or control characters (zero plaintext credentials in config). | `ConfigValidation` |

---

## Related Reference and Procedures

- CLI options, ad-hoc flags, and JSON schemas:
  [CLI Commands & Exit Codes](../cli-and-scripts/cli-commands-and-exit-codes.md)
- Error taxonomy and alert-to-runbook mapping:
  [Error & Alert Matrix](../troubleshooting-codes/error-and-alert-matrix.md)
- Operational setup guides:
  [Configure Backup Retention and S3 Archival](../../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md)
  · [Enforce Pre-Delete Hooks and TOCTOU Guards](../../02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md)
- Architectural internals:
  [Fail-Closed Six-Phase Pipeline](../../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
  · [Retention Calculus and TOCTOU Defenses](../../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
