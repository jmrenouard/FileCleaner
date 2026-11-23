# How-To Guides and Operational Runbooks

This section provides goal-oriented operational procedures and incident runbooks for **FileCleaner** v1.1.0 written in
**ASD-STE100 Issue 9** Simplified Technical English.

## Deployment

| Guide | Purpose |
| :--- | :--- |
| [Deploy FileCleaner with Docker and systemd](deployment/deploy-filecleaner-docker-and-systemd.md) | Run the multi-arch container image or install a hardened Linux `systemd` service unit. |

## Operations

| Guide | Purpose |
| :--- | :--- |
| [Configure Backup Retention and S3 Archival](operations/configure-backup-retention-and-s3-archival.md) | Set up `backup_guard` safety floors, environment S3 credentials, and SHA-256 archival sweeps. |
| [Run Reactive Daemon and Rotate Logs](operations/run-reactive-daemon-and-rotate-logs.md) | Configure `hybrid` watcher scheduling, `copy_truncate` or `rename_by_date` log rotation, and `audit.jsonl`. |

## Incident Response

| Guide | Purpose |
| :--- | :--- |
| [Remediate Backup Freeze or S3 Failure](incident-response/remediate-backup-freeze-or-s3-failure.md) | Diagnose and resolve `backup_guard` staleness freezes (`exit 3`) and S3 verification errors (`exit 1`). |

## Security

| Guide | Purpose |
| :--- | :--- |
| [Enforce Pre-Delete Hooks and TOCTOU Guards](security/enforce-pre-delete-hooks-and-toctou-guards.md) | Configure `pre_delete_hook` exit-`42` vetoes, 6-layer `in_use` checks, and `follow_symlinks = false`. |

## Production Cookbooks

Complete multi-rule recipes with sample TOML/YAML files and expected audit outputs are available in both languages:

- [Cookbook Examples (English)](../en/COOKBOOK_EXAMPLES.md)
- [Exemples & Recettes (Français)](../fr/EXEMPLES_RECETTES.md)

## Other Documentation Quadrants

- [Documentation Portal](../index.md)
- [Tutorials](../01-tutorials/index.md)
- [Reference Specifications](../03-reference/index.md)
- [Architecture Explanation](../04-explanation/index.md)
