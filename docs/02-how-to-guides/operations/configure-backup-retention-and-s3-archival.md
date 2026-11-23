# Configure Backup Retention and S3 Archival

Refer to
[Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
before you edit the configuration file.

## Prerequisites

1. Make sure that `filecleaner` v1.1.0 or newer is active on the host.
2. Prepare your target S3 bucket name, region, HTTPS endpoint, and access credentials.
3. Make sure that the service account can read and write files in local backup and log directories.

## Safety Instructions

**CAUTION:** Supply S3 credentials only through environment variables such as `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`. Plaintext credentials inside TOML or YAML files create a credential leak risk.

**CAUTION:** Keep `verify_mode = "checksum_sha256"` and `overwrite = false` on production rules.
Disabled checksum verification removes fail-closed protection against remote corruption.

## Procedure

1. Generate a starter DBA PostgreSQL configuration file with `filecleaner init`:

   ```bash
   filecleaner init --profile dba-postgres --format toml --output /etc/filecleaner/pg-s3.toml
   ```

2. Edit `/etc/filecleaner/pg-s3.toml` with your bucket, HTTPS endpoint, and retention limits:

   ```toml
   dry_run = true

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
   include_globs = ["*.sql", "*.sql.gz", "*.dump"]
   exclude_globs = ["pinned/**", "*.keep"]
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
   ```

3. Set restrictive file permissions on `/etc/filecleaner/pg-s3.toml`:

   ```bash
   sudo chmod 0600 /etc/filecleaner/pg-s3.toml
   ```

4. Export S3 credentials in the runtime environment or protected environment file:

   ```bash
   export AWS_ACCESS_KEY_ID="<access-key>"
   export AWS_SECRET_ACCESS_KEY="<secret-key>"
   ```

5. Validate all 14 configuration invariants with `filecleaner validate`:

   ```bash
   filecleaner validate --config /etc/filecleaner/pg-s3.toml
   ```

6. Execute a read-only plan to inspect projected S3 URIs and protected floor counts:

   ```bash
   filecleaner plan --config /etc/filecleaner/pg-s3.toml --fail-on-freeze --json
   ```

7. Change `dry_run = true` to `dry_run = false`, then execute a live archival sweep:

   ```bash
   filecleaner run --config /etc/filecleaner/pg-s3.toml --fail-on-freeze
   ```

**NOTE:** FileCleaner runs `Delete` only after `HeadObject` or `GetObject` SHA-256 verification succeeds.

## Verification

1. Confirm that `filecleaner run` exits with code `0`.
2. Inspect `/var/log/filecleaner/pg-wal-s3-audit.jsonl` for `action_exec` entries with `"verify_mode"`.
3. Confirm that the newest `min_keep_files` backups remain on local disk.

## Rollback

If archival produces unexpected deletions or errors, do these steps:

1. Set `dry_run = true` in `/etc/filecleaner/pg-s3.toml`.
2. If you want to pause only one rule, set `enabled = false` under that rule.
3. Download required archives back from `s3://pg-pitr-archive` into the local backup directory.
4. Run `filecleaner validate` and `filecleaner plan` after every configuration change.

## Related Documentation

- Reference: [Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
- Reference: [Error and Alert Matrix](../../03-reference/troubleshooting-codes/error-and-alert-matrix.md)
- Explanation:
  [Retention Calculus and TOCTOU Defenses](../../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
- Explanation:
  [ADR-001: Pure-Rust Fail-Closed Engine](../../04-explanation/design-decisions/adr-001-pure-rust-fail-closed-engine.md)
