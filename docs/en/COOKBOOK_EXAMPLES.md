<div align="center">
  <img src="../assets/header-banner.svg" alt="FileCleaner — Production Cookbook Examples" width="100%" />
</div>

# FileCleaner v1.0.1 — Production Cookbook Examples

> **Language Editions**: **English** · [Français (`docs/fr/EXEMPLES_RECETTES.md`)](../fr/EXEMPLES_RECETTES.md)  
> **Related Docs**: [README](../../README.md) · [5-Minute HOWTO](HOWTO.md) · [User Guide](USER_GUIDE.md) · [Formal Specifications](SPECIFICATIONS.md)

Every configuration in this cookbook conforms strictly to `src/config.rs` and `Config::validate()` invariants (`INV-CFG-01`..`INV-CFG-14`). Test any recipe safely before touching files via `filecleaner validate` and `filecleaner plan`.

---

## Table of Contents

1. [Recipe 1: PostgreSQL & MySQL Nightly Backup Retention (Dead-Man's-Switch + Cross-FS SHA-256 NAS Move)](#1-recipe-1-postgresql--mysql-nightly-backup-retention)
2. [Recipe 2: Application & Nginx Log Rotation (`CopyTruncate` Opt-In vs. `RenameByDate` + `Lz4`/`Brotli`)](#2-recipe-2-application--nginx-log-rotation)
3. [Recipe 3: High-Throughput ETL Incoming Spool (Sidecars, `flock`, Stability Window & Exit-`42` Checksum Veto)](#3-recipe-3-high-throughput-etl-incoming-spool)
4. [Recipe 4: Multi-Rule Daemon Deployment (`Hybrid` Mode, PID/State/Stop Files & Strict Alerting)](#4-recipe-4-multi-rule-daemon-deployment)
5. [Recipe 5: PostgreSQL Base Backups & WAL Spool Archival to S3 / MinIO (`upload_s3` + PITR Guard + Docker E2E)](#5-recipe-5-postgresql-base-backups--wal-spool-archival-to-s3--minio)

---

## 1. Recipe 1: PostgreSQL & MySQL Nightly Backup Retention

### Operational Goal

Protect local database dump staging directories (`/var/backups/postgres` and `/var/backups/mysql`) while archiving older dumps onto a cold NAS mount (`/mnt/cold-nas/...`):

- Parse the backup timestamp directly from the **filename** (`time_basis = "filename_date"`), ignoring `scp`/`rsync` mtime drift.
- Reject corrupt or `0`-byte failed dumps (`min_valid_backup_bytes = "1MiB"`) so broken dumps never satisfy freshness checks.
- Always keep the **7 newest valid local backups** (`min_keep_files = 7`).
- **Dead-man's-switch freeze**: if no valid backup newer than `36h` exists (`require_newer_within = "36h"`), freeze all pruning/archiving immediately (`E = empty`).
- Compress older eligible local dumps with pure-Rust **gzip** and `move` them into date-partitioned cold NAS folders (`%Y/%m`) with dual-read SHA-256 verification across filesystems.

### Configuration (`/etc/filecleaner/db-backups.toml`)

```toml
dry_run = false

[audit]
log_path = "/var/log/filecleaner/db-backups-audit.jsonl"
jsonl = true

[[rules]]
name = "postgres-nightly"
path = "/var/backups/postgres"
recursive = false
follow_symlinks = false
include_globs = ["pg_*.sql", "pg_*.dump"]
exclude_globs = ["pinned/**"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_age = "14d"
max_total_size = "100GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
compress_overwrite = false
move_to = "/mnt/cold-nas/postgres"
move_date_subdir = "%Y/%m"
move_overwrite = false

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
sidecar_suffixes = [".lock", ".part", ".tmp", ".writing"]
check_os_locks = true
check_proc_fd = true
min_idle_age = "5m"
stability_window = "5s"

[[rules]]
name = "mysql-nightly"
path = "/var/backups/mysql"
recursive = false
follow_symlinks = false
include_globs = ["mysql_*.sql", "mysql_*.xbstream"]
time_basis = "filename_date"
filename_date_regex = 'mysql_(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2})'
filename_date_format = "%Y-%m-%d_%H-%M-%S"
max_files = 10
max_total_size = "80GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
move_to = "/mnt/cold-nas/mysql"
move_date_subdir = "%Y/%m"

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "5m"
stability_window = "5s"
```

### Verification & Execution Commands

```bash
# 1. Static validation
filecleaner validate -c /etc/filecleaner/db-backups.toml

# 2. Read-only deterministic plan
filecleaner plan -c /etc/filecleaner/db-backups.toml --rule postgres-nightly

# 3. Cron/systemd oneshot with strict freeze & constraint alerting (exit 3 triggers pager)
filecleaner run -c /etc/filecleaner/db-backups.toml --fail-on-freeze --strict-constraints
```

### Expected CLI & Audit Output

**Scenario A — Normal Sweep (`exit 0`)**:

```text
configuration valid: 2 rule(s) loaded from /etc/filecleaner/db-backups.toml
```

```json
{"ts":"2026-11-20T04:00:02.118Z","event":"file_decision","run_id":"run-18204-1","rule":"postgres-nightly","path":"/var/backups/postgres/pg_20261105_020000.sql","size_bytes":524288000,"resolved_ts":"2026-11-05T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Gzip)","Move"],"dest_path":"/mnt/cold-nas/postgres/2026/11/pg_20261105_020000.sql.gz","sha256":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08","dry_run":false,"duration_ms":3410,"error":null}
```

**Scenario B — Failed Nightly Cron (`0`-byte stub + stale valid backup > `36h`, `exit 3`)**:

Even if a `0`-byte file `pg_20261120_020000.sql` was created last night, `min_valid_backup_bytes = "1MiB"` excludes it from $\mathcal{F}_{\text{valid}}$. The newest valid backup is now `> 36h` old, so `BackupGuard` freezes all pruning and `--fail-on-freeze` returns exit code `3`:

```json
{"ts":"2026-11-20T15:00:00.012Z","event":"backup_freeze","run_id":"run-19102-1","rule":"postgres-nightly","path":null,"size_bytes":null,"resolved_ts":null,"decision":"freeze","in_use_reasons":null,"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":4,"error":"newest valid backup (2026-11-18T02:00:00Z) exceeds require_newer_within=36h; rule frozen"}
```

---

## 2. Recipe 2: Application & Nginx Log Rotation

### Operational Goal

Handle two real-world daemon logging patterns cleanly in a single YAML config (`/etc/filecleaner/logs-rotation.yaml`):

1. **Strategy A (`app-logs-copy-truncate`)**: Legacy Java/Python service writing continuously to an open `O_APPEND` file descriptor (`app.log`) without `SIGHUP`/`SIGUSR1` log-reopen support. Uses **explicit opt-in `copy_truncate`** with fast **LZ4** archiving and `check_os_locks = false` / `check_proc_fd = false`.
2. **Strategy B (`nginx-reopened-logs`)**: Cooperating Nginx logs rotated by `nginx -s reopen` (`access.log.1`, `error.log.1`), using **`rename_by_date` + high-ratio `brotli` compression** with zero tail-byte race window.

> **Trade-Off Advisory (`CopyTruncate`)**: Because `CopyTruncate` copies bytes from `src` to `<archive>.fc-tmp` and then calls `file.set_len(0)` without halting the external writer, any log bytes appended between copy EOF and `set_len(0)` are lost. Always prefer **Strategy B** (`rename_by_date` + `compress` after `SIGUSR1` reopen) whenever the service supports log reopening.

### Configuration (`/etc/filecleaner/logs-rotation.yaml`)

```yaml
dry_run: false

audit:
  log_path: /var/log/filecleaner/logs-audit.jsonl
  jsonl: true

rules:
  # Strategy A: Explicit opt-in CopyTruncate for non-cooperating open O_APPEND logs
  - name: app-logs-copy-truncate
    path: /var/log/legacy-app
    recursive: false
    include_globs:
      - "*.log"
    min_file_size: 100MiB
    time_basis: modified
    max_age: 1d
    max_total_size: 2GiB
    combine_mode: any
    eviction_order: largest_first
    in_use:
      enabled: true
      check_sidecar_locks: true
      # Disable OS handle/FD probes because legacy-app intentionally holds app.log open
      check_os_locks: false
      check_proc_fd: false
      min_idle_age: 5s
      stability_window: 0s
    actions:
      - copy_truncate
    copy_truncate_to: /var/log/legacy-app/rotated
    compression: lz4

  # Strategy B: Cooperating Nginx logs -> RenameByDate + Brotli compression
  - name: nginx-reopened-logs
    path: /var/log/nginx
    recursive: true
    max_depth: 2
    include_globs:
      - "*.log.[0-9]*"
      - "*.log.rotated"
    exclude_globs:
      - "archive/**"
    time_basis: modified
    max_files: 30
    max_age: 14d
    max_total_size: 10GiB
    combine_mode: any
    eviction_order: oldest_first
    in_use:
      enabled: true
      check_os_locks: true
      check_proc_fd: true
      min_idle_age: 30s
      stability_window: 2s
    actions:
      - rename_by_date
      - compress
      - move
    rename_template: "%Y%m%d_%H%M%S"
    compression: brotli
    compression_level: 6
    compress_remove_source: true
    move_to: /var/log/nginx/archive
    move_date_subdir: "%Y/%m"
```

Notice `exclude_globs: ["archive/**"]` on the recursive `nginx-reopened-logs` rule satisfies `INV-CFG-09` so `/var/log/nginx/archive` is never re-scanned.

### Validation, Dry-Run Plan & Expected Output

```bash
filecleaner validate -c /etc/filecleaner/logs-rotation.yaml
filecleaner plan -c /etc/filecleaner/logs-rotation.yaml --json
```

Expected `validate` & `plan --json` excerpt:

```text
warning: rule 'app-logs-copy-truncate' enables CopyTruncate; bytes written between archive copy EOF and truncate may be lost for non-cooperating writers
configuration valid: 2 rule(s) loaded from /etc/filecleaner/logs-rotation.yaml
```

```json
{"ts":"2026-11-20T09:15:00.044Z","event":"file_decision","run_id":"plan-9012-1","rule":"nginx-reopened-logs","path":"/var/log/nginx/access.log.1","size_bytes":268435456,"resolved_ts":"2026-11-19T00:00:05Z","decision":"would_act","in_use_reasons":null,"actions":["RenameByDate","Compress(Brotli)","Move"],"dest_path":"/var/log/nginx/archive/2026/11/access.log_20261119_000005.1.br","sha256":null,"dry_run":true,"duration_ms":3,"error":null}
```

---

## 3. Recipe 3: High-Throughput ETL Incoming Spool

### Operational Goal

Clean up processed Parquet/CSV batches in `/var/spool/etl/incoming` **only** once every producer and downstream warehouse consumer has finished:

- Protect active uploads with custom sidecars (`.part`, `.lock`, `.uploading`, `.processing`), Unix `flock`, Linux `/proc/*/fd`, `min_idle_age = "30s"`, and a batched 3-second `(size, mtime)` `stability_window`.
- Attach a **`pre_delete_hook`** verifying that a matching `.sha256.verified` ingestion receipt exists and matches the file's SHA-256 digest before moving or deleting.
- If the receipt is missing (warehouse ingestion still pending), the hook exits **`42`** (graceful veto skip, keeping sweep exit code `0`). If the checksum mismatches, it exits **`1`** (`abort_file`) and escalates an operational error.

### Hook Script (`/usr/local/bin/verify_etl_manifest.sh`)

```bash
#!/usr/bin/env bash
set -euo pipefail

# FileCleaner executes hook commands directly without a shell (argv + FILECLEANER_* env).
TARGET="${FILECLEANER_PATH:?missing FILECLEANER_PATH}"
RECEIPT="${TARGET}.sha256.verified"

# Return exit 42 (skip_exit_code) to veto eviction safely if ingestion receipt is absent
if [[ ! -f "${RECEIPT}" ]]; then
  echo "veto: ingestion receipt missing for ${FILECLEANER_NAME} (rule=${FILECLEANER_RULE})" >&2
  exit 42
fi

EXPECTED_HASH="$(awk '{print $1}' "${RECEIPT}")"
ACTUAL_HASH="$(sha256sum "${TARGET}" | awk '{print $1}')"

if [[ -z "${EXPECTED_HASH}" || "${EXPECTED_HASH}" != "${ACTUAL_HASH}" ]]; then
  echo "error: checksum mismatch on ${TARGET} (expected=${EXPECTED_HASH}, actual=${ACTUAL_HASH})" >&2
  exit 1
fi

exit 0
```

Make executable (`chmod 0755 /usr/local/bin/verify_etl_manifest.sh`).

### Configuration (`/etc/filecleaner/etl-spool.toml`)

```toml
dry_run = false
global_trash_dir = "/var/spool/etl/.quarantine"

[audit]
log_path = "/var/log/filecleaner/etl-spool-audit.jsonl"
jsonl = true

[[rules]]
name = "etl-incoming-parquet"
path = "/var/spool/etl/incoming"
recursive = true
max_depth = 3
follow_symlinks = false
include_globs = ["*.parquet", "*.csv"]
exclude_globs = ["*.sha256.verified", "quarantine/**"]
min_file_size = "1B"
time_basis = "modified"
max_age = "6h"
max_total_size = "200GiB"
combine_mode = "all"
eviction_order = "oldest_first"
actions = ["compress", "trash"]
compression = "lz4"
compress_remove_source = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
sidecar_suffixes = [".lock", ".lck", ".part", ".tmp", ".processing", ".uploading"]
check_os_locks = true
check_proc_fd = true
min_idle_age = "30s"
stability_window = "3s"

[rules.pre_delete_hook]
command = "/usr/local/bin/verify_etl_manifest.sh"
args = ["--file", "{path}", "--size", "{size}", "--rule", "{rule}"]
timeout = "10s"
on_failure = "abort_file"
skip_exit_code = 42
run_in_dry_run = false
```

### Verification & Expected Outcomes

```bash
# Single-file shell probe before manual job
filecleaner check-in-use /var/spool/etl/incoming/batch_20261120_0900.parquet --idle-age 30s --stability 3s --json
echo "exit=$?"   # 10 if busy (e.g., .part sidecar or flock held), 0 if settled

# Sweep execution
filecleaner run -c /etc/filecleaner/etl-spool.toml
```

Expected `check-in-use` output for an actively transferring batch (`exit 10`):

```json
{"all_safe":false,"busy_count":1,"reports":[{"path":"/var/spool/etl/incoming/batch_20261120_0900.parquet","is_busy":true,"reasons":[{"kind":"sidecar_lock","lock_path":"/var/spool/etl/incoming/batch_20261120_0900.parquet.part"},{"kind":"min_idle_age","age_ms":4120,"min_idle_ms":30000}]}]}
```

Expected JSONL audit events showing a busy file skip (`skip_in_use`), an exit-`42` hook veto (`skip_hook`), and a verified archival (`acted`):

```json
{"ts":"2026-11-20T09:20:03.012Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261120_0900.parquet","size_bytes":67108864,"resolved_ts":"2026-11-20T09:19:58Z","decision":"skip_in_use","in_use_reasons":["SidecarLock","MinIdleAge"],"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":3002,"error":null}
{"ts":"2026-11-20T09:20:03.040Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261120_0100.parquet","size_bytes":134217728,"resolved_ts":"2026-11-20T01:00:00Z","decision":"skip_hook","in_use_reasons":null,"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":14,"error":"hook returned skip_exit_code=42"}
{"ts":"2026-11-20T09:20:03.590Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261119_2200.parquet","size_bytes":134217728,"resolved_ts":"2026-11-19T22:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Lz4)","Trash"],"dest_path":"/var/spool/etl/.quarantine/batch_20261119_2200.parquet.lz4","sha256":null,"dry_run":false,"duration_ms":548,"error":null}
```

---

## 4. Recipe 4: Multi-Rule Daemon Deployment

### Operational Goal

Run a continuous, multi-rule daemon on a production node combining:

- **`Hybrid` scheduling** (`mode = "hybrid"`, `interval = "5m"`, `watch = true`, `debounce = "2s"`) for immediate response to bursts plus guaranteed wall-clock age/staleness sweeps.
- Atomic **PID**, live JSON **state**, and cooperative **stop** control files under `/run/filecleaner/`.
- Multiple independent rules (`db-backups`, `nginx-logs`, `tmp-build-artifacts`) with isolated `BackupGuard` and `InUse` settings.
- Automated alerting on `--fail-on-freeze` / `--strict-constraints` (exit code `3`) plus live daemon status inspection.

### Configuration (`/etc/filecleaner/production-daemon.toml`)

```toml
dry_run = false
global_trash_dir = "/var/tmp/filecleaner-quarantine"

[daemon]
mode = "hybrid"
interval = "5m"
watch = true
debounce = "2s"
sweep_on_start = true
pid_file = "/run/filecleaner/filecleaner.pid"
state_file = "/run/filecleaner/state.json"
stop_file = "/run/filecleaner/filecleaner.stop"

[audit]
log_path = "/var/log/filecleaner/daemon-audit.jsonl"
jsonl = true

[[rules]]
name = "db-backups"
path = "/var/backups/postgres"
include_globs = ["*.sql", "*.dump"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_total_size = "40GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
move_to = "/mnt/cold-nas/postgres"
move_date_subdir = "%Y/%m"

[rules.backup_guard]
enabled = true
min_keep_files = 5
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[[rules]]
name = "nginx-logs"
path = "/var/log/nginx"
include_globs = ["*.log.[0-9]*"]
max_age = "14d"
max_total_size = "10GiB"
actions = ["compress", "move"]
compression = "brotli"
move_to = "/var/archive/nginx"

[rules.in_use]
enabled = true
min_idle_age = "30s"
stability_window = "2s"

[[rules]]
name = "tmp-build-artifacts"
path = "/var/cache/ci-artifacts"
recursive = true
max_depth = 3
max_age = "48h"
max_total_size = "25GiB"
combine_mode = "any"
eviction_order = "largest_first"
actions = ["delete"]

[rules.in_use]
enabled = true
min_idle_age = "2m"
stability_window = "2s"
```

### Daemon Deployment, Health Check & Strict Alerting

```bash
# 1. Validate configuration and generate systemd service unit
filecleaner validate -c /etc/filecleaner/production-daemon.toml
filecleaner daemon service-template --init-system systemd \
  --config /etc/filecleaner/production-daemon.toml \
  --output /etc/systemd/system/filecleaner.service

sudo systemctl daemon-reload
sudo systemctl enable --now filecleaner

# 2. Inspect live daemon status & metrics
filecleaner daemon status -c /etc/filecleaner/production-daemon.toml --json

# 3. Periodic health-probe wrapper (cron / Nagios / systemd timer)
# Returns exit 3 immediately if any rule is frozen or constrained!
filecleaner plan -c /etc/filecleaner/production-daemon.toml \
  --fail-on-freeze --strict-constraints --json

# 4. Bounded CI smoke test (runs exactly 1 sweep then exits cleanly)
filecleaner daemon run -c /etc/filecleaner/production-daemon.toml --max-sweeps 1 --dry-run

# 5. Graceful cross-platform shutdown (finishes active file cleanly, removes PID & stop_file)
filecleaner daemon stop -c /etc/filecleaner/production-daemon.toml --timeout 30s
```

### Expected Live `state.json` Output

```json
{
  "version": "1.0.0",
  "pid": 42180,
  "status": "idle",
  "watch_mode": "hybrid",
  "started_at": "2026-11-20T09:00:00.000Z",
  "last_sweep_at": "2026-11-20T09:05:00.214Z",
  "next_sweep_at": "2026-11-20T09:10:00.000Z",
  "sweeps_completed": 2,
  "total_files_evaluated": 312,
  "total_files_processed": 27,
  "total_bytes_reclaimed": 4831838208,
  "active_freezes": [],
  "constrained_rules": ["db-backups"],
  "last_error": null
}
```

In this example, `constrained_rules: ["db-backups"]` signals that keeping the mandatory `min_keep_files = 5` local backups exceeded the `40GiB` quota—FileCleaner protected all `5` newest valid dumps, pruned everything older, and logged the constraint clearly so an operator can either expand `/var/backups/postgres` or lower `min_keep_files`.

---

## 5. Recipe 5: PostgreSQL Base Backups & WAL Spool Archival to S3 / MinIO

### Operational Goal

Archive PostgreSQL base dumps and 16 MiB WAL segments directly to AWS S3 or an on-premise MinIO / Ceph RGW object store with zero C/C++ dependencies and strict fail-closed integrity:

- **Starter generation**: Generate a safe `dry_run = true` baseline instantly via `filecleaner init --profile dba-postgres --format toml` (canonical examples shipped in `examples/dba-postgres-wal-s3.toml` and `examples/dba-postgres-wal-s3.yaml`).
- **Fail-closed SHA-256 S3 archival (`upload_s3`)**: Compresses eligible base dumps (`gzip`) and 16 MiB WAL segments (`lz4`), streams them via pure-Rust AWS SigV4 (`ureq` v3 + `rustls-rustcrypto`), verifies remote `x-amz-checksum-sha256` and `Content-Length` via `HeadObject`, and executes `delete` **only after** cryptographic verification succeeds.
- **Zero plaintext secrets**: S3 credentials are injected exclusively through environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN`) and masked in memory (`RedactedSecret`).
- **PostgreSQL PITR & timeline safety**: Matches only canonical 24-hex WAL segments (`^[0-9A-F]{24}$`, `min_file_size = "16MiB"`), unconditionally excludes `*.history`, `*.partial`, `*.ready`, `*.done`, `*.tmp`, and `archive_status/**`, and attaches `examples/hooks/verify_pg_wal_safe.sh` (`exit 42` veto for segments held by active base backups or lagging standby replicas).

### Configuration (`/etc/filecleaner/dba-postgres-wal-s3.toml`)

```toml
dry_run = false

[audit]
log_path = "/var/log/filecleaner/pg-wal-s3-audit.jsonl"
jsonl = true

[s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
force_path_style = true
allow_insecure_http = false
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "AWS_ACCESS_KEY_ID"
secret_key_env = "AWS_SECRET_ACCESS_KEY"
session_token_env = "AWS_SESSION_TOKEN"
timeout = "60s"
max_retries = 3
multipart_part_size = "16MiB"

[[rules]]
name = "pg-base-backups"
path = "/var/backups/postgres/base"
recursive = true
max_depth = 2
follow_symlinks = false
include_globs = ["*.sql", "*.sql.gz", "*.dump", "*.tar"]
exclude_globs = ["pinned/**", "*.keep"]
min_file_size = "1KiB"
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_age = "30d"
max_total_size = "200GiB"
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"

[rules.s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
prefix = "postgres/base-backups"
key_date_subdir = "%Y/%m"
force_path_style = true
overwrite = false
verify_mode = "checksum_sha256"

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "2m"
stability_window = "5s"

[[rules]]
name = "pg-wal-archive"
path = "/var/lib/postgresql/wal_archive"
recursive = false
follow_symlinks = false
include_regex = '^[0-9A-F]{24}$'
exclude_globs = ["*.history", "*.partial", "*.ready", "*.done", "*.tmp", "archive_status/**"]
min_file_size = "16MiB"
time_basis = "modified"
max_files = 64
max_age = "7d"
max_total_size = "100GiB"
actions = ["compress", "upload_s3", "delete"]
compression = "lz4"

[rules.s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
prefix = "postgres/wal"
key_date_subdir = "%Y/%m/%d"
force_path_style = true
overwrite = false
verify_mode = "checksum_sha256"

[rules.backup_guard]
enabled = true
min_keep_files = 16
min_valid_backup_bytes = "16MiB"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "60s"
stability_window = "3s"

[rules.pre_delete_hook]
command = "/usr/local/bin/verify_pg_wal_safe.sh"
args = ["--file", "{path}", "--name", "{name}", "--rule", "{rule}"]
timeout = "15s"
on_failure = "abort_file"
skip_exit_code = 42
run_in_dry_run = false

[rules.pre_delete_hook.env]
FILECLEANER_PG_MIN_KEEP_WAL_FILE = "/var/lib/postgresql/wal_archive/.pitr_min_keep_wal"
```

### Verification, Live Execution & Docker E2E Lab

```bash
# 1. Generate starter DBA profile (dry_run = true by default)
filecleaner init --profile dba-postgres --format toml --output /etc/filecleaner/dba-postgres-wal-s3.toml

# 2. Export S3 credentials via environment variables (never store secrets in TOML/YAML)
export AWS_ACCESS_KEY_ID="minio-archive-writer"
export AWS_SECRET_ACCESS_KEY="<injected-from-vault>"

# 3. Validate invariants (INV-CFG-01..14) and inspect zero-I/O S3 URI dry-run plan
filecleaner validate /etc/filecleaner/dba-postgres-wal-s3.toml
filecleaner plan -c /etc/filecleaner/dba-postgres-wal-s3.toml --json

# 4. Run live fail-closed archival sweep (exit 3 alerts on staleness freeze; exit 1 on S3/checksum fault)
filecleaner run -c /etc/filecleaner/dba-postgres-wal-s3.toml --fail-on-freeze

# 5. Reproducible live Docker E2E lab (PostgreSQL 17 compatible with multi-db-docker-env + pinned MinIO)
bash scripts/e2e-dba-s3-lab.sh
# Or via Cargo ignored E2E suite:
cargo test --test e2e_dba_s3_docker_test -- --ignored --nocapture
```

### Expected S3 Audit Record (`audit.jsonl`)

```json
{"ts":"2026-11-22T03:04:01.412Z","event":"action_exec","run_id":"run-7681-1","rule":"pg-wal-archive","path":"/var/lib/postgresql/wal_archive/000000010000000000000001","size_bytes":16777216,"resolved_ts":"2026-11-21T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Lz4)","UploadS3","Delete"],"dest_path":"s3://pg-pitr-archive/postgres/wal/2026/11/21/000000010000000000000001.lz4","sha256":"dac906d5f1872d44e1d0f0b47c8e9c9a21f42c44d8f28a391b7c98f3b21c5e12","details":{"etag":"\"597539973be5e225\"","parts_uploaded":1,"s3_uri":"s3://pg-pitr-archive/postgres/wal/2026/11/21/000000010000000000000001.lz4","verified_size":84210,"verify_mode":"checksum_sha256"},"dry_run":false,"duration_ms":64,"error":null}
```
