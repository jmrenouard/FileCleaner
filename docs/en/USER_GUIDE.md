<div align="center">
  <img src="../assets/header-banner.svg" alt="FileCleaner — User Guide" width="100%" />
</div>

# FileCleaner v1.0.0 — Complete User & Operations Guide

> **Language Editions**: **English** · [Français (`docs/fr/GUIDE_UTILISATEUR.md`)](../fr/GUIDE_UTILISATEUR.md)  
> **Related Docs**: [README](../../README.md) · [5-Minute HOWTO](HOWTO.md) · [Cookbook Examples](COOKBOOK_EXAMPLES.md) · [Formal Specifications](SPECIFICATIONS.md)

---

## Table of Contents

1. [Installation (Prebuilt Binaries & Source)](#1-installation-prebuilt-binaries--source)
2. [CLI Reference & Exit Codes](#2-cli-reference--exit-codes)
3. [Complete TOML & YAML Configuration Reference](#3-complete-toml--yaml-configuration-reference)
4. [Backup Safety Guardrails & `Constrained` Outcomes](#4-backup-safety-guardrails--constrained-outcomes)
5. [6-Layer In-Use File Detection Matrix](#5-6-layer-in-use-file-detection-matrix)
6. [Pre-Delete Lifecycle Hooks (`FILECLEANER_*` & Exit `42`)](#6-pre-delete-lifecycle-hooks)
7. [Lifecycle Action Pipeline & Atomic Staging](#7-lifecycle-action-pipeline--atomic-staging)
8. [JSONL Audit Log & Live Daemon State Schema](#8-jsonl-audit-log--live-daemon-state-schema)
9. [Daemon Mode & OS Service Integration (`systemd`, `launchd`, Windows Task Scheduler)](#9-daemon-mode--os-service-integration)

---

## 1. Installation (Prebuilt Binaries & Source)

FileCleaner ships as a single self-contained native binary with **zero runtime dependencies** and **zero C/C++ `-sys` library linkages**.

### 1.1 Prebuilt Binaries (9 Release Targets)

Download the archive matching your OS and architecture from [GitHub Releases](https://github.com/jmrenouard/FileCleaner/releases), verify its SHA-256 digest against `SHA256SUMS.txt`, and place `filecleaner` (or `filecleaner.exe`) on your `PATH`:

| OS | Architecture | Target Triple | Archive |
| :--- | :--- | :--- | :--- |
| Linux (glibc) | `x86_64` | `x86_64-unknown-linux-gnu` | `filecleaner-v1.0.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (static `musl`) | `x86_64` | `x86_64-unknown-linux-musl` | `filecleaner-v1.0.0-x86_64-unknown-linux-musl.tar.gz` |
| Linux (glibc) | `ARM64` | `aarch64-unknown-linux-gnu` | `filecleaner-v1.0.0-aarch64-unknown-linux-gnu.tar.gz` |
| Linux (static `musl`) | `ARM64` | `aarch64-unknown-linux-musl` | `filecleaner-v1.0.0-aarch64-unknown-linux-musl.tar.gz` |
| Linux (hard-float) | `ARMv7` | `armv7-unknown-linux-gnueabihf` | `filecleaner-v1.0.0-armv7-unknown-linux-gnueabihf.tar.gz` |
| Windows (MSVC) | `x86_64` | `x86_64-pc-windows-msvc` | `filecleaner-v1.0.0-x86_64-pc-windows-msvc.zip` |
| Windows (ARM64 MSVC) | `ARM64` | `aarch64-pc-windows-msvc` | `filecleaner-v1.0.0-aarch64-pc-windows-msvc.zip` |
| macOS (Apple Silicon) | `ARM64` | `aarch64-apple-darwin` | `filecleaner-v1.0.0-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `x86_64` | `x86_64-apple-darwin` | `filecleaner-v1.0.0-x86_64-apple-darwin.tar.gz` |

#### Linux / macOS Install & Checksum Verification

```bash
VERSION="v1.0.0"
TARGET="x86_64-unknown-linux-musl"   # or aarch64-apple-darwin, etc.

curl -fLO "https://github.com/jmrenouard/FileCleaner/releases/download/${VERSION}/filecleaner-${VERSION}-${TARGET}.tar.gz"
curl -fLO "https://github.com/jmrenouard/FileCleaner/releases/download/${VERSION}/SHA256SUMS.txt"

sha256sum --ignore-missing -c SHA256SUMS.txt   # macOS: shasum -a 256 --ignore-missing -c SHA256SUMS.txt
tar -xzf "filecleaner-${VERSION}-${TARGET}.tar.gz"
sudo install -m 0755 filecleaner /usr/local/bin/filecleaner
filecleaner --version
```

#### Windows PowerShell Install & Checksum Verification

```powershell
$Version = "v1.0.0"
$Target  = "x86_64-pc-windows-msvc"
$Archive = "filecleaner-$Version-$Target.zip"

Invoke-WebRequest -Uri "https://github.com/jmrenouard/FileCleaner/releases/download/$Version/$Archive" -OutFile $Archive
Invoke-WebRequest -Uri "https://github.com/jmrenouard/FileCleaner/releases/download/$Version/SHA256SUMS.txt" -OutFile "SHA256SUMS.txt"

Get-FileHash $Archive -Algorithm SHA256
Expand-Archive -Path $Archive -DestinationPath "$Env:ProgramFiles\FileCleaner" -Force
# Add "$Env:ProgramFiles\FileCleaner" to System PATH
& "$Env:ProgramFiles\FileCleaner\filecleaner.exe" --version
```

### 1.2 Install via Cargo (`cargo install` or Source Build)

Requires Rust Edition 2021 (`rustc >= 1.75`). Because FileCleaner uses zero C `-sys` crates, no `gcc`, `cmake`, or OpenSSL headers are required:

```bash
# Install directly from GitHub
cargo install --git https://github.com/jmrenouard/FileCleaner --tag v1.0.0 --locked

# Or clone and build a local release binary
git clone https://github.com/jmrenouard/FileCleaner.git
cd FileCleaner
cargo build --release --locked
./target/release/filecleaner --help
```

---

## 2. CLI Reference & Exit Codes

### 2.1 Global Options

Global flags may be supplied before or alongside subcommands:

| Flag | Short | Argument | Purpose |
| :--- | :--- | :--- | :--- |
| `--config` | `-c` | `<PATH>` | Path to `.toml`, `.yaml`, or `.yml` configuration file. |
| `--dry-run` | — | *(switch)* | Force dry-run simulation across all rules; prevents every filesystem write, rename, compress, move, truncate, trash, or unlink. |
| `--json` | — | *(switch)* | Emit structured machine-readable JSON output to stdout. |
| `--verbose` | `-v` | *(switch)* | Increase diagnostic logging verbosity (per-candidate skip reasons, probe details). |
| `--quiet` | `-q` | *(switch)* | Suppress non-essential stdout summaries (errors still emit to stderr). |
| `--audit-log` | — | `<PATH>` | Override `[audit].log_path` / `audit_log` from CLI for JSONL event output. |

### 2.2 Subcommand Reference

#### `filecleaner init`

Generates an annotated starter configuration with safe defaults (`dry_run = true`, `backup_guard.min_keep_files = 3`).

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-o, --output <PATH>` | `./filecleaner.toml` (or `.yaml`) | Output file path written atomically via `.fc-tmp`. |
| `--profile <PROFILE>` | `backups` | Starter rule profile template: `backups`, `logs`, or `spool`. |
| `--format <FMT>` | Auto / `toml` | Output format: `toml` or `yaml` (auto-inferred from `--output` extension when omitted). |
| `--force` | `false` | Overwrite an existing file at `--output`. |

```bash
filecleaner init --output /etc/filecleaner/filecleaner.toml --profile backups --format toml
filecleaner init --output /etc/filecleaner/filecleaner.yaml --profile logs --format yaml --force
```

#### `filecleaner validate [PATH]`

Parses `.toml`/`.yaml`/`.yml`, checks human byte/duration units, compiles all globs, regexes, and `strftime` patterns, and enforces all 12 `INV-CFG-*` invariants. Never touches rule target directories.

| Flag | Description |
| :--- | :--- |
| `<PATH>` / `-c, --config <PATH>` | Configuration file to validate (**required** via positional `<PATH>` or `--config`). |
| `--json` | Emit structured validation verdict (valid rule count, warnings such as `CopyTruncate` advisories, or validation error details). |

```bash
filecleaner validate /etc/filecleaner/filecleaner.toml
filecleaner validate -c /etc/filecleaner/filecleaner.yaml --json
```

#### `filecleaner plan`

Executes Phases 1–4 (`Scan -> InUseProbe -> BackupGuard -> RetentionEval`) in strictly read-only mode (`dry_run` forced `true`). Supports either `--config <PATH>` or ad-hoc `--dir <DIR>` mode. Produces the exact `SweepPlan` that a live `run` would execute under the same filesystem snapshot. Pre-delete hooks are reported as simulated unless `hook.run_in_dry_run = true`.

| Flag | Description |
| :--- | :--- |
| `-c, --config <PATH>` | Configuration file (mutually exclusive with `--dir`). |
| `--rule <NAME>` | Filter execution to one or more named rules (repeatable). |
| `--fail-on-freeze` | Return exit code `3` if any evaluated rule enters `BackupGuard` freeze. |
| `--strict-constraints` | Return exit code `3` if any evaluated rule finishes as `Constrained`. |
| `--json` | Output full `SweepPlan` and summary report in JSON. |

```bash
filecleaner plan -c /etc/filecleaner/filecleaner.toml --rule pg-backups
filecleaner plan -c /etc/filecleaner/filecleaner.toml --json --fail-on-freeze --strict-constraints
filecleaner plan --dir /var/backups/postgres --include "*.sql" --max-age 14d --min-keep 3 --compress gzip
```

#### `filecleaner run` (Alias: `clean`)

Executes a single synchronous sweep across enabled rules (all 6 phases: `Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`) using either `--config <PATH>` or ad-hoc CLI flags (`--dir`).

| Flag | Description |
| :--- | :--- |
| `-c, --config <PATH>` | Configuration file (mutually exclusive with `--dir`). |
| `--rule <NAME>` | Restrict sweep to specified rule name(s) (repeatable). |
| `--dry-run` | Simulate identically to `plan` without modifying disk state. |
| `--fail-on-freeze` | Elevate exit code from `0` to `3` when a rule freezes due to missing/stale backups. |
| `--strict-constraints` | Elevate exit code from `0` to `3` when a rule hits `Constrained` (`max_files` or `max_total_size` still exceeded after respecting `min_keep_files` and `S_busy`). |
| `--json` | Output structured sweep summary JSON. |

**Ad-Hoc Mode Flags (for `run` / `clean` / `plan` without `--config`)**:

| Flag | Description |
| :--- | :--- |
| `--dir <DIR>` | Ad-hoc target directory to sweep (mutually exclusive with `--config`). |
| `--include <GLOB>` / `--exclude <GLOB>` | Repeatable include/exclude glob filters. |
| `--recursive` | Traverse subdirectories recursively. |
| `--max-files <N>` / `--max-age <DUR>` / `--max-size <BYTES>` | Ad-hoc retention caps. |
| `--min-keep <N>` / `--min-valid-backup-bytes <BYTES>` | Protect newest `N` backups meeting size threshold (`backup_guard`). |
| `--require-newer-within <DUR>` / `--freeze-before-date <TS>` | Dead-man's-switch freshness window or UTC cutoff (`RFC 3339` or `YYYY-MM-DD`). |
| `--min-idle-age <DUR>` | Minimum `mtime` idle cooldown before touching candidates. |
| `--rename-by-date` / `--rename-template <STRFTIME>` | Prepend `RenameByDate` step with optional `strftime` template. |
| `--compress <gzip\|lz4\|brotli\|zip>` | Compress evicted candidates using pure-Rust codec. |
| `--move-to <DIR>` / `--trash-dir <DIR>` | Archive destination or trash directory (mutually exclusive in ad-hoc mode). |
| `--pre-hook <COMMAND>` | Synchronous pre-delete hook command (`timeout = 30s`, `skip_exit_code = 42`). |

```bash
filecleaner run -c /etc/filecleaner/filecleaner.toml
filecleaner clean -c /etc/filecleaner/filecleaner.toml --dry-run --rule app-logs
filecleaner run --dir /var/tmp/builds --recursive --max-age 48h --trash-dir /var/tmp/quarantine
```

#### `filecleaner check-in-use <PATH...>`

Runs the 6-layer active-writer detection matrix on one or more target paths without modifying them. Exits `0` only when **all** paths are safe; exits `10` if any path is busy.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `<PATH...>` | **Required** | One or more candidate file paths to inspect. |
| `--idle-age <DUR>` | None | Require `now - mtime >= <DUR>` (e.g., `15s`, `2m`). |
| `--stability <DUR>` | None | Two-point `(size, mtime)` observation window (e.g., `2s`, `500ms`). |
| `--sidecar-ext <EXT>` | `.lock,.lck,.part,.tmp,.processing` | Override or supply sidecar extensions (repeatable). |
| `--no-flock` | `false` | Disable Unix `flock(LOCK_EX \| LOCK_NB)` check. |
| `--no-proc-fd` | `false` | Disable Linux `/proc/*/fd` inode scan. |
| `--no-windows-share` | `false` | Disable Windows `share_mode(0)` probe. |
| `--json` | `false` | Emit structured JSON `{ all_safe, busy_count, reports }`. |

```bash
# Exit 0 if safe, exit 10 if busy
if filecleaner check-in-use /var/spool/etl/batch_094.parquet --idle-age 30s --stability 3s; then
  echo "File is settled and safe for downstream ingestion"
else
  rc=$?
  echo "File is busy or errored (exit=$rc)"
fi
```

#### `filecleaner daemon` Subcommands

| Command | Flags | Behavior |
| :--- | :--- | :--- |
| `daemon run` | `-c <PATH>`, `--mode interval\|notify\|hybrid`, `--max-sweeps <N>`, `--fail-on-freeze`, `--strict-constraints`, `--dry-run` | Runs foreground daemon loop; honors `pid_file`, `state_file`, `stop_file`, `SIGINT`/`SIGTERM`, and optional bounded `--max-sweeps`. |
| `daemon status` | `-c <PATH>`, `--pid-file <PATH>`, `--state-file <PATH>`, `--json` | Checks `pid_file` process liveness and reads `state_file` metrics. Exits `0` if alive, `4` if stopped or stale. |
| `daemon stop` | `-c <PATH>`, `--pid-file <PATH>`, `--stop-file <PATH>`, `--timeout <DUR>` (default `10s`) | Creates `stop_file` (and sends `SIGTERM` on Unix) and waits up to `--timeout` for PID shutdown (`0` on clean exit, `4` on timeout). |
| `daemon service-template` | `--init-system systemd\|launchd\|windows-task` (alias `--target`), `-c <PATH>`, `--user <USER>`, `-o/--output <PATH>` | Generates a ready-to-install service unit/plist/Task Scheduler definition to stdout or `--output`. |

### 2.3 Deterministic Exit Codes

| Exit Code | Meaning & Automation Contract |
| :---: | :--- |
| **`0`** | Clean success (`run`, `plan`, `validate`, `init`, `daemon run` clean exit, `daemon status` running, `daemon stop` completed), **or** `check-in-use` confirmed file is **SAFE (idle)**. Graceful hook skips (`skip_exit_code = 42`) and non-strict `Frozen`/`Constrained` rules also exit `0`. |
| **`1`** | Runtime I/O failure, `IntegrityMismatch` during cross-FS SHA-256 verification, hook `AbortFile`/`AbortRule` failure, compression error, or `TimestampUnsupported` error on live sweep. |
| **`2`** | CLI argument syntax error, missing/invalid config format (`.toml`/`.yaml`/`.yml` required), or `INV-CFG-*` semantic validation error. |
| **`3`** | Strict safety gate triggered: rule hit `BackupGuard` freeze with `--fail-on-freeze`, **or** rule finished `Constrained` with `--strict-constraints`. Ideal for pager/alerting integrations. |
| **`4`** | Daemon lifecycle error (`daemon run` blocked by live `pid_file`, `daemon status` finds daemon stopped/stale, `daemon stop` exceeded `--timeout`). |
| **`10`** | `check-in-use <PATH>` detected that target file is **IN-USE / BUSY** (or failed fail-closed probe). Distinct from `1`/`2` so shell `if` checks distinguish busy from CLI typos. |

---

## 3. Complete TOML & YAML Configuration Reference

`Config::from_file` detects format strictly by lowercase extension: `.toml` uses `toml`, `.yaml`/`.yml` uses `serde_yaml_ng`. Any other extension fails immediately with `ConfigFormat`.

All Rust `enum` fields serialize as `snake_case` by default while accepting `PascalCase` `serde` aliases (for example, `"oldest_first"` or `"OldestFirst"`).

### 3.1 Top-Level `Config` Keys

| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `dry_run` | `bool` | `false` | Global simulation switch preventing any filesystem mutation across all rules. |
| `audit_log` | `Option<PathBuf>` | `None` | Top-level shorthand path for the JSONL audit file (`[audit].log_path` takes precedence if both are set). |
| `global_trash_dir` | `Option<PathBuf>` | `None` | Fallback quarantine directory used by any rule with `actions = [..., "trash"]` and no rule-local `trash_dir`. |
| `daemon` | `DaemonConfig` | Default `DaemonConfig` | Scheduling mode, polling interval, FS watcher debounce, and PID/state/stop file paths. |
| `audit` | `AuditConfig` | Default `AuditConfig` | Structured audit output (`log_path`, `jsonl` boolean defaulting to `true`). |
| `rules` | `Vec<RuleConfig>` | **Required (non-empty)** | Ordered list of independent directory rules. |

### 3.2 `[daemon]` (`DaemonConfig`)

| Key | Type | Default | Aliases | Description |
| :--- | :--- | :--- | :--- | :--- |
| `mode` | `DaemonMode` | `"hybrid"` | `Interval`, `Notify`, `Hybrid` | `"interval"` (fixed period), `"notify"` (OS FS watcher), or `"hybrid"` (watcher + fallback interval). |
| `interval` | `Duration` | `"60s"` | `poll_interval` | Sweep period (`> 0`). In `hybrid`, guarantees age/freshness checks run even when no FS events occur. |
| `watch` | `bool` | `false` | — | Explicitly enables `notify` watcher; `mode = "notify"` or `"hybrid"` automatically activates watching. |
| `debounce` | `Duration` | `"2s"` | — | Coalescing window batching bursts of filesystem watcher events before triggering a sweep. |
| `sweep_on_start` | `bool` | `true` | — | Execute an immediate sweep upon daemon startup before entering the wait loop. |
| `pid_file` | `Option<PathBuf>` | `None` | — | Atomic PID lock file path checked on startup and removed on clean shutdown. |
| `state_file` | `Option<PathBuf>` | `None` | — | Atomic live JSON daemon status output path. |
| `stop_file` | `Option<PathBuf>` | `None` | — | Cooperative shutdown trigger file watched every $\le 250\text{ ms}$. |

### 3.3 `[audit]` (`AuditConfig`)

| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `log_path` | `Option<PathBuf>` | `None` | Path where structured JSONL records are appended and flushed synchronously. |
| `jsonl` | `bool` | `true` | Emit JSON Lines records when `log_path` / `audit_log` (or `--audit-log`) is configured. |

### 3.4 `[[rules]]` (`RuleConfig`)

| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `name` | `String` | **Required** | Unique non-empty rule name across all `rules`. |
| `path` | `PathBuf` | **Required** | Monitored root directory. |
| `enabled` | `bool` | `true` | Set `false` to skip this rule without deleting it from config. |
| `recursive` | `bool` | `false` | Traverse subdirectories when `true`. |
| `max_depth` | `Option<usize>` | `None` | Maximum recursion depth (ignored unless `recursive = true`). |
| `follow_symlinks` | `bool` | `false` | Follow symbolic links during scan. **Keep `false`** unless explicitly needed. |
| `include_globs` | `Vec<String>` | `[]` | `globset` patterns matched against forward-slash relative paths (empty selects all). |
| `exclude_globs` | `Vec<String>` | `[]` | Exclusion globs; always override `include_globs` and `include_regex`. |
| `include_regex` | `Option<String>` | `None` | Rust `regex` inclusion pattern on normalized relative paths. |
| `exclude_regex` | `Option<String>` | `None` | Rust `regex` exclusion pattern; always overrides includes. |
| `min_file_size` | `Option<Bytes>` | `None` | Lower file-size filter (`size >= min_file_size`) to exclude irrelevant tiny files from candidate selection. |
| `max_file_size` | `Option<Bytes>` | `None` | Upper file-size filter (`size <= max_file_size`); requires `min_file_size <= max_file_size`. |
| `time_basis` | `TimeBasis` | `"modified"` | Timestamp source: `"modified"`, `"created"`, or `"filename_date"`. |
| `filename_date_regex` | `Option<String>` | `None` | Regex extracting timestamp substring when `time_basis = "filename_date"` (prefers named group `(?<date>...)`, else group `1`, else group `0`). |
| `filename_date_format` | `Option<String>` | `None` | `chrono` `strftime` format (e.g. `"%Y%m%d_%H%M%S"`, `"%Y-%m-%d"`). |
| `max_files` | `Option<usize>` | `None` | Maximum retained candidate count (`>= backup_guard.min_keep_files`). |
| `max_age` | `Option<Duration>` | `None` | Maximum candidate age (`now - t(f) > max_age` triggers eviction eligibility). |
| `max_total_size` | `Option<Bytes>` | `None` | Maximum aggregate candidate byte cap across the rule pool. |
| `combine_mode` | `CombineMode` | `"any"` | `"any"` (OR: evict if any active limit is violated) or `"all"` (AND: evict only when all active limits are simultaneously violated). |
| `eviction_order` | `EvictionOrder` | `"oldest_first"` | `"oldest_first"` ($t(f)$ ascending) or `"largest_first"` (`size(f)` descending, oldest tie-breaker). |
| `dry_run` | `bool` | `false` | Force simulation mode for this rule only. |
| `actions` | `Vec<ActionStep>` | `["delete"]` | Ordered steps: `"rename_by_date"`, `"compress"`, `"move"`, `"copy_truncate"`, `"trash"`, `"delete"`. Terminal steps (`trash`, `delete`) must be last. |
| `move_to` | `Option<PathBuf>` | `None` | Required destination directory when `actions` includes `"move"`. |
| `move_date_subdir` | `Option<String>` | `None` | Optional `strftime` date subfolder appended to `move_to` (e.g., `"%Y/%m"`). |
| `move_overwrite` | `bool` | `false` | Allow overwriting an existing file at `move_to` destination. |
| `trash_dir` | `Option<PathBuf>` | `None` | Rule-specific quarantine directory (overrides `global_trash_dir`). |
| `compression` | `CompressionFormat` | `"gzip"` | Pure-Rust codec for `"compress"`: `"gzip"`, `"lz4"`, `"brotli"`, `"zip"`. |
| `compression_level` | `Option<u32>` | `None` | Optional codec-specific compression level. |
| `compress_remove_source` | `bool` | `true` | Unlink original file after staged compression atomically succeeds. |
| `compress_overwrite` | `bool` | `false` | Allow overwriting existing compressed destination archive. |
| `copy_truncate_to` | `Option<PathBuf>` | `None` | Destination archive directory for `"copy_truncate"` (falls back to `move_to`). |
| `rename_template` | `Option<String>` | `None` | Optional `strftime`/filename template for `"rename_by_date"` (defaults to `%Y%m%d_%H%M%S`). |
| `backup_guard` | `BackupGuardConfig` | Default | Backup floor and staleness freeze controls (§4). |
| `in_use` | `InUseConfig` | Default | 6-layer active-writer detection controls (§5). |
| `pre_delete_hook` | `Option<HookConfig>` | `None` | Alias: `pre_hook`. Synchronous external veto command (§6). |

### 3.5 Human-Readable Byte & Duration Units

**Byte Sizes (`parse_bytes`)**: Accepts unsigned integer bytes (`1048576`) or `<number>\s*<unit>` (case-insensitive, fractional decimals supported, e.g. `"1.5 GiB"`):

- **SI Decimal ($10^3$ powers)**: `KB` (`1,000`), `MB` (`1,000,000`), `GB` (`1,000,000,000`), `TB` (`10^{12}`), `PB` (`10^{15}`).
- **IEC Binary ($2^{10}$ powers)**: `K`/`KiB` (`1,024`), `M`/`MiB` (`1,048,576`), `G`/`GiB` (`1,073,741,824`), `T`/`TiB` (`1,099,511,627,776`), `P`/`PiB` (`2^{50}`).

**Durations (`parse_duration_str`)**: Accepts integer seconds (`60`) or compound `<uint><unit>` segments (`"500ms"`, `"30s"`, `"1d 12h"`, `"2m30s"`, `"2w"`):

- `ms` ($1\text{ ms}$), `s` ($1{,}000\text{ ms}$), `m` ($60{,}000\text{ ms}$), `h` ($3{,}600{,}000\text{ ms}$), `d` ($24\text{ h}$), `w` ($7\text{ d}$).

### 3.6 Static Validation Guardrails (`INV-CFG-01` .. `INV-CFG-12`)

`filecleaner validate` enforces twelve structural safety checks before any disk traversal:

- **Non-Empty & Unique**: `rules` non-empty, unique trimmed `rule.name`, non-empty `rule.path`, non-empty `rule.actions`, `daemon.interval > 0`.
- **Bounds Consistency**: `min_file_size <= max_file_size`; `max_files >= backup_guard.min_keep_files`.
- **Valid Matchers & Formats**: Every glob (`include_globs`, `exclude_globs`), regex (`include_regex`, `exclude_regex`, `filename_date_regex`), and `strftime` (`filename_date_format`, `move_date_subdir`) must compile cleanly; `time_basis = "filename_date"` requires both regex and format.
- **Required Destinations & Terminal Ordering**: `move` requires `move_to`; `copy_truncate` requires `copy_truncate_to` or `move_to`; `trash` requires `trash_dir` or `global_trash_dir`. `delete` and `trash` cannot be followed by further steps.
- **Recursive Self-Containment Guard (`INV-CFG-09`)**: If `recursive = true`, `move_to`, `copy_truncate_to`, and effective `trash_dir` **cannot** equal or nest inside `rule.path` unless explicitly excluded via `exclude_globs` or `exclude_regex`—preventing infinite move-and-rescan loops.

---

## 4. Backup Safety Guardrails & `Constrained` Outcomes

`[rules.backup_guard]` prevents automated cleanup from destroying your last healthy backup when an upstream dump or replication job breaks silently.

### 4.1 `[rules.backup_guard]` Fields

| Field | Type | Default | Aliases | Behavior |
| :--- | :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `false` | — | Enables backup safety guardrails on the rule. |
| `min_keep_files` | `usize` | `0` | — | Unconditionally protects the $N_{\min}$ newest valid backups ($P_{\text{floor}}$). |
| `min_valid_backup_bytes` | `Bytes` | `0` | `min_valid_size` | Minimum byte size $B_{\min}$ for a file to enter the valid backup set $\mathcal{F}_{\text{valid}}$. Files below $B_{\min}$ (such as `0`-byte failed dumps) cannot count toward `min_keep_files` or freshness checks. |
| `require_newer_within` | `Option<Duration>` | `None` | `max_newest_age` | **Dead-man's-switch**: freezes the entire rule (`E = empty`) if `now - newest_valid_ts > require_newer_within`. |
| `freeze_if_newest_older_than` | `Option<DateTime<Utc>>` | `None` | `min_newest_timestamp` | Absolute RFC 3339 UTC freeze boundary: freezes the rule if the newest valid backup predates this cutoff. |
| `freeze_on_empty_valid` | `bool` | `true` | `freeze_on_empty` | Freezes the rule when zero files satisfy `size >= min_valid_backup_bytes`. |

### 4.2 How In-Place Transforms Interact with Backup Counting

- **In-Place Transforms (`rename_by_date`, `compress`)**: When a compressed or renamed artifact remains inside `rule.path`, matches `include_globs`/`exclude_globs`, and still satisfies `size >= min_valid_backup_bytes`, it **continues counting** toward `min_keep_files` and freshness on subsequent sweeps.
- **Relocating Transforms (`move`, `trash`)**: Once an artifact is moved out of `rule.path` to a cold NAS or trash vault, it exits $\mathcal{F}$ for that rule.

### 4.3 Constraint Conflict Resolution (`Constrained`)

If `min_keep_files` ($P_{\text{floor}}$) and/or busy files ($S_{\text{busy}}$) prevent evicting enough files to satisfy `max_total_size` or `max_files`:

1. **Safety ALWAYS wins**: FileCleaner never violates `min_keep_files` or touches an in-use file.
2. **Outcome = `Constrained`**: FileCleaner records excess diagnostics (`remaining_files`, `target_max_files`, `remaining_bytes`, `target_max_total_size`, `protected_floor_count`, `busy_skipped_count`) in CLI output, the JSONL audit log, and daemon `state_file` (`constrained_rules`).
3. **Exit Code Behavior**: Returns `0` by default (preventing false cron alarms on transient locks) or exit code `3` when `--strict-constraints` is passed.

---

## 5. 6-Layer In-Use File Detection Matrix

When `in_use.enabled = true` (default), every candidate passes through up to six fail-closed probes before eviction planning. A single active trigger or candidate-level probe error places the file into $S_{\text{busy}}$.

### 5.1 `[rules.in_use]` Fields

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `true` | Master toggle for active-writer probes. |
| `check_sidecar_locks` | `bool` | `true` | Detect `.lock`, `.lck`, `.part`, `.tmp`, `.processing` filenames or `<file><suffix>` sidecars. |
| `sidecar_suffixes` | `Vec<String>` | `[".lock", ".lck", ".part", ".tmp", ".processing"]` | Custom list of active sidecar/transfer suffixes. |
| `check_os_locks` | `bool` | `true` | Unix `flock(LOCK_EX \| LOCK_NB)` via `O_RDONLY` / Windows `OpenOptionsExt::share_mode(0)`. |
| `check_proc_fd` | `bool` | `true` | Linux `/proc/<pid>/fd` `(st_dev, st_ino)` open-descriptor scan. |
| `min_idle_age` | `Option<Duration>` | `None` | Cooldown requiring `now - mtime >= min_idle_age` (also shields future-skewed `mtime > now`). |
| `stability_window` | `Option<Duration>` | `None` | Two-point `(size, mtime)` verification interval. |

### 5.2 Platform Behavior & Performance Notes

- **Linux `/proc/*/fd` Privilege Boundaries**: Unprivileged `EACCES`/`EPERM` on other users' `/proc/<pid>/fd` directories is logged as a diagnostic note (so unrelated root daemons do not block all user files), while any matching accessible fd marks `ProcFdOpen { pid }` and any error opening/stat-ing the candidate itself fails closed (`ProbeError`).
- **macOS / BSD**: Uses sidecar locks, `flock`, `min_idle_age`, and `stability_window` (recommended for un-flocked incoming transfers on macOS).
- **Windows**: `share_mode(0)` enforces kernel-level exclusive share checking even against processes that do not acquire explicit advisory locks.
- **Batched Stability Optimization**: `stability_window` captures snapshot `p0 = (size, mtime)` across all candidates that survived Probes 1–5, sleeps **once** for `stability_window`, and captures `p1` across all of them ($O(\Delta_{\text{stab}})$ total instead of $O(N \cdot \Delta_{\text{stab}})$).

---

## 6. Pre-Delete Lifecycle Hooks

`[rules.pre_delete_hook]` (alias `pre_hook`) lets an external script or verifier approve, skip, or abort before FileCleaner mutates a candidate.

### 6.1 Security Contract: Zero Shell Injection

FileCleaner **always** executes `hook.command` directly via `std::process::Command::new(&hook.command).args(...)` — **never** through `sh -c` or `cmd /C`. Filenames containing spaces, quotes, `;`, `|`, or `$()` remain harmless literal bytes in `argv`.

### 6.2 `[rules.pre_delete_hook]` Fields & Environment

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `command` | `String` | **Required** | Executable path or binary name. |
| `args` | `Vec<String>` | `[]` | Argument list supporting tokens `{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`. |
| `timeout` | `Duration` | `"30s"` | Wall-clock deadline; timed-out hooks are terminated (`kill()` + `wait()`). |
| `on_failure` | `HookFailurePolicy` | `"abort_file"` | `"abort_file"` (skip file, error), `"abort_rule"` (halt rule, exit `1`), `"warn_and_continue"` (warn and run actions). |
| `skip_exit_code` | `Option<i32>` | `42` | Exit code treated as a graceful, non-error per-file veto skip (keeps sweep exit code `0`). |
| `run_in_dry_run` | `bool` | `false` | Execute hook even during `plan` or `--dry-run`. |
| `working_dir` | `Option<PathBuf>` | `None` | Optional child working directory. |
| `env` | `Map<String, String>` | `{}` | Static environment variables merged with `FILECLEANER_*`. |

**Injected `FILECLEANER_*` Environment Variables**:

- `FILECLEANER_PATH`: Full path to candidate file.
- `FILECLEANER_NAME`: Candidate file basename.
- `FILECLEANER_DIR`: Parent directory path.
- `FILECLEANER_SIZE`: File size in bytes.
- `FILECLEANER_MTIME_ISO`: Filesystem `mtime` in RFC 3339 UTC.
- `FILECLEANER_RULE`: Rule identifier (`rule.name`).
- `FILECLEANER_ACTION`: Action pipeline summary (e.g. `Compress(Gzip) -> Move`).
- `FILECLEANER_DRY_RUN`: `"true"` or `"false"`.

---

## 7. Lifecycle Action Pipeline & Atomic Staging

Steps inside `rule.actions` run strictly sequentially, propagating `WorkingTarget` (current path, size, original timestamps) from one step to the next (e.g. `["rename_by_date", "compress", "move"]`).

| Action Step | Behavior & Key Rule Parameters | Crash-Safety & Metadata Contract |
| :--- | :--- | :--- |
| `rename_by_date` | Formats candidate timestamp with `rename_template` (`%Y%m%d_%H%M%S` default). | Never overwrites an existing target silently; preserves `atime`/`mtime` via `std::fs::FileTimes` (`File::set_times`); updates `WorkingTarget`. |
| `compress` | Pure-Rust codecs: `gzip` (`.gz`), `lz4` (`.lz4`), `brotli` (`.br`), `zip` (`.zip`). Controlled by `compression`, `compression_level`, `compress_remove_source`, `compress_overwrite`. | Streams into `<dest>.fc-tmp...`, flushes encoder + `sync_all()`, restores `atime`/`mtime` via `std::fs::FileTimes`, atomically renames to `<dest>`, then unlinks source only if `compress_remove_source = true`. |
| `move` | Moves `WorkingTarget` into `move_to` (plus optional `strftime` `move_date_subdir`). Controlled by `move_overwrite`. | Fast same-FS `rename`; on cross-FS (`EXDEV`), streams to `<dest>.fc-tmp...`, calls `sync_all()`, **re-reads `<dest>.fc-tmp...` from disk to verify SHA-256**, restores timestamps (`std::fs::FileTimes`) and permissions, atomically renames, then unlinks source. |
| `copy_truncate` | **Explicit opt-in** for append-only logs (`copy_truncate_to` or `move_to`, optional `compression`). | Stages + verifies archive **before** `file.set_len(0)` + `sync_all()` (preserves original path & inode). Emits validation warning regarding tiny trailing-byte race window between copy EOF and truncate. |
| `trash` | Quarantines file into `trash_dir` or `global_trash_dir`. | Disambiguates filename collisions cleanly (`_1`, `_2`, ...); cross-FS safe via SHA-256 verified copy. |
| `delete` | Unlinks `WorkingTarget` permanently (`remove_file`). | Terminal step (`trash` and `delete` must appear last). |

---

## 8. JSONL Audit Log & Live Daemon State Schema

### 8.1 JSONL Audit Log (`[audit].log_path` / `--audit-log`)

Every sweep decision, freeze, constraint conflict, hook run, action, and error appends one UTF-8 JSON line flushed synchronously to disk:

```json
{"ts":"2026-11-20T09:05:00.010Z","event":"file_decision","run_id":"run-42180-1","rule":"pg-backups","path":"/var/backups/postgres/pg_20261101_020000.sql","size_bytes":10485760,"resolved_ts":"2026-11-01T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Gzip)","Move"],"dest_path":"/mnt/cold-archive/postgres/2026/11/pg_20261101_020000.sql.gz","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","dry_run":false,"duration_ms":42,"error":null}
```

Key fields:

- `event`: `sweep_start` | `sweep_end` | `file_decision` | `backup_freeze` | `rule_constrained` | `hook_exec` | `action_exec` | `error`
- `decision`: `keep_floor` | `keep_retention` | `skip_in_use` | `skip_hook` | `skip_ts_parse` | `freeze` | `would_act` | `acted` | `aborted` | `error`
- `sha256`: Populated whenever cross-FS verified `move` or `copy_truncate` verifies content integrity.

### 8.2 Live Daemon State JSON (`[daemon].state_file`)

Updated atomically (`.fc-tmp` + `rename`) across `"starting"`, `"idle"`, `"sweeping"`, `"stopping"`, and `"stopped"` transitions:

```json
{
  "version": "1.0.0",
  "pid": 42180,
  "status": "idle",
  "watch_mode": "hybrid",
  "started_at": "2026-11-20T09:00:00.000Z",
  "last_sweep_at": "2026-11-20T09:05:00.142Z",
  "next_sweep_at": "2026-11-20T09:10:00.000Z",
  "sweeps_completed": 2,
  "total_files_evaluated": 128,
  "total_files_processed": 14,
  "total_bytes_reclaimed": 536870912,
  "active_freezes": ["pg-backups"],
  "constrained_rules": [],
  "last_error": null
}
```

---

## 9. Daemon Mode & OS Service Integration

FileCleaner's built-in daemon eliminates external cron jitter and exposes instant FS reactivity (`notify`) alongside wall-clock age/freshness guarantees (`hybrid` mode).

### 9.1 `systemd` Service (Linux)

Generate directly via `filecleaner daemon service-template --init-system systemd --config /etc/filecleaner/filecleaner.toml`:

```ini
[Unit]
Description=FileCleaner Deterministic Directory Retention Daemon
Documentation=https://github.com/jmrenouard/FileCleaner
After=local-fs.target network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/filecleaner daemon run --config /etc/filecleaner/filecleaner.toml
ExecStop=/usr/local/bin/filecleaner daemon stop --config /etc/filecleaner/filecleaner.toml --timeout 15s
Restart=on-failure
RestartSec=5s
KillSignal=SIGTERM
TimeoutStopSec=20s
NoNewPrivileges=true
PrivateTmp=false

[Install]
WantedBy=multi-user.target
```

Enable and monitor:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now filecleaner
filecleaner daemon status -c /etc/filecleaner/filecleaner.toml --json
```

### 9.2 `launchd` Agent / Daemon (macOS)

Generate via `filecleaner daemon service-template --init-system launchd --config /usr/local/etc/filecleaner.toml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.filecleaner.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/filecleaner</string>
        <string>daemon</string>
        <string>run</string>
        <string>--config</string>
        <string>/usr/local/etc/filecleaner.toml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/filecleaner/daemon.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/filecleaner/daemon.stderr.log</string>
</dict>
</plist>
```

### 9.3 Windows Task Scheduler

Generate an XML template via `filecleaner daemon service-template --init-system windows-task --config "C:\ProgramData\FileCleaner\filecleaner.toml" --output filecleaner-task.xml` and register it with PowerShell:

```powershell
Register-ScheduledTask -TaskName "FileCleanerDaemon" `
  -Xml (Get-Content "C:\ProgramData\FileCleaner\filecleaner-task.xml" -Raw)
Start-ScheduledTask -TaskName "FileCleanerDaemon"

# Cooperative cross-platform stop (creates daemon.stop_file and waits for clean mid-file finish)
filecleaner.exe daemon stop --config C:\ProgramData\FileCleaner\filecleaner.toml --timeout 15s
```

### 9.4 Bounded CI / Cron Runs (`--max-sweeps`)

To run `N` deterministic daemon sweeps (testing watcher + state JSON + shutdown cleanup) without running indefinitely:

```bash
filecleaner daemon run -c filecleaner.toml --mode interval --max-sweeps 1
```
