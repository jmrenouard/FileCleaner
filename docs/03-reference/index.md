# Reference: Configuration, CLI, Scripts, and Error Matrix

Normative technical specifications for **FileCleaner** `v1.1.0` (`filecleaner` binary, Rust Edition 2021,
Apache-2.0).

## Core Reference Specifications

| Document | Scope | Primary Source Modules |
| :--- | :--- | :--- |
| [Configuration Schema & Validation Invariants](configuration/config-schema-and-invariants.md) | Top-level `Config`, `DaemonConfig`, `AuditConfig`, `RuleConfig`, `BackupGuardConfig`, `InUseConfig`, `HookConfig`, `S3Config`, enum variants, human byte/duration parsers, and invariants `INV-CFG-01`..`INV-CFG-14`. | `src/config.rs`, `src/scanner.rs`, `src/s3.rs` |
| [CLI Commands, Scripts, Output Schemas & Exit Codes](cli-and-scripts/cli-commands-and-exit-codes.md) | Global flags, `init`, `validate`, `plan`, `run` (`clean`), `check-in-use`, `daemon` subcommands, ad-hoc mode flags, exit codes `0`/`1`/`2`/`3`/`4`/`10`, repository scripts, JSONL audit log schema, and live daemon JSON state schema. | `src/cli.rs`, `src/engine.rs`, `src/audit.rs`, `src/daemon.rs`, `scripts/e2e-dba-s3-lab.sh`, `scripts/build_versioned_docs.py` |
| [Error, In-Use Probe & Alert Matrix](troubleshooting-codes/error-and-alert-matrix.md) | `FileCleanerError` variants, `InUseReason` probe triggers, `RuleOutcome` / `DecisionKind` statuses, S3 verification failures, and direct remediation runbook links. | `src/error.rs`, `src/in_use.rs`, `src/policy.rs`, `src/s3.rs`, `src/hooks.rs` |

## Complete Bilingual Technical Manuals

| Document | Language Edition | Focus |
| :--- | :--- | :--- |
| [Formal Technical Specifications (EN)](../en/SPECIFICATIONS.md) | English | Normative mathematical model, 6-phase pipeline contract, and protocol invariants |
| [Spécifications formelles (FR)](../fr/SPECIFICATIONS.md) | Français | Spécifications mathématiques normatives et contrat du pipeline |
| [User & Operations Guide (EN)](../en/USER_GUIDE.md) | English | Installation, operational defaults, service templates, and full CLI examples |
| [Guide utilisateur (FR)](../fr/GUIDE_UTILISATEUR.md) | Français | Installation, options d'exploitation et intégration système |

## Engineering Traceability & Release Records

| Record | Scope | Location |
| :--- | :--- | :--- |
| **Changelog** | Semantic version release history (`v1.0.0`, `v1.0.1`, `v1.1.0`) | [`CHANGELOG.md`](../../CHANGELOG.md) |
| **Release Notes v1.0.1** | Security hardening, TOCTOU fixes, and multi-arch Docker image notes | [`docs/RELEASE_NOTES_v1.0.1.md`](../RELEASE_NOTES_v1.0.1.md) |
| **Action Logbook** | Chronological engineering requests, changes, test counts, and `llvm-cov` metrics | [`docs/ACTION_LOGBOOK.md`](../ACTION_LOGBOOK.md) |
| **RCA Register** | Root cause analyses (`RCA-*`), dead-ends (`IMP-*`), approximations (`APP-*`), roadmap (`IDEA-*`) | [`docs/RCA_REGISTER.md`](../RCA_REGISTER.md) |

## Related Documentation Quadrants

| Quadrant | Purpose | Entrypoint |
| :--- | :--- | :--- |
| **Portal Home** | Main documentation hub and 2x2 Diátaxis overview | [Documentation Portal](../index.md) |
| **Tutorials** | Isolated sandbox labs for retention, backup guards, and PG17 + MinIO S3 archival | [Tutorials Index](../01-tutorials/index.md) |
| **How-To Guides** | Goal-oriented operational procedures, deployment SOPs, and incident runbooks | [How-To Guides Index](../02-how-to-guides/index.md) |
| **Explanation** | Fail-closed architecture, ADRs, and retention/TOCTOU mathematical deep-dives | [Explanation Index](../04-explanation/index.md) |
