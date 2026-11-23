# Run Reactive Daemon and Rotate Logs

Refer to
[CLI Commands and Exit Codes](../../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
and
[Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
before you edit daemon settings.

## Prerequisites

1. Make sure that `/run/filecleaner` and `/var/log/filecleaner` exist with write permissions.
2. Make sure that `/var/log/nginx/archive` and `/var/log/myapp/rotated` directories exist or are writable.
3. Confirm whether your application reopens log files on `SIGUSR1` or keeps an open descriptor.

## Safety Instructions

**CAUTION:** Select `copy_truncate` only when the writer cannot reopen log files.
Log bytes written between copy completion and file truncation are lost permanently.

**CAUTION:** Exclude archive subfolders inside recursive rules with `exclude_globs`.
Otherwise `filecleaner validate` rejects the self-referential path with `INV-CFG-09`.

## Procedure

1. Create `/etc/filecleaner/daemon-logs.yaml` with `hybrid` mode and two log rotation rules:

   ```yaml
   dry_run: true

   daemon:
     mode: hybrid
     interval: 5m
     watch: true
     debounce: 2s
     sweep_on_start: true
     pid_file: /run/filecleaner/filecleaner.pid
     state_file: /run/filecleaner/state.json
     stop_file: /run/filecleaner/filecleaner.stop

   audit:
     log_path: /var/log/filecleaner/logs-audit.jsonl
     jsonl: true

   rules:
     - name: app-active-logs
       path: /var/log/myapp
       recursive: false
       follow_symlinks: false
       include_globs:
         - "*.log"
       min_file_size: 10MiB
       time_basis: modified
       max_total_size: 2GiB
       in_use:
         enabled: true
         check_sidecar_locks: true
         check_os_locks: false
         check_proc_fd: false
         min_idle_age: 5s
       actions:
         - copy_truncate
       copy_truncate_to: /var/log/myapp/rotated
       compression: lz4
       rename_template: "%Y%m%d_%H%M%S"

     - name: nginx-reopened-logs
       path: /var/log/nginx
       recursive: true
       max_depth: 2
       follow_symlinks: false
       include_globs:
         - "*.log.[0-9]*"
       exclude_globs:
         - "archive/**"
       time_basis: modified
       max_files: 30
       max_age: 14d
       actions:
         - rename_by_date
         - compress
         - move
       rename_template: "%Y%m%d_%H%M%S"
       compression: brotli
       compress_remove_source: true
       move_to: /var/log/nginx/archive
       move_date_subdir: "%Y/%m"
   ```

2. Run `filecleaner validate` and examine the expected `copy_truncate` advisory message on `stderr`:

   ```bash
   filecleaner validate --config /etc/filecleaner/daemon-logs.yaml
   ```

3. Run one bounded dry-run daemon cycle to test watcher setup and JSON state output:

   ```bash
   filecleaner daemon run --config /etc/filecleaner/daemon-logs.yaml --mode interval --max-sweeps 1 --dry-run
   ```

4. Change `dry_run: true` to `dry_run: false` in `/etc/filecleaner/daemon-logs.yaml`.

5. Start the daemon in `hybrid` mode under your process supervisor or in a dedicated terminal:

   ```bash
   filecleaner daemon run --config /etc/filecleaner/daemon-logs.yaml --mode hybrid
   ```

6. Stop the daemon cleanly when maintenance is necessary:

   ```bash
   filecleaner daemon stop --config /etc/filecleaner/daemon-logs.yaml --timeout 15s
   ```

**NOTE:** `AuditLogger` rotates `logs-audit.jsonl` to `logs-audit.jsonl.1` when size reaches `50 MiB`.
FileCleaner automatically excludes rotated `.1` audit files from rule scans.

## Verification

1. Run `filecleaner daemon status` and confirm `"running": true` in JSON output:

   ```bash
   filecleaner daemon status --config /etc/filecleaner/daemon-logs.yaml --json
   ```

2. Inspect `/var/log/nginx/archive` and `/var/log/myapp/rotated` for compressed `.br` and `.lz4` files.
3. Inspect `/run/filecleaner/state.json` and confirm `last_error` is `null`.

## Rollback

If log rotation drops active bytes or causes high I/O load, do these steps:

1. Stop the daemon cleanly:

   ```bash
   filecleaner daemon stop --config /etc/filecleaner/daemon-logs.yaml --timeout 15s
   ```

2. Set `dry_run: true` in `/etc/filecleaner/daemon-logs.yaml`.
3. Replace `copy_truncate` with `rename_by_date`, `compress`, and `move` after `SIGUSR1` log reopen.
4. Validate the updated configuration and restart the daemon.

## Related Documentation

- Reference: [CLI Commands and Exit Codes](../../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
- Reference: [Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
- Explanation:
  [Fail-Closed 6-Phase Pipeline](../../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
