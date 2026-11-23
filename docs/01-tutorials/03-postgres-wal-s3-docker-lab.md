# Tutorial 3: PostgreSQL WAL and S3 Docker Lab

> **Learning Goal:** Generate a DBA PostgreSQL WAL archival configuration, then run FileCleaner's self-cleaning
> 6-scenario Docker E2E lab (`scripts/e2e-dba-s3-lab.sh`) against live PostgreSQL 17 and MinIO containers.

> [!WARNING]
> **Isolated Local Docker Sandbox Only:** `scripts/e2e-dba-s3-lab.sh` creates ephemeral Docker containers, loopback HTTP
> MinIO endpoints, and disposable test credentials. Never use `allow_insecure_http = true` or ephemeral lab credentials
> on production object stores.

---

## Prerequisites

Make sure your environment meets these requirements:

- Cloned `FileCleaner` repository with `scripts/e2e-dba-s3-lab.sh`
- Docker Engine `24+` running on Linux / WSL2 with permission to run containers
- `python3`, `gzip`, and `bash` available on `PATH`
- Compiled `filecleaner` binary (or Rust toolchain so the script builds it in `/tmp/filecleaner-target`)

---

## What You Will Build

By the end of this tutorial, you will have:

1. Generated and validated a starter `--profile dba-postgres` configuration (`dry_run = true`).
2. Inspected a local MinIO-compatible S3 archival configuration combining `compress` (`gzip`/`lz4`), `upload_s3`, and
   `delete`.
3. Executed `scripts/e2e-dba-s3-lab.sh` covering real 16 MiB PostgreSQL 17 WAL segments, `flock` writer protection,
   fail-closed SHA-256 S3 uploads, outage and corrupt-checksum fault injection, staleness freezes, and a `253`-row
   PostgreSQL restore.

---

## Step 1: Generate and Validate the DBA Starter Configuration

Generate a safe starter configuration using `--profile dba-postgres`:

```bash
mkdir -p /tmp/fc-pg-lab
filecleaner init --profile dba-postgres --format toml --output /tmp/fc-pg-lab/dba-starter.toml
filecleaner validate /tmp/fc-pg-lab/dba-starter.toml
```

Expected output:

```text
initialized dba-postgres starter configuration at /tmp/fc-pg-lab/dba-starter.toml (dry_run = true)
configuration valid: 2 rule(s) loaded from /tmp/fc-pg-lab/dba-starter.toml
```

Notice that `/tmp/fc-pg-lab/dba-starter.toml` creates two rules (`pg-base-backups` and `pg-wal-archive`) with
`dry_run = true` and environment-variable S3 credentials (`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`).

---

## Step 2: Inspect a Local MinIO S3 Lab Configuration

When testing against a local loopback MinIO container (`http://127.0.0.1:9000`), path-style addressing and explicit
`allow_insecure_http = true` are required (`INV-CFG-13`):

```toml
dry_run = true

[audit]
log_path = "/tmp/fc-pg-lab/audit.jsonl"
jsonl = true

[s3]
bucket = "fc-e2e-pg-archive"
region = "us-east-1"
endpoint = "http://127.0.0.1:9000"
force_path_style = true
allow_insecure_http = true
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "FC_E2E_S3_ACCESS_KEY"
secret_key_env = "FC_E2E_S3_SECRET_KEY"
timeout = "30s"
max_retries = 1
multipart_part_size = "16MiB"

[[rules]]
name = "pg-base-backups"
path = "/tmp/fc-pg-lab/pg_base"
include_globs = ["pg_*.sql"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 2
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "1KiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[[rules]]
name = "pg-wal-archive"
path = "/tmp/fc-pg-lab/pg_wal"
include_regex = '^[0-9A-F]{24}$'
exclude_globs = ["*.history", "*.partial", "*.ready", "*.done", "*.tmp", "archive_status/**"]
min_file_size = "16MiB"
time_basis = "modified"
max_files = 2
actions = ["compress", "upload_s3", "delete"]
compression = "lz4"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "16MiB"
freeze_on_empty_valid = true
```

Save that snippet as `/tmp/fc-pg-lab/minio-lab.toml` and verify zero-network `validate` and `plan`:

```bash
filecleaner validate /tmp/fc-pg-lab/minio-lab.toml
filecleaner plan -c /tmp/fc-pg-lab/minio-lab.toml --json
```

---

## Step 3: Run the 6-Scenario Live Docker E2E Lab

From the `FileCleaner` repository root, launch `scripts/e2e-dba-s3-lab.sh`:

```bash
bash scripts/e2e-dba-s3-lab.sh
```

> [!TIP]
> You can also run the same harness through Cargo's ignored E2E integration test:
> `CARGO_TARGET_DIR=/tmp/filecleaner-target cargo test --test e2e_dba_s3_docker_test -- --ignored --nocapture`.

Expected output:

```text
[e2e-dba-s3] Starting isolated Docker network, PostgreSQL 17 (archive_mode=on), and MinIO (pgsty/minio:RELEASE.2026-08-04T00-00-00Z)...
[e2e-dba-s3] Scenario 1: Generating real 16 MiB PostgreSQL WAL segments and pg_dump base backups...
[e2e-dba-s3] Scenario 2: Verifying active writer protection (check-in-use exit 10 + skip_in_use)...
[e2e-dba-s3] Scenario 3: Running live FileCleaner compress -> upload_s3 (SHA-256) -> delete against MinIO...
[e2e-dba-s3] Scenario 4: Injecting MinIO outage and corrupt-checksum server (asserting exit 1 & 100% local retention)...
[e2e-dba-s3] Scenario 5: Verifying dead-man's-switch freeze (--fail-on-freeze exit 3) on 0-byte stub...
[e2e-dba-s3] Scenario 6: Downloading archived dump & WAL from MinIO, verifying SHA-256, and restoring PostgreSQL...
Verified MinIO SHA-256 matches audit.jsonl: dump=... wal=...
[e2e-dba-s3] ALL 6 LIVE DOCKER E2E SCENARIOS PASSED (PostgreSQL 17 + MinIO S3 SHA-256 + Full Restore: 253 rows)!
```

During Scenario 4, notice how FileCleaner returns exit code `1` and preserves 100% of local files whenever MinIO is down
or returns a mismatched `x-amz-checksum-sha256` header (`IntegrityMismatch`).

---

## Step 4: Clean Up Lab Files

`scripts/e2e-dba-s3-lab.sh` automatically removes its containers, Docker network, and `/var/tmp/fc-e2e-*` directory on
exit via a `trap cleanup EXIT INT TERM` handler. Remove `/tmp/fc-pg-lab` as well:

```bash
rm -rf /tmp/fc-pg-lab
```

---

## Next Steps

- Deploy production S3 archival with TLS:
  [Configure Backup Retention and S3 Archival](../02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md)
- Inspect all `[s3]` fields and `INV-CFG-13`..`INV-CFG-14` rules:
  [Configuration Schema & Invariants](../03-reference/configuration/config-schema-and-invariants.md)
- Understand why FileCleaner uses pure-Rust `ureq` + `rustls-rustcrypto`:
  [ADR-001: Pure-Rust Fail-Closed Engine](../04-explanation/design-decisions/adr-001-pure-rust-fail-closed-engine.md)
