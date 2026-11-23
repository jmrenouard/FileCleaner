# Enforce Pre-Delete Hooks and TOCTOU Guards

Refer to
[Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
before you change security controls.

## Prerequisites

1. Make sure that `filecleaner` v1.1.0 or newer is installed on the host.
2. Prepare a dedicated hook script path such as `/usr/local/bin/verify_checksum_before_delete.sh`.
3. Make sure that the hook script returns `0` to permit action, `42` to skip, and `1` to abort.

## Safety Instructions

**CAUTION:** Keep `follow_symlinks = false` on all production rules.
Symbolic link traversal can expose files outside the configured rule directory.

**CAUTION:** Invoke the hook script directly in `pre_delete_hook.command`.
Never pass `sh -c` or `cmd /C` wrappers that permit filename shell injection.

## Procedure

1. Install the checksum veto hook script and set executable permissions:

   ```bash
   sudo install -m 0755 examples/hooks/verify_checksum_before_delete.sh /usr/local/bin/verify_checksum_before_delete.sh
   ```

2. Create `/etc/filecleaner/secure-hooks.toml` with symlink checks, six in-use probes, and `[rules.pre_delete_hook]`:

   ```toml
   dry_run = true
   global_trash_dir = "/var/tmp/filecleaner-quarantine"

   [audit]
   log_path = "/var/log/filecleaner/security-audit.jsonl"
   jsonl = true

   [[rules]]
   name = "verified-etl-spool"
   path = "/var/spool/etl/incoming"
   recursive = true
   max_depth = 3
   follow_symlinks = false
   include_globs = ["*.parquet", "*.csv"]
   exclude_globs = ["*.sha256", "quarantine/**"]
   time_basis = "modified"
   max_age = "12h"
   max_total_size = "20GiB"
   actions = ["compress", "trash"]
   compression = "lz4"
   trash_dir = "/var/tmp/filecleaner-quarantine/etl"

   [rules.in_use]
   enabled = true
   check_sidecar_locks = true
   sidecar_suffixes = [".lock", ".lck", ".part", ".tmp", ".processing"]
   check_os_locks = true
   check_proc_fd = true
   min_idle_age = "30s"
   stability_window = "3s"

   [rules.pre_delete_hook]
   command = "/usr/local/bin/verify_checksum_before_delete.sh"
   args = ["--strict", "{path}", "{size}"]
   timeout = "15s"
   on_failure = "abort_rule"
   skip_exit_code = 42
   run_in_dry_run = false
   ```

3. Set private `0600` permissions on `/etc/filecleaner/secure-hooks.toml`:

   ```bash
   sudo chmod 0600 /etc/filecleaner/secure-hooks.toml
   ```

4. Validate the configuration file with `filecleaner validate`:

   ```bash
   filecleaner validate --config /etc/filecleaner/secure-hooks.toml
   ```

5. Test an active file with `filecleaner check-in-use` before any live sweep:

   ```bash
   filecleaner check-in-use /var/spool/etl/incoming/sample.parquet --idle-age 30s --stability 3s --json
   ```

6. Change `dry_run = true` to `dry_run = false`, then run a live sweep:

   ```bash
   filecleaner run --config /etc/filecleaner/secure-hooks.toml
   ```

**NOTE:** FileCleaner opens candidate files with `O_NOFOLLOW` on Unix and `FILE_FLAG_OPEN_REPARSE_POINT` on Windows.
Before truncation or S3 upload, FileCleaner also checks device and inode continuity.

## Verification

1. Confirm `filecleaner validate` exits with code `0`.
2. Place a `.part` sidecar beside a test file and confirm `filecleaner check-in-use` exits with code `10`.
3. Remove the `.sha256` manifest on a settled file and confirm `"decision":"skip_hook"` in `security-audit.jsonl`.

## Rollback

If the hook blocks valid archival or causes rule aborts, do these steps:

1. Set `dry_run = true` in `/etc/filecleaner/secure-hooks.toml`.
2. Restore the previous hook script under `/usr/local/bin/verify_checksum_before_delete.sh`.
3. Run `filecleaner plan --config /etc/filecleaner/secure-hooks.toml --json` before you restore `dry_run = false`.

## Related Documentation

- Reference: [Configuration Schema and Invariants](../../03-reference/configuration/config-schema-and-invariants.md)
- Reference: [Error and Alert Matrix](../../03-reference/troubleshooting-codes/error-and-alert-matrix.md)
- Explanation:
  [Retention Calculus and TOCTOU Defenses](../../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
- Explanation:
  [ADR-001: Pure-Rust Fail-Closed Engine](../../04-explanation/design-decisions/adr-001-pure-rust-fail-closed-engine.md)
