# Tutorial 1: Local Retention Sandbox

> **Learning Goal:** Practice FileCleaner's safe `init` $\rightarrow$ `validate` $\rightarrow$ `plan` $\rightarrow$
> `run` workflow inside an isolated temporary directory (`/tmp/fc-sandbox`) with zero impact on system files.

> [!WARNING]
> **Isolated Local Sandbox Only:** Run every command in this tutorial inside `/tmp/fc-sandbox`. Never point tutorial
> commands or starter configurations at production backup or log directories.

---

## Prerequisites

Make sure your workstation meets these requirements before starting:

- `filecleaner` v1.1.0+ binary installed on your `PATH` (or built via `cargo build --bin filecleaner`)
- Standard Unix shell (`bash`) with GNU `touch` (`-d` flag) and `mktemp`
- Minimum `10 MiB` free temporary space under `/tmp`

Verify the binary is accessible:

```bash
filecleaner --version
```

Expected output:

```text
filecleaner 1.1.0
```

---

## What You Will Build

By the end of this tutorial, you will have:

1. Created an isolated `/tmp/fc-sandbox` directory tree containing synthetic dated `.log` files.
2. Previewed an ad-hoc cleanup plan using zero-config CLI flags (`filecleaner plan --dir ...`).
3. Generated and customized a `sandbox.toml` configuration starting in safe `dry_run = true` mode.
4. Validated the configuration (`filecleaner validate`), previewed exact decisions (`filecleaner plan`), and executed
   a live `rename_by_date -> compress -> move` pipeline.
5. Inspected the structured JSONL audit log and cleaned up all temporary sandbox directories.

---

## Step 1: Create the Isolated Sandbox Directory

Create three isolated subdirectories (`incoming`, `archive`, and `trash`) and populate `incoming` with four synthetic
logs carrying distinct modification timestamps:

```bash
rm -rf /tmp/fc-sandbox
mkdir -p /tmp/fc-sandbox/incoming /tmp/fc-sandbox/archive /tmp/fc-sandbox/trash

printf 'old log entry from 20 days ago\n' > /tmp/fc-sandbox/incoming/app-old-1.log
printf 'old log entry from 15 days ago\n' > /tmp/fc-sandbox/incoming/app-old-2.log
printf 'recent log entry from 2 days ago\n' > /tmp/fc-sandbox/incoming/app-recent.log
printf 'active log entry from today\n' > /tmp/fc-sandbox/incoming/app-current.log

touch -d "20 days ago" /tmp/fc-sandbox/incoming/app-old-1.log
touch -d "15 days ago" /tmp/fc-sandbox/incoming/app-old-2.log
touch -d "2 days ago" /tmp/fc-sandbox/incoming/app-recent.log
```

List the sandbox files to confirm their timestamps:

```bash
ls -l /tmp/fc-sandbox/incoming
```

---

## Step 2: Preview Ad-Hoc Retention with `filecleaner plan`

Run a read-only `plan` directly against `/tmp/fc-sandbox/incoming` to see which files exceed `--max-age 10d`:

```bash
filecleaner -v plan \
  --dir /tmp/fc-sandbox/incoming \
  --include "*.log" \
  --max-age 10d \
  --rename-by-date \
  --compress gzip \
  --move-to /tmp/fc-sandbox/archive
```

Expected output:

```text
[adhoc] KeepRetention: /tmp/fc-sandbox/incoming/app-current.log (retained (age 0s <= max_age 864000s))
[adhoc] KeepRetention: /tmp/fc-sandbox/incoming/app-recent.log (retained (age 172800s <= max_age 864000s))
[adhoc] WouldEvict: /tmp/fc-sandbox/incoming/app-old-2.log (combine=any triggered: age 1296000s > max_age 864000s)
[adhoc] WouldEvict: /tmp/fc-sandbox/incoming/app-old-1.log (combine=any triggered: age 1728000s > max_age 864000s)
Run: run-12488-1 | Mode: DRY-RUN
Rule   Outcome  Evaluated   Acted   Busy  HookSkip  Floor       Freed  Mode
--------------------------------------------------------------------------------
adhoc  ok               4       2      0         0      0         0 B  dry-run
--------------------------------------------------------------------------------
Totals: 4 evaluated, 2 would-act, 0 B freed
```

Notice that `plan` prints `Mode: DRY-RUN` and `0 B freed`, leaving all four files untouched inside
`/tmp/fc-sandbox/incoming`.

---

## Step 3: Generate and Customize a Starter Configuration

Generate a starter `logs` profile with `filecleaner init`:

```bash
filecleaner init \
  --profile logs \
  --format toml \
  --output /tmp/fc-sandbox/starter.toml
```

Expected output:

```text
initialized logs starter configuration at /tmp/fc-sandbox/starter.toml (dry_run = true)
```

Now write a sandbox-scoped configuration at `/tmp/fc-sandbox/sandbox.toml` starting with `dry_run = true`:

```toml
dry_run = true
global_trash_dir = "/tmp/fc-sandbox/trash"

[audit]
log_path = "/tmp/fc-sandbox/audit.jsonl"
jsonl = true

[[rules]]
name = "sandbox-logs"
path = "/tmp/fc-sandbox/incoming"
enabled = true
recursive = false
follow_symlinks = false
include_globs = ["*.log"]
exclude_globs = ["*.keep"]
time_basis = "modified"
max_files = 2
max_age = "10d"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["rename_by_date", "compress", "move"]
rename_template = "%Y%m%d_%H%M%S"
compression = "gzip"
compress_remove_source = true
compress_overwrite = false
move_to = "/tmp/fc-sandbox/archive"
move_date_subdir = "%Y/%m"
move_overwrite = false
```

Validate the configuration file against all schema and safety invariants (`INV-CFG-01`..`INV-CFG-14`):

```bash
filecleaner validate --config /tmp/fc-sandbox/sandbox.toml
```

Expected output:

```text
configuration valid: 1 rule(s) loaded from /tmp/fc-sandbox/sandbox.toml
```

---

## Step 4: Preview `plan`, Enable Live Mode, and Run

Run `filecleaner plan` with `--json` to inspect the projected `SweepPlan` before allowing mutations:

```bash
filecleaner plan --config /tmp/fc-sandbox/sandbox.toml --json
```

Flip `dry_run = true` to `dry_run = false` in `/tmp/fc-sandbox/sandbox.toml`, then run the live sweep:

```bash
sed -i 's/^dry_run = true/dry_run = false/' /tmp/fc-sandbox/sandbox.toml
filecleaner run --config /tmp/fc-sandbox/sandbox.toml
```

Expected output:

```text
Run: run-12537-1 | Mode: LIVE
Rule          Outcome  Evaluated   Acted   Busy  HookSkip  Floor       Freed  Mode
---------------------------------------------------------------------------------------
sandbox-logs  ok               4       2      0         0      0        62 B  live
---------------------------------------------------------------------------------------
Totals: 4 evaluated, 2 acted, 62 B freed
```

Verify the transformed `.gz` archives under `/tmp/fc-sandbox/archive` and inspect the JSONL audit trail:

```bash
find /tmp/fc-sandbox/incoming /tmp/fc-sandbox/archive -type f | sort
cat /tmp/fc-sandbox/audit.jsonl
```

Notice that `app-old-1.log` and `app-old-2.log` were renamed with their modification date, compressed to `.gz`,
moved into dated `YYYY/MM` subfolders under `/tmp/fc-sandbox/archive`, and recorded in `/tmp/fc-sandbox/audit.jsonl`.

---

## Step 5: Clean Up the Sandbox

Remove the temporary `/tmp/fc-sandbox` directory cleanly:

```bash
rm -rf /tmp/fc-sandbox
```

---

## Next Steps

- Continue to [Tutorial 2: Backup Guard and In-Use Lab](02-backup-guard-and-in-use-lab.md)
- Execute production operational procedures: [How-To Guides](../02-how-to-guides/index.md)
- Look up all CLI flags and exit codes:
  [CLI Commands & Exit Codes](../03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
- Understand how the six phases run in order:
  [Fail-Closed 6-Phase Pipeline](../04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
