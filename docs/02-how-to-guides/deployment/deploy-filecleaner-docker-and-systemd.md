# Deploy FileCleaner with Docker and systemd

Refer to
[CLI Commands and Exit Codes](../../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
before you start this procedure.

## Prerequisites

1. Make sure that you have `sudo` or root access on the Linux host.
2. Make sure that Docker Engine or `systemd` is active on the host.
3. Prepare `/etc/filecleaner`, `/var/lib/filecleaner`, and `/var/log/filecleaner` directories.

## Safety Instructions

**CAUTION:** Set `dry_run = true` in new configuration files before you start the service.
Rules with `dry_run = false` immediately compress, move, or delete files.

## Procedure

1. Create `/etc/filecleaner/filecleaner.toml` with `dry_run = true` and a validated rule:

   ```toml
   dry_run = true
   global_trash_dir = "/var/tmp/filecleaner-trash"

   [daemon]
   mode = "hybrid"
   interval = "5m"
   watch = true
   debounce = "2s"
   sweep_on_start = true
   pid_file = "/var/lib/filecleaner/filecleaner.pid"
   state_file = "/var/lib/filecleaner/state.json"
   stop_file = "/var/lib/filecleaner/filecleaner.stop"

   [audit]
   log_path = "/var/log/filecleaner/audit.jsonl"
   jsonl = true

   [[rules]]
   name = "app-backups"
   path = "/var/backups/app"
   recursive = false
   follow_symlinks = false
   include_globs = ["*.sql", "*.dump"]
   max_files = 14
   max_age = "30d"
   actions = ["compress", "move"]
   compression = "gzip"
   move_to = "/mnt/cold-archive/app"

   [rules.backup_guard]
   enabled = true
   min_keep_files = 3
   min_valid_backup_bytes = "1MiB"
   require_newer_within = "36h"
   freeze_on_empty_valid = true
   ```

2. Restrict permissions on the configuration file to owner read and write:

   ```bash
   sudo chmod 0600 /etc/filecleaner/filecleaner.toml
   ```

3. Validate the configuration file before you start any container or service:

   ```bash
   filecleaner validate --config /etc/filecleaner/filecleaner.toml
   ```

4. Run a read-only sweep plan to examine projected decisions:

   ```bash
   filecleaner plan --config /etc/filecleaner/filecleaner.toml --json
   ```

5. For a Docker deployment, set UID `10001` and GID `10001` on the configuration and host volumes:

   ```bash
   sudo chown 10001:10001 /etc/filecleaner/filecleaner.toml
   sudo chown -R 10001:10001 /var/lib/filecleaner /var/log/filecleaner /var/backups/app /mnt/cold-archive/app
   ```

6. Pull the official Docker container image:

   ```bash
   docker pull jmrenouard/filecleaner:1.1.0
   ```

7. Start container `filecleaner-daemon` with the mounted configuration and volume paths:

   ```bash
   docker run -d \
     --name filecleaner-daemon \
     --restart unless-stopped \
     -v /etc/filecleaner:/etc/filecleaner:ro \
     -v /var/backups/app:/var/backups/app \
     -v /mnt/cold-archive/app:/mnt/cold-archive/app \
     -v /var/lib/filecleaner:/var/lib/filecleaner \
     -v /var/log/filecleaner:/var/log/filecleaner \
     jmrenouard/filecleaner:1.1.0 \
     daemon run --config /etc/filecleaner/filecleaner.toml
   ```

8. For a host `systemd` deployment, generate `/etc/systemd/system/filecleaner.service` and enable the unit:

   ```bash
   sudo filecleaner daemon service-template \
     --init-system systemd \
     --config /etc/filecleaner/filecleaner.toml \
     --output /etc/systemd/system/filecleaner.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now filecleaner
   ```

9. Change `dry_run = true` to `dry_run = false` in `/etc/filecleaner/filecleaner.toml`, then restart the service.

**NOTE:** The official Docker image runs as non-root UID and GID `10001`.
Mounted host files and directories must permit UID `10001` access.

## Verification

1. Examine container or `systemd` unit status:

   ```bash
   docker ps --filter name=filecleaner-daemon
   sudo systemctl status filecleaner --no-pager
   ```

2. Run `filecleaner daemon status` and confirm exit code `0`:

   ```bash
   filecleaner daemon status --config /etc/filecleaner/filecleaner.toml --json
   ```

3. Examine `/var/log/filecleaner/audit.jsonl` and make sure that `sweep_end` records appear.

## Rollback

If an error occurs after deployment, do these steps:

1. Stop the Docker container or `systemd` service:

   ```bash
   docker stop filecleaner-daemon && docker rm filecleaner-daemon
   sudo systemctl disable --now filecleaner
   ```

2. If a stale PID remains, run `filecleaner daemon stop` with a timeout:

   ```bash
   filecleaner daemon stop --config /etc/filecleaner/filecleaner.toml --timeout 15s
   ```

3. Replace `/etc/filecleaner/filecleaner.toml` with the previous valid configuration file.
4. Start the previous service version and confirm healthy `daemon status` output.

## Related Documentation

- Reference: [CLI Commands and Exit Codes](../../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
- Reference: [Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
- Explanation:
  [Fail-Closed 6-Phase Pipeline](../../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
- Explanation:
  [ADR-001: Pure-Rust Fail-Closed Engine](../../04-explanation/design-decisions/adr-001-pure-rust-fail-closed-engine.md)
