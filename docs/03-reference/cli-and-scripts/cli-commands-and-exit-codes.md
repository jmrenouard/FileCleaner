# CLI Commands, Scripts, Output Schemas & Exit Codes

Normative command-line, JSON output, repository script, and exit-code specifications for FileCleaner `v1.1.0`
(`src/cli.rs`, `src/engine.rs`, `src/audit.rs`, `src/daemon.rs`).

## 1. Global CLI Options (`filecleaner`)

Global options apply across subcommands (`clap` v4 `global = true`):

| Flag | Short | Argument | Default | Behavior |
| :--- | :--- | :--- | :--- | :--- |
| `--config` | `-c` | `<PATH>` | `None` | Path to `.toml`, `.yaml`, or `.yml` configuration file. |
| `--dry-run` | — | *(switch)* | `false` | Forces read-only simulation; blocks all filesystem and S3 mutations. |
| `--json` | — | *(switch)* | `false` | Emits structured JSON payloads on `stdout`. |
| `--verbose` | `-v` | *(switch)* | `false` | Emits per-candidate decision lines (`[rule] Decision: path (reason)`) before the summary table. |
| `--quiet` | `-q` | *(switch)* | `false` | Suppresses non-error human-readable `stdout` summaries (ignored when `--json` is active). |
| `--audit-log` | — | `<PATH>` | `None` | Overrides `[audit].log_path` and top-level `audit_log` for JSONL event output. |
| `--version` | `-V` | *(switch)* | — | Prints package name and version (`1.1.0`). |
| `--help` | `-h` | *(switch)* | — | Prints command or subcommand help text. |

---

## 2. Subcommand Specifications

### `filecleaner run` (Alias: `clean`) and `filecleaner plan`

- `filecleaner run` executes all 6 phases (`Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`).
- `filecleaner plan` forces `dry_run = true` and executes Phases 1–4 plus `SimulatedDryRun` hook reporting
  (hooks spawn only if `hook.run_in_dry_run = true`).

#### Config-Driven and Gate Flags

| Flag | Value | Default | Behavior |
| :--- | :--- | :--- | :--- |
| `--rule` | `<NAME>` | `[]` | Restricts execution to named rule(s) (repeatable). Unknown rule names fail with exit `2`. |
| `--fail-on-freeze` | *(switch)* | `false` | Elevates exit code from `0` to `3` if any evaluated rule enters `RuleOutcome::Frozen`. |
| `--strict-constraints` | *(switch)* | `false` | Elevates exit code from `0` to `3` if any evaluated rule ends in `RuleOutcome::Constrained`. |

#### Ad-Hoc Mode Flags (`--dir` Mutually Exclusive with `--config`)

Passing both `--config` and `--dir`, or omitting both, fails with exit `2`. Passing both `--move-to` and
`--trash-dir` in ad-hoc mode fails with exit `2`.

| Flag | Value | Default | Mapped `RuleConfig` Field |
| :--- | :--- | :--- | :--- |
| `--dir` | `<DIR>` | `None` | `rule.path` (creates synthetic rule `"adhoc"`). |
| `--include` | `<GLOB>` | `[]` | `rule.include_globs` (repeatable). |
| `--exclude` | `<GLOB>` | `[]` | `rule.exclude_globs` (repeatable). |
| `--recursive` | *(switch)* | `false` | `rule.recursive`. |
| `--max-files` | `<N>` | `None` | `rule.max_files`. |
| `--max-age` | `<DURATION>` | `None` | `rule.max_age` (`parse_duration_str`). |
| `--max-size` | `<BYTES>` | `None` | `rule.max_total_size` (`parse_bytes`). |
| `--min-keep` | `<N>` | `None` | `backup_guard.min_keep_files` (enables `backup_guard`). |
| `--min-valid-backup-bytes` | `<BYTES>` | `None` | `backup_guard.min_valid_backup_bytes` (enables `backup_guard`). |
| `--require-newer-within` | `<DURATION>` | `None` | `backup_guard.require_newer_within` (enables `backup_guard`). |
| `--freeze-before-date` | `<TIMESTAMP>` | `None` | `backup_guard.freeze_if_newest_older_than` (`RFC 3339`, `YYYY-MM-DD HH:MM:SS`, or `YYYY-MM-DD` UTC). |
| `--min-idle-age` | `<DURATION>` | `None` | `in_use.min_idle_age`. |
| `--rename-by-date` | *(switch)* | `false` | Prepends `ActionStep::RenameByDate`. |
| `--rename-template` | `<STRFTIME>` | `None` | Sets `rule.rename_template` and activates `ActionStep::RenameByDate`. |
| `--compress` | `<CODEC>` | `None` | Appends `ActionStep::Compress` (`gzip`, `lz4`, `brotli`, `zip`). |
| `--move-to` | `<DIR>` | `None` | Appends `ActionStep::Move` with `move_to`. |
| `--trash-dir` | `<DIR>` | `None` | Appends `ActionStep::Trash` with `trash_dir`. Defaults to `["delete"]` when no `Move`/`Trash`/`Compress`/`RenameByDate` step is present. |
| `--pre-hook` | `<COMMAND>` | `None` | Sets `pre_delete_hook` (`timeout = 30s`, `on_failure = abort_file`, `skip_exit_code = 42`, `run_in_dry_run = false`). |

### `filecleaner check-in-use <PATH...>`

Runs the 6-layer active-writer detection matrix in read-only mode (`EXIT_OK = 0` when `busy_count == 0`,
`EXIT_IN_USE_BUSY = 10` when any path is busy or unreadable).

| Argument / Flag | Value | Default | Behavior |
| :--- | :--- | :--- | :--- |
| `<PATH>...` | `<PATH>` | **Required** | One or more file paths to probe (non-regular or missing paths fail closed as `ProbeError`). |
| `--idle-age` | `<DURATION>` | `None` | Sets `in_use.min_idle_age`. |
| `--stability` | `<DURATION>` | `None` | Sets `in_use.stability_window`. |
| `--sidecar-ext` | `<EXT>` | `[".lock", ".lck", ".part", ".tmp", ".processing"]` | Overrides `in_use.sidecar_suffixes` (repeatable). |
| `--no-flock` | *(switch)* | `false` | Disables Unix `flock` check (`check_os_locks = false` on Unix). |
| `--no-proc-fd` | *(switch)* | `false` | Disables Linux `/proc/*/fd` scan (`check_proc_fd = false`). |
| `--no-windows-share` | *(switch)* | `false` | Disables Windows `share_mode(0)` probe (`check_os_locks = false` on Windows). |

JSON output structure (`--json`): `{ "all_safe": bool, "busy_count": usize, "reports": [InUseReport] }`.

### `filecleaner validate [PATH]`

Parses and validates a `.toml`, `.yaml`, or `.yml` file against `INV-CFG-01`..`INV-CFG-14` without accessing
monitored directories. Requires positional `<PATH>` or global `-c/--config <PATH>`.

JSON output structure (`--json`):

- Valid: `{ "valid": true, "path": "<PATH>", "rules": <usize>, "rule_names": [...], "warnings": [...] }`
  (`warnings` flags rules enabling `copy_truncate`).
- Invalid: `{ "valid": false, "path": "<PATH>", "error": "<String>" }` with exit code `2` (or `1` on I/O failure).

### `filecleaner init`

Atomically writes a validated starter configuration with `0o600` Unix mode (`dry_run = true`).

| Flag | Value | Default | Allowed Values / Notes |
| :--- | :--- | :--- | :--- |
| `-o, --output` | `<PATH>` | `filecleaner.toml` (or `.yaml`) | Target file path; refuses overwrite without `--force`. |
| `--profile` | `<PROFILE>` | `backups` | `backups`, `logs`, `spool`, `dba-postgres`. |
| `--format` | `<FORMAT>` | Auto / `toml` | `toml`, `yaml` (auto-detected from `.yaml`/`.yml` output extension when omitted). |
| `--force` | *(switch)* | `false` | Permits overwriting an existing output file. |

### `filecleaner daemon` Subcommands

| Subcommand | Required Flags | Optional Flags & Defaults | Exit Contract |
| :--- | :--- | :--- | :--- |
| `daemon run` | `-c, --config <PATH>` | `--mode <interval\|notify\|hybrid>`, `--max-sweeps <N>`, `--fail-on-freeze`, `--strict-constraints`, `--dry-run`, `--audit-log <PATH>` | `0` clean exit; `2` missing/invalid config; `4` live PID lock held. |
| `daemon status` | `-c <PATH>` or `--pid-file` / `--state-file` | `--pid-file <PATH>`, `--state-file <PATH>`, `--json` | `0` when running (`is_live_filecleaner_pid`); `4` when stopped, stale, or missing state/PID. |
| `daemon stop` | `-c <PATH>` or `--pid-file` / `--stop-file` | `--pid-file <PATH>`, `--stop-file <PATH>`, `--timeout <DURATION>` (`"10s"`) | `0` on clean stop or already stopped; `4` on foreign PID refusal or timeout. |
| `daemon service-template` | `--init-system <TARGET>` (alias `--target`) | `-c <PATH>` (default `/etc/filecleaner/filecleaner.toml`), `--user <USER>`, `-o, --output <PATH>` (`0o644`) | `0` on template emission (`systemd`, `launchd`, `windows-task`). |

---

## 3. Process Exit Codes (`src/engine.rs`, `src/cli.rs`)

| Exit Code | Rust Constant | Trigger Conditions |
| :---: | :--- | :--- |
| `0` | `EXIT_OK` | Clean completion of `run`, `plan`, `validate`, `init`, `daemon run`, `daemon status` (running), `daemon stop`; `check-in-use` with `busy_count == 0`; benign hook vetoes (`skip_exit_code = 42`); non-strict `Frozen` or `Constrained` rules. |
| `1` | `EXIT_OPERATIONAL_ERROR` | Filesystem `Io` error, `S3Upload` error, `IntegrityMismatch`, `Compression` failure, `Serialization` failure, `BackupGuardViolation`, `InUseProbe` error, `TimestampUnsupported` (`TimeBasis::Created` unsupported), or hook `HookFailed`/`HookTimedOut` under `abort_file`/`abort_rule`. Takes precedence over exit `3`. |
| `2` | `EXIT_CONFIG_OR_USAGE_ERROR` | `clap` argument syntax error, unknown `--rule`, `ConfigParse`, `ConfigFormat`, `ConfigValidation` (`INV-CFG-*`), `InvalidGlob`, `InvalidRegex`, `InvalidDuration`, `InvalidByteSize`. |
| `3` | `EXIT_STRICT_SAFETY_GATE` | `RuleOutcome::Frozen` with `--fail-on-freeze`, or `RuleOutcome::Constrained` with `--strict-constraints` (when `has_operational_error == false`). |
| `4` | `EXIT_DAEMON_ERROR` | `FileCleanerError::Daemon` (live PID already running, foreign PID verification failure, `daemon stop` timeout) or `daemon status` reporting stopped/stale daemon. |
| `10` | `EXIT_IN_USE_BUSY` | `filecleaner check-in-use` detects `busy_count > 0` or candidate probe error (`ProbeError`). |

---

## 4. Structured Telemetry & Audit Log Schemas

### JSONL Audit Record (`AuditEntry`, `src/audit.rs`)

Each record is serialized as a single UTF-8 JSON line terminated by `\n` and flushed synchronously. `AuditLogger`
creates files with `0o600` Unix permissions and rotates at `50 MiB` (`DEFAULT_AUDIT_ROTATE_BYTES`) to
`<log_path>.1`.

| Key | JSON Type | Values / Format |
| :--- | :--- | :--- |
| `ts` | `string` | UTC RFC 3339 event timestamp. |
| `event` | `string` | `sweep_start`, `sweep_end`, `file_decision`, `backup_freeze`, `rule_constrained`, `hook_exec`, `action_exec`, `error`. |
| `run_id` | `string` | Unique sweep identifier (`run-<pid>-<seq>`). |
| `rule` | `string \| null` | Rule name (`null` for `sweep_start` / `sweep_end`). |
| `path` | `string \| null` | Candidate file path. |
| `size_bytes` | `number \| null` | Candidate byte length. |
| `resolved_ts` | `string \| null` | Resolved `TimeBasis` timestamp in RFC 3339 UTC. |
| `decision` | `string \| null` | `keep_floor`, `keep_retention`, `skip_in_use`, `skip_hook`, `skip_ts_parse`, `freeze`, `would_act`, `acted`, `aborted`, `error`. |
| `in_use_reasons` | `array \| null` | Triggered `InUseReason` descriptions when `decision == "skip_in_use"`. |
| `actions` | `array<string>` | Executed step labels (`RenameByDate`, `Compress(Gzip)`, `Move`, `CopyTruncate`, `UploadS3`, `Trash`, `Delete`). |
| `action` | `string \| undefined` | Arrow-joined summary (omitted when `actions` is empty). |
| `dest_path` | `string \| null` | Final local destination path (`null` after `Delete` or `sweep_*`). |
| `sha256` | `string \| null` | Verified lowercase hex SHA-256 digest from cross-FS `Move`/`Trash`/`CopyTruncate` or `UploadS3`. |
| `dry_run` | `boolean` | Effective dry-run state. |
| `duration_ms` | `number` | Elapsed operation duration in milliseconds. |
| `error` | `string \| null` | Diagnostic error, freeze, or constraint message. |
| `details` | `object \| undefined` | Extended event payload (`s3_uri`, `etag`, `verify_mode`, `parts_uploaded`, `verified_size` for `UploadS3`; omitted when unset). |

### Live Daemon State JSON (`DaemonState`, `src/daemon.rs`)

Written atomically via `.fc-tmp` with `0o600` permissions to `daemon.state_file`:

| Key | JSON Type | Description |
| :--- | :--- | :--- |
| `version` | `string` | Schema version (`"1.0.0"`). |
| `pid` | `number` | Daemon OS process ID. |
| `status` | `string` | `starting`, `idle`, `sweeping`, `stopping`, `stopped`. |
| `watch_mode` | `string` | Effective `DaemonMode`: `interval`, `notify`, or `hybrid`. |
| `started_at` | `string` | UTC RFC 3339 daemon start timestamp. |
| `last_sweep_at` | `string \| null` | UTC RFC 3339 completion timestamp of most recent sweep. |
| `next_sweep_at` | `string \| null` | Scheduled UTC RFC 3339 next interval tick (`null` in `notify` or while `sweeping`/`stopping`/`stopped`). |
| `sweeps_completed` | `number` | Completed sweep counter. |
| `total_files_evaluated` | `number` | Cumulative candidate files evaluated. |
| `total_files_processed` | `number` | Cumulative files acted upon (or simulated in dry-run). |
| `total_bytes_reclaimed` | `number` | Cumulative local bytes freed in live sweeps. |
| `active_freezes` | `array<string>` | Rule names frozen in the latest sweep. |
| `constrained_rules` | `array<string>` | Rule names ending as `Constrained` in the latest sweep. |
| `last_error` | `string \| null` | Most recent sweep error diagnostic, if any. |

---

## 5. Repository Automation Scripts

### Docker E2E Verification Harness (`scripts/e2e-dba-s3-lab.sh`)

Runs 6 live scenarios against `postgres:17-bookworm` (`wal_level=replica`, `archive_mode=on`) and pinned MinIO
(`pgsty/minio:RELEASE.2026-08-04T00-00-00Z`).

| Variable | Default | Purpose |
| :--- | :--- | :--- |
| `FC_E2E_BIN` | `${CARGO_TARGET_DIR:-/tmp/filecleaner-target}/debug/filecleaner` | Compiled `filecleaner` binary path. |
| `FC_E2E_PG_IMAGE` | `postgres:17-bookworm` | PostgreSQL container image. |
| `FC_E2E_MINIO_IMAGE` | `pgsty/minio:RELEASE.2026-08-04T00-00-00Z` | MinIO S3-compatible server image. |
| `FC_E2E_TMP_ROOT` | `/var/tmp` | Host directory bind-mounted into containers (shared across WSL2 Docker namespaces). |

### Versioned Documentation Builder (`scripts/build_versioned_docs.py`)

Builds multi-version MkDocs Material static output (`main`, `v1.0.1`, `v1.0.0`, and future `v*` semver tags) with
`versions.json` and Apache `.htaccess` routing for `https://filecleaner.lightpath.fr/`.

| Flag / Env Var | Default | Purpose |
| :--- | :--- | :--- |
| `--output` | `site_dist` | Target static site output directory (protected paths such as `.git`, `docs`, `src`, `tests` are rejected). |
| `--base-url` | `https://filecleaner.lightpath.fr` | Base canonical URL applied to versioned MkDocs builds. |
| `--main-ref` | `WORKTREE` | Git ref for `/main/` (`origin/main` in release CI). |
| `--tags` | *(all semver tags)* | Space- or comma-separated release tags to render. |
| `--skip-tags` | `false` | Build only `/main/` without historical tags. |
| `--no-strict` | `false` | Disables `mkdocs build --strict`. |
| `FILECLEANER_DOCS_REF` | `main` | Runtime hook git ref controlling `edit_uri` and external GitHub source links. |
| `FILECLEANER_DOCS_SITE_URL` | *(from `mkdocs.yml`)* | Per-version `site_url` override injected by `build_versioned_docs.py`. |

---

## Related Reference and Procedures

- Configuration fields and invariants:
  [Configuration Schema & Invariants](../configuration/config-schema-and-invariants.md)
- Error taxonomy and alert-to-runbook matrix:
  [Error & Alert Matrix](../troubleshooting-codes/error-and-alert-matrix.md)
- Deployment and daemon operations:
  [Deploy with Docker and systemd](../../02-how-to-guides/deployment/deploy-filecleaner-docker-and-systemd.md)
  · [Run Reactive Daemon and Rotate Logs](../../02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md)
- Documentation platform decision:
  [ADR-002: Diátaxis and Versioned MkDocs Publishing](../../04-explanation/design-decisions/adr-002-diataxis-versioned-mkdocs-publishing.md)
