# Changelog

All notable changes to **FileCleaner** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [1.1.0] - 2026-11-22

### Added

- **Pure-Rust S3 / S3-Compatible Archival Sink (`IDEA-001`, Issues `#24`, `#26`, `#28`)**:
  - Added non-terminal `ActionStep::UploadS3` (`upload_s3`), `S3VerifyMode` (`checksum_sha256` default, `readback_sha256`), `S3Config` (global `[s3]` and per-rule `[rules.s3]` resolved via `Config::effective_s3`), `FileCleanerError::S3Upload` (exit `1`), and static validation invariants `INV-CFG-08`/`10`/`13`/`14` (`src/config.rs`, `src/error.rs`).
  - Implemented `src/s3.rs` (`S3Uploader`, `HttpS3Uploader`, `RedactedSecret`) on a 100% pure-Rust stack (`ureq` v3 `default-features = false`, `rustls` v0.23 `default-features = false`, `rustls-rustcrypto`, `hmac`, `base64`) preserving `IMP-001` (zero C/C++ `*-sys`, `ring`, `aws-lc-*`, or `openssl-*` across all 9 release targets).
  - Enforced AWS Signature Version 4 (`AWS4-HMAC-SHA256`), streaming single-part `PutObject` and S3 Multipart Upload (`CreateMultipartUpload` / `UploadPart` / `CompleteMultipartUpload` with HTTP-200 `<Error>` inspection and automatic `AbortMultipartUpload`), non-overwrite `HeadObject` preflight, bounded `5xx` retries, and fail-closed SHA-256 verification (`HeadObject` `x-amz-checksum-mode: ENABLED` with composite `<b64>-<N>` matching, or streaming `GetObject` `readback_sha256`) before any local `Delete`, `Trash`, or `Move` executes.
  - Integrated `UploadS3` into `ActionExecutor::execute_pipeline_with_s3` (`src/actions.rs`) with TOCTOU-safe `open_regular_nofollow_read`, zero-I/O dry-run URI projection, `SweepEngine::run_config_with_s3` (`src/engine.rs`), structured JSONL `sha256` & `details` (`s3_uri`, `etag`, `verify_mode`, `parts_uploaded`, `verified_size`), and loopback integration suite (`tests/s3_archival_sink_test.rs`).
- **DBA PostgreSQL WAL & Base-Backup Retention Profile (Issue `#30`)**:
  - Added `filecleaner init --profile dba-postgres` (TOML and YAML, `dry_run = true`) in `src/cli.rs`.
  - Shipped validated production examples `examples/dba-postgres-wal-s3.toml` and `examples/dba-postgres-wal-s3.yaml`, plus executable PITR/standby-replica boundary pre-delete hook `examples/hooks/verify_pg_wal_safe.sh` (`export LC_ALL=C`, exit `42` veto on held/protected/non-WAL artifacts, fail-closed exit `1` on malformed markers).
  - Added end-to-end DBA workflow verification (`howto_workflow_5_dba_postgres_wal_and_base_backups_s3_archival_and_pitr_guards`) in `tests/product_workflows_e2e_test.rs`.
- **Reproducible Live Docker E2E Verification Harness (Issue `#32`)**:
  - Delivered `scripts/e2e-dba-s3-lab.sh` and `tests/e2e_dba_s3_docker_test.rs` exercising all 6 DBA PostgreSQL 17 (`postgres:17-bookworm`, compatible with `jmrenouard/multi-db-docker-env`) + pinned MinIO (`pgsty/minio:RELEASE.2026-08-04T00-00-00Z`) scenarios live, including real 16 MiB WAL archival, active-writer `check-in-use` exit `10`, MinIO outage and corrupt-checksum fault injection, 0-byte stub `--fail-on-freeze` exit `3`, and MinIO download + SHA-256 `audit.jsonl` parity + full 253-row PostgreSQL 17 restore.
- **Synchronized Bilingual EN/FR Documentation & Governance (Issues `#34`, `#36`)**:
  - Updated `docs/en/SPECIFICATIONS.md`, `docs/fr/SPECIFICATIONS.md`, `docs/en/COOKBOOK_EXAMPLES.md`, `docs/fr/EXEMPLES_RECETTES.md` (Recipe 5), `docs/en/USER_GUIDE.md`, `docs/fr/GUIDE_UTILISATEUR.md`, `docs/en/HOWTO.md`, `docs/fr/HOWTO.md`, `README.md`, and `README.fr.md` with automated heading parity across all 5 EN/FR pairs in `tests/governance_and_docs_test.rs`.
  - Marked `IDEA-001` shipped in `docs/RCA_REGISTER.md` (`APP-003`, `RCA-009`) and logged `ACT-010`..`ACT-016` in `docs/ACTION_LOGBOOK.md`.

---

## [1.0.1] - 2026-11-20

### Added & Hardened

- **Multi-Arch Docker Hub Container Packaging & CI Publishing (`Dockerfile`, `.dockerignore`, `.github/workflows/docker-publish.yml`)**: Added hardened multi-stage Alpine/Debian-slim container (`rust:1.100.0-bookworm` builder -> non-root `debian:bookworm-slim` runtime UID/GID `10001` `fcuser:fcgroup`, `HEALTHCHECK`, `VOLUME` mountpoints, bundled `examples/`) and SHA-pinned Buildx workflow publishing `jmrenouard/filecleaner` (`latest`, `v1.0.1`, `1.0.1`) across `linux/amd64` and `linux/arm64` on release publication or manual dispatch.
- **Enterprise Release Notes (`docs/RELEASE_NOTES_v1.0.1.md`) & Bilingual Sync**: Updated `README.md`, `README.fr.md`, and EN/FR guides with `v1.0.1` binary archives and Docker Hub Quickstart commands.

### Post-v1.0.1 Audit, Coverage & Documentation (Issue #20)

- **Hook Stderr Drain Protection (`src/hooks.rs`)**: Replaced unbounded/channel stderr forwarding with a bounded `Arc<Mutex<Vec<u8>>>` buffer (`MAX_HOOK_STDERR_BYTES = 64 KiB`, UTF-8 char-boundary safe truncation) and bounded EOF drain grace (`STDERR_DRAIN_GRACE`) so failing hooks that spawn lingering background grandchildren still preserve their diagnostic `stderr` without blocking.
- **Template Validation & Ad-Hoc Rename Flag (`src/config.rs`, `src/cli.rs`)**: Validated non-empty `rule.rename_template` `strftime` formats eagerly inside `Config::validate()`, and automatically activated `ActionStep::RenameByDate` when `--rename-template` is passed without `--rename-by-date` in ad-hoc CLI sweeps.
- **Process Identity Guard (`src/daemon.rs`)**: Verified `is_live_filecleaner_pid(pid)` (`is_pid_alive(pid) && is_filecleaner_process(pid)`) in both `daemon_status` and `claim_pid_file`, treating recycled non-FileCleaner PIDs as stale rather than active daemons.
- **Bilingual 5-Minute HOWTO & Schema Alignment (`docs/en/HOWTO.md`, `docs/fr/HOWTO.md`, `docs/`)**: Added beginner-friendly EN/FR quick-start HOWTO guides with automated heading parity and config validation tests, added cross-links across READMEs/User Guides/Cookbooks, and corrected the `check-in-use --json` wrapper schema example (`all_safe`, `busy_count`, `reports`).
- **Expanded Verification & Coverage (`tests/product_workflows_e2e_test.rs`)**: Added end-to-end HOWTO workflow tests and targeted unit/integration coverage lifting the workspace to `124` passing tests (`64` unit + `60` integration across `9` suites) and `94.31%` line coverage (`>= 90.21%` per `src/*.rs` module).

### Security

- **Symlink-Swap TOCTOU Hardening & Private Staging (`src/actions.rs`, `src/engine.rs`)**: Added non-following regular-file opens (`O_NOFOLLOW` plus `(st_dev, st_ino)` verification on Unix; `FILE_FLAG_OPEN_REPARSE_POINT` plus reparse-point attribute validation on Windows), `0o600` private `.fc-tmp` staging creation, `copy_truncate` pre/post inode continuity checks, and pre-execute symlink verification before running live action pipelines.
- **Private `0o600` Runtime Files & Audit Rotation (`src/audit.rs`, `src/daemon.rs`, `src/cli.rs`, `src/scanner.rs`)**: Enforced `0o600` Unix permissions on `audit.jsonl`, `pid_file`, `state_file`, `stop_file`, and `filecleaner init` outputs, plus `50 MiB` single-generation `<audit_log>.1` audit log rotation (`DEFAULT_AUDIT_ROTATE_BYTES`) with `.1` excluded from candidate scanning and daemon watching.
- **Hook Isolation & Pipe Deadlock Prevention (`src/hooks.rs`)**: Switched hook child `stdout` to `Stdio::null()`, capped captured `stderr` at `64 KiB` via a bounded `sync_channel` with `50 ms` drain grace (preventing background grandchildren from deadlocking pipe EOF), isolated Unix children in `process_group(0)`, and terminated full process groups on timeout (`SIGKILL`).
- **Daemon `stop` PID Identity Guard (`src/daemon.rs`)**: Verified that any live PID referenced by `pid_file` belongs to a `filecleaner` process (`is_filecleaner_process` via `/proc` on Linux, `ps` on non-Linux Unix, or `QueryFullProcessImageNameW` on Windows) before writing `stop_file` or sending `SIGTERM`.
- **GitHub Actions Supply-Chain Hardening (`.github/workflows/ci.yml`, `.github/workflows/release.yml`)**: Pinned every `uses:` action to an immutable 40-character commit SHA, moved `${{ ... }}` matrix/ref values into `env:` out of `run:` scripts, and restricted `contents: write` strictly to `publish-release`.

### Fixed

- **Idempotent Multi-Sweep `Compress` & `RenameByDate` (`src/actions.rs`, `src/scanner.rs`)**: Prevented repeated daemon sweeps from looping into `_date_date` or `.gz.gz` on rules without terminal removal steps (`has_compressed_extension`, `has_trailing_date_stamp`).
- **Accurate Quota Accounting for In-Place Actions (`src/policy.rs`)**: Distinguished file-removing (`Delete`, `Trash`, `Move`, `CopyTruncate` with `min_file_size > 0`) and byte-reclaiming actions in `RetentionEngine::plan` so `remaining_files`, `remaining_bytes`, and `RuleOutcome::Constrained` remain exact for `RenameByDate`, `Compress` (`compress_remove_source = false`), and `CopyTruncate`.

### Performance

- **Non-Intrusive & Read-Only In-Use Probing (`src/in_use.rs`, `src/engine.rs`, `src/cli.rs`)**: Used non-exclusive `LOCK_SH | LOCK_NB` / `FILE_SHARE_READ` lock probing during dry-run/`plan`, evaluated sidecars, `/proc/*/fd`, and `min_idle_age` + protected floor $P_{\text{floor}}$ prior to OS lock probes, cached `/proc` inode snapshots once per sweep via `SweepLockProbe` (removing redundant `read_link` calls), batched `filecleaner check-in-use <PATH...>` via `InUseDetector::check_paths`, precompiled rule regex/glob matchers once per sweep/daemon run (`CompiledRuleMatchers`), and reduced daemon `STOP_POLL_SLICE` to `200 ms`.

### Changed

- **Lean Code Deduplication (`src/config.rs`, `src/scanner.rs`, `src/daemon.rs`, `src/cli.rs`)**: Unified lexical path normalization on `crate::config::resolve_path_lexically`, consolidated rule matcher compilation in `compile_rule_matchers`, and reused `daemon::write_atomic_bytes_with_mode` from `cli.rs` with zero new `Cargo.toml` dependencies.

---

## [1.0.0] - 2026-11-20

### Added

- **Fail-Closed 6-Phase Retention Engine (`Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`)**
  - Deterministic newest-first ordering with normalized POSIX relative-path tie-breaking.
  - `TimeBasis` support for `modified` (`mtime`), `created` (`birthtime`, fail-closed without silent fallback on unsupported filesystems), and `filename_date` (`regex` + `chrono` `strftime`, fail-closed skip on unparseable names).
  - Glob (`globset`) and Rust `regex` include/exclude filters, `min_file_size` / `max_file_size` bounds, `recursive` + `max_depth`, and safe `follow_symlinks = false` default.
  - Flexible retention caps (`max_files`, `max_age`, `max_total_size`) with `CombineMode::{Any, All}`, `EvictionOrder::{OldestFirst, LargestFirst}`, human-readable SI/IEC byte sizes (`B`..`PB`/`PiB`), and compound durations (`ms`, `s`, `m`, `h`, `d`, `w`).

- **Backup Safety Guardrails (`backup_guard`)**
  - Minimum valid backup threshold (`min_valid_backup_bytes`, alias `min_valid_size`) filtering out corrupt/0-byte stubs prior to floor or freshness checks.
  - Protected newest valid backup floor (`min_keep_files`) strictly excluded from eviction (`E ∩ P_floor = ∅`).
  - Dead-man's-switch staleness freezes (`require_newer_within`, `freeze_if_newest_older_than`, `freeze_on_empty_valid`).
  - Non-destructive constraint conflict resolution (`RuleOutcome::Constrained`) ensuring safety floors and active locks always win, with optional strict exit code `3` via `--strict-constraints` and `--fail-on-freeze`.

- **6-Layer In-Use File Detection Matrix (`in_use` & `check-in-use`)**
  - Sidecar and partial-transfer suffix detection (`.lock`, `.lck`, `.part`, `.tmp`, `.processing`, configurable).
  - Unix cooperative `flock(LOCK_EX | LOCK_NB)` via `O_RDONLY` without altering `mtime`.
  - Linux `/proc/*/fd` open file descriptor `(st_dev, st_ino)` scan with non-blocking diagnostic logging for unprivileged `EACCES`/`EPERM`.
  - Windows kernel-enforced `OpenOptionsExt::share_mode(0)` probe.
  - Minimum idle age cooldown (`min_idle_age`, including future-skew protection) and batched two-point `(size, mtime)` stability verification (`stability_window`).

- **Shell-Injection-Free Pre-Delete Lifecycle Hooks (`pre_delete_hook`)**
  - Direct process execution via `Command::new` (`argv`, never `sh -c` or `cmd /C`) with token interpolation (`{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`) and `FILECLEANER_*` environment variables.
  - Configurable wall-clock `timeout` with process termination/reaping, graceful per-file skip exit code (`skip_exit_code = 42` default), and failure policies (`abort_file`, `abort_rule`, `warn_and_continue`).

- **Atomic Lifecycle Pipeline & Pure-Rust Compression (`actions`)**
  - Sequential `WorkingTarget` composition across `rename_by_date`, `compress`, `move`, `copy_truncate` (explicit opt-in with validation advisory), `trash`, and `delete`.
  - 100% pure-Rust compression codecs (`gzip` via `flate2`/`miniz_oxide`, `lz4` via `lz4_flex`, `brotli`, `zip` pure-Rust `deflate`) preserving original `atime`/`mtime` via Rust stdlib `std::fs::FileTimes` (`File::set_times`).
  - Crash-safe `.fc-tmp` staging + `sync_all()` + atomic `rename`, plus cross-filesystem (`EXDEV`) dual-read SHA-256 integrity verification before source unlink.
  - Strict `plan` / `--dry-run` parity with zero filesystem mutations or `.fc-tmp` artifacts.

- **CLI, Daemon Mode & Structured Audit Logging**
  - Complete CLI (`clap` v4): `run` (alias `clean`), `plan` (supporting both `--config` and ad-hoc `--dir` mode), `check-in-use <PATH...>`, `validate [PATH]`, `init` (`--profile backups|logs|spool`), and `daemon {run, status, stop, service-template}` with deterministic exit codes (`0`, `1`, `2`, `3`, `4`, `10`) and `--json` support.
  - Daemon scheduling across `interval`, `notify` (debounced filesystem watcher), and `hybrid` modes, atomic `pid_file` and live JSON `state_file`, cooperative `stop_file` + `SIGINT`/`SIGTERM` mid-file-safe shutdown, bounded `--max-sweeps`, and OS service templates for `systemd`, `launchd`, and Windows Task Scheduler.
  - Synchronous UTF-8 JSONL structured audit logging (`[audit].log_path` / `--audit-log`).

- **9-Target Cross-Platform Portability & Bilingual Documentation**
  - Zero C/C++ `-sys` dependencies, supporting Linux (`x86_64`/`aarch64` `gnu` & `musl`, plus `armv7-unknown-linux-gnueabihf`), Windows (`x86_64`/`aarch64` MSVC), and macOS (`aarch64`/`x86_64` Darwin), verified with **`95` passing tests** (`57` unit + `38` integration across `8` suites), **`0` Clippy warnings**, and **`92.85%` line coverage** across `8,082` lines (`cargo llvm-cov --fail-under-lines 90`).
  - Synchronized bilingual English & French documentation: `README.md` / `README.fr.md`, `docs/en/SPECIFICATIONS.md` / `docs/fr/SPECIFICATIONS.md`, `docs/en/USER_GUIDE.md` / `docs/fr/GUIDE_UTILISATEUR.md`, `docs/en/COOKBOOK_EXAMPLES.md` / `docs/fr/EXEMPLES_RECETTES.md`, custom SVG/PNG branding (`docs/assets/*`), and engineering governance registers (`docs/ACTION_LOGBOOK.md`, `docs/RCA_REGISTER.md`).
