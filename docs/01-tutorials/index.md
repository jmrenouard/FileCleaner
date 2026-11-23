# Tutorials

> [!NOTE]
> **Learning-Oriented Quadrant (`01-tutorials/`):** Follow these guided, self-contained sandbox labs to learn
> FileCleaner safely from a clean temporary directory before touching real filesystems.

Each tutorial uses isolated paths (`/tmp/fc-sandbox`, `/tmp/fc-guard-lab`, or ephemeral Docker containers), provides
exact commands and expected outputs, and cleans up all temporary artifacts at the end.

---

## Guided Learning Path

| Lesson | What You Will Practice | Key Commands & Subsystems |
| :--- | :--- | :--- |
| [1. Local Retention Sandbox](01-local-retention-sandbox.md) | Run `init`, `validate`, `plan`, and `run` inside `/tmp/fc-sandbox` with gzip compression and audit logging | `filecleaner init`, `validate`, `plan`, `run`, `audit.jsonl` |
| [2. Backup Guard and In-Use Lab](02-backup-guard-and-in-use-lab.md) | Trigger `check-in-use` exit `10`, protect `min_keep_files`, and trigger `--fail-on-freeze` exit `3` with a `0`-byte stub | `filecleaner check-in-use`, `backup_guard`, `in_use`, exit codes `0`, `3`, `10` |
| [3. PostgreSQL WAL and S3 Docker Lab](03-postgres-wal-s3-docker-lab.md) | Run the 6-scenario PostgreSQL 17 + MinIO Docker lab archiving real 16 MiB WAL segments and verifying SHA-256 restores | `init --profile dba-postgres`, `scripts/e2e-dba-s3-lab.sh`, `upload_s3` |

---

## Bilingual 5-Minute Quickstarts

For concise single-page quickstarts with copy-paste CLI one-liners and hook snippets:

- [5-Minute HOWTO (English)](../en/HOWTO.md)
- [HOWTO 5 Minutes (Français)](../fr/HOWTO.md)

---

## Next Steps

- Execute production deployment, archival, daemon, and security procedures:
  [How-To Guides](../02-how-to-guides/index.md)
- Look up exhaustive CLI flags, configuration invariants, and error matrices: [Reference](../03-reference/index.md)
- Study the fail-closed 6-phase pipeline, retention math, and ADRs: [Explanation](../04-explanation/index.md)
