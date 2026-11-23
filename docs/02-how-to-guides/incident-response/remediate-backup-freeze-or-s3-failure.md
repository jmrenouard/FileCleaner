# Remediate Backup Freeze or S3 Failure

Refer to the
[Error and Alert Matrix](../../03-reference/troubleshooting-codes/error-and-alert-matrix.md)
before you execute this runbook.

## Prerequisites

1. Make sure that you have terminal access to the affected FileCleaner host.
2. Collect the rule name, process exit code (`1` or `3`), and paths to `audit.jsonl` and `state.json`.
3. Confirm access to the upstream backup script logs and target S3 bucket status.

## Safety Instructions

**CAUTION:** Do not disable `backup_guard` or lower `min_keep_files` to clear exit code `3`.
If you disable `backup_guard` during a backup outage, FileCleaner can delete the last valid backup.

**CAUTION:** Do not delete local backups manually during an S3 upload error.
FileCleaner keeps local files intact until remote SHA-256 verification succeeds.

## Procedure

1. Run a verbose dry-run plan to identify every frozen, constrained, or failed rule:

   ```bash
   filecleaner -v plan --config /etc/filecleaner/filecleaner.toml --fail-on-freeze --strict-constraints --json
   ```

2. Inspect the most recent error, freeze, or constraint events in `audit.jsonl`:

   ```bash
   tail -n 50 /var/log/filecleaner/audit.jsonl
   ```

3. If `event` equals `"backup_freeze"` (`exit 3`), examine the backup source directory for `0`-byte or small files:

   ```bash
   ls -lh /var/backups/postgres
   ```

4. Repair the upstream backup cron or `pg_dump` job, then run one manual backup to produce a valid file.
5. Make sure that the new backup file size equals or exceeds `min_valid_backup_bytes`.
6. If `event` equals `"rule_constrained"` (`exit 3`), compare `kept_floor` and `busy_skipped_count` to caps.
7. Expand disk storage or increase `max_total_size` and `max_files` so retention limits fit `min_keep_files`.
8. If S3 upload fails (`exit 1`), confirm `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, endpoint URL, and TLS trust.
9. If your S3 gateway omits `x-amz-checksum-sha256` on `HeadObject`, set `verify_mode = "readback_sha256"`.
10. Re-run `filecleaner plan` with strict flags to confirm the fault clears:

    ```bash
    filecleaner plan --config /etc/filecleaner/filecleaner.toml --fail-on-freeze --strict-constraints
    ```

**NOTE:** Exit code `3` comes from `--fail-on-freeze` or `--strict-constraints`.
Exit code `1` indicates an I/O, S3 transport, or `IntegrityMismatch` error.

## Verification

1. Execute a live sweep with strict policy flags and confirm exit code `0`:

   ```bash
   filecleaner run --config /etc/filecleaner/filecleaner.toml --fail-on-freeze --strict-constraints
   ```

2. Inspect `state.json` or run `filecleaner daemon status --json` and confirm `active_freezes` is `[]`.
3. Inspect `audit.jsonl` and confirm zero new `"event":"error"` records.

## Rollback

If remediation triggers unexpected errors or new alerts, do these steps:

1. Stop any active FileCleaner daemon:

   ```bash
   filecleaner daemon stop --config /etc/filecleaner/filecleaner.toml --timeout 15s
   ```

2. Set `dry_run = true` in `/etc/filecleaner/filecleaner.toml`.
3. Restore the previous `/etc/filecleaner/filecleaner.toml` copy and escalate to database or storage administrators.

## Related Documentation

- Reference: [Error and Alert Matrix](../../03-reference/troubleshooting-codes/error-and-alert-matrix.md)
- Reference: [CLI Commands and Exit Codes](../../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
- Explanation:
  [Fail-Closed 6-Phase Pipeline](../../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
- Explanation:
  [Retention Calculus and TOCTOU Defenses](../../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
