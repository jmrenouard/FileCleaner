# Tutorial 2: Backup Guard and In-Use Lab

> **Learning Goal:** Test FileCleaner's 6-layer active-writer protection (`filecleaner check-in-use`) and
> `backup_guard` dead-man's-switch staleness freezes (`--fail-on-freeze` exit `3`) in an isolated sandbox.

> [!WARNING]
> **Isolated Local Sandbox Only:** Execute all commands inside `/tmp/fc-guard-lab`. Never hold locks or create dummy
> zero-byte stubs inside production backup directories.

---

## Prerequisites

Make sure your environment meets these requirements:

- `filecleaner` v1.1.0+ binary on `PATH`
- `python3` (used to hold an exclusive POSIX `flock` lock in the background)
- Unix shell (`bash`) with `truncate` and GNU `touch`

---

## What You Will Build

By the end of this tutorial, you will have:

1. Created synthetic `2 MiB` backup dumps and a `0`-byte failed dump stub inside `/tmp/fc-guard-lab`.
2. Simulated an active writer with `flock` and a `.part` sidecar marker, then observed `filecleaner check-in-use` exit
   with code `10`.
3. Executed a sweep protecting both the `min_keep_files` floor and the locked file.
4. Triggered a `backup_guard` dead-man's-switch freeze (`--fail-on-freeze` exit `3`) when only stale dumps and a
   `0`-byte stub remain.
5. Stopped the background locker process and removed `/tmp/fc-guard-lab`.

---

## Step 1: Create the Sandbox Backup Vault

Create `/tmp/fc-guard-lab` with three valid `2 MiB` backup files dated `5`, `3`, and `0` days ago:

```bash
rm -rf /tmp/fc-guard-lab
mkdir -p /tmp/fc-guard-lab/backups /tmp/fc-guard-lab/archive

OLD_STAMP=$(date -u -d "5 days ago" +%Y%m%d_%H%M%S)
MID_STAMP=$(date -u -d "3 days ago" +%Y%m%d_%H%M%S)
NOW_STAMP=$(date -u +%Y%m%d_%H%M%S)

truncate -s 2M "/tmp/fc-guard-lab/backups/pg_${OLD_STAMP}.sql"
truncate -s 2M "/tmp/fc-guard-lab/backups/pg_${MID_STAMP}.sql"
truncate -s 2M "/tmp/fc-guard-lab/backups/pg_${NOW_STAMP}.sql"
```

---

## Step 2: Probe an Active Writer with `filecleaner check-in-use`

Create a companion `.part` sidecar file and hold an exclusive `flock` on the oldest backup from a background Python
process:

```bash
BUSY_FILE="/tmp/fc-guard-lab/backups/pg_${OLD_STAMP}.sql"
touch "${BUSY_FILE}.part"

python3 - "$BUSY_FILE" <<'PY' &
import fcntl, sys, time
with open(sys.argv[1], "r+b") as fh:
    fcntl.flock(fh.fileno(), fcntl.LOCK_EX)
    time.sleep(120)
PY
LOCK_PID=$!
sleep 0.5
```

Probe both the busy file and the newest idle file:

```bash
filecleaner check-in-use "$BUSY_FILE" --json
echo "busy_exit=$?"

filecleaner check-in-use "/tmp/fc-guard-lab/backups/pg_${NOW_STAMP}.sql"
echo "safe_exit=$?"
```

Expected output:

```json
{
  "all_safe": false,
  "busy_count": 1,
  "reports": [
    {
      "path": "/tmp/fc-guard-lab/backups/pg_20261118_174428.sql",
      "is_busy": true,
      "reasons": [
        {
          "kind": "sidecar_lock",
          "lock_path": "/tmp/fc-guard-lab/backups/pg_20261118_174428.sql.part"
        },
        {
          "kind": "proc_fd_open",
          "pid": 12609
        }
      ]
    }
  ]
}
```

```text
busy_exit=10
SAFE: /tmp/fc-guard-lab/backups/pg_20261123_174428.sql
safe_exit=0
```

Exit code `10` unambiguously signals that at least one target file is currently busy.

---

## Step 3: Run with `backup_guard` Protection (`Constrained` Outcome)

Save `/tmp/fc-guard-lab/guard-lab.toml` with `min_keep_files = 2`, `min_valid_backup_bytes = "1MiB"`, and
`require_newer_within = "36h"`:

```toml
dry_run = false

[audit]
log_path = "/tmp/fc-guard-lab/audit.jsonl"
jsonl = true

[[rules]]
name = "lab-backups"
path = "/tmp/fc-guard-lab/backups"
enabled = true
recursive = false
follow_symlinks = false
include_globs = ["pg_*.sql"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 2
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
move_to = "/tmp/fc-guard-lab/archive"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "0s"
stability_window = "0s"
```

Validate the config and run with `--strict-constraints`:

```bash
filecleaner validate -c /tmp/fc-guard-lab/guard-lab.toml
filecleaner -v run -c /tmp/fc-guard-lab/guard-lab.toml --strict-constraints
echo "strict_exit=$?"
```

Expected output:

```text
[lab-backups] KeepFloor: /tmp/fc-guard-lab/backups/pg_20261123_174428.sql (protected in newest 2 valid backup(s) (min_keep_files=2, min_valid_backup_bytes=1048576))
[lab-backups] KeepFloor: /tmp/fc-guard-lab/backups/pg_20261120_174428.sql (protected in newest 2 valid backup(s) (min_keep_files=2, min_valid_backup_bytes=1048576))
[lab-backups] SkipInUse: /tmp/fc-guard-lab/backups/pg_20261118_174428.sql (skipped in-use: sidecar lock exists: /tmp/fc-guard-lab/backups/pg_20261118_174428.sql.part, open file descriptor held by pid 12609)
Run: run-12735-1 | Mode: LIVE
Rule         Outcome      Evaluated   Acted   Busy  HookSkip  Floor       Freed  Mode
------------------------------------------------------------------------------------------
lab-backups  constrained          3       0      1         0      2         0 B  live
------------------------------------------------------------------------------------------
Totals: 3 evaluated, 0 acted, 0 B freed
Constrained rules: lab-backups
  [constrained] lab-backups: max_files exceeded (3 > 2): protected by 2 floor backup(s) and 1 in-use skip(s)
strict_exit=3
```

Because two files are locked in `min_keep_files = 2` (`P_floor`) and the third file is in `S_busy`, `max_files = 2`
cannot be met safely. Safety invariants win, `0` files are evicted, the rule finishes as `constrained`, and
`--strict-constraints` exits with `3`.

---

## Step 4: Trigger a Dead-Man's-Switch Freeze (`0`-Byte Stub)

Release the lock on the oldest file, then simulate a silent cron backup failure by replacing today's valid `2 MiB` dump
with a `0`-byte stub:

```bash
kill "$LOCK_PID" 2>/dev/null || true
wait "$LOCK_PID" 2>/dev/null || true
rm -f "${BUSY_FILE}.part"

: > "/tmp/fc-guard-lab/backups/pg_${NOW_STAMP}.sql"
filecleaner -v run -c /tmp/fc-guard-lab/guard-lab.toml --fail-on-freeze
echo "freeze_exit=$?"
```

Expected output:

```text
[lab-backups] FreezeBackupGuard: /tmp/fc-guard-lab/backups/pg_20261123_174428.sql (backup_guard freeze: ...)
[lab-backups] FreezeBackupGuard: /tmp/fc-guard-lab/backups/pg_20261120_174428.sql (backup_guard freeze: ...)
[lab-backups] FreezeBackupGuard: /tmp/fc-guard-lab/backups/pg_20261118_174428.sql (backup_guard freeze: ...)
Run: run-12745-1 | Mode: LIVE
Rule         Outcome  Evaluated   Acted   Busy  HookSkip  Floor       Freed  Mode
--------------------------------------------------------------------------------------
lab-backups  frozen           3       0      0         0      2         0 B  live
--------------------------------------------------------------------------------------
Totals: 3 evaluated, 0 acted, 0 B freed
Active freezes: lab-backups
  [frozen] lab-backups: backup_guard freeze: newest valid backup 'pg_20261120_174428.sql' ... (> require_newer_within 129600s)
freeze_exit=3
```

Because the `0`-byte file fails `min_valid_backup_bytes = "1MiB"`, the newest valid dump is `3` days old (`> 36h`),
triggering a full `frozen` state and protecting the remaining old backups from eviction.

---

## Step 5: Clean Up the Lab

Verify the background lock process is terminated and delete `/tmp/fc-guard-lab`:

```bash
rm -rf /tmp/fc-guard-lab
```

---

## Next Steps

- Continue to [Tutorial 3: PostgreSQL WAL and S3 Docker Lab](03-postgres-wal-s3-docker-lab.md)
- Remediate production freezes and constraints:
  [Remediate Backup Freeze or S3 Failure](../02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md)
- Review every `[rules.backup_guard]` and `[rules.in_use]` field:
  [Configuration Schema & Invariants](../03-reference/configuration/config-schema-and-invariants.md)
- Study the set-theoretic backup floor and freeze proofs:
  [Retention Calculus & TOCTOU Defenses](../04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)
