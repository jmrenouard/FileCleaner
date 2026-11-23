---
title: FileCleaner Documentation
description: Deterministic pure-Rust directory retention, backup safety guardrails, 6-layer in-use detection, S3 archival, and daemon engine
hide:
  - navigation
  - toc
---

<div class="filecleaner-hero">
  <img src="assets/header-banner.svg" alt="FileCleaner — Deterministic Pure-Rust Retention, Backup Guardrails & Lifecycle Engine" />
</div>

# FileCleaner — Deterministic Pure-Rust Retention, Backup Guardrails & Lifecycle Engine

FileCleaner (`filecleaner` v1.1.0) is a cross-platform directory retention, backup safety guardrail, active-file probe,
lifecycle transformation, and reactive daemon engine written in 100% pure Rust. Published documentation lives at
[https://filecleaner.lightpath.fr/](https://filecleaner.lightpath.fr/).

<div class="filecleaner-metrics">
  <div class="filecleaner-metric">
    <span class="filecleaner-metric-value">9 Targets</span>
    <span class="filecleaner-metric-label">Linux, macOS & Windows · Zero C -sys</span>
  </div>
  <div class="filecleaner-metric">
    <span class="filecleaner-metric-value">100% Pure Rust</span>
    <span class="filecleaner-metric-label">Edition 2021 · Pure-Rust Codecs & SigV4 S3</span>
  </div>
  <div class="filecleaner-metric">
    <span class="filecleaner-metric-value">94.01% Coverage</span>
    <span class="filecleaner-metric-label">135 Active Tests + 6/6 Live PG17/MinIO E2E</span>
  </div>
  <div class="filecleaner-metric">
    <span class="filecleaner-metric-value">6-Phase Fail-Closed</span>
    <span class="filecleaner-metric-label">InUseProbe & BackupGuard Always Win</span>
  </div>
</div>

<div class="filecleaner-actions" markdown>
[Start Tutorials](01-tutorials/index.md){ .md-button .md-button--primary }
[Browse How-To Guides](02-how-to-guides/index.md){ .md-button }
[Inspect Reference](03-reference/index.md){ .md-button }
[Read Explanation](04-explanation/index.md){ .md-button }
</div>

---

## Diátaxis Documentation Matrix (Action × Cognition)

FileCleaner organizes its documentation across the four Diátaxis quadrants so every page serves one clear purpose
without mixing tutorials, operational runbooks, reference tables, and architectural explanations:

| | **Practical (Action)** | **Theoretical (Cognition)** |
| :--- | :--- | :--- |
| **Acquisition (Study)** | [**1. Tutorials (`01-tutorials/`)**](01-tutorials/index.md)<br/>Guided isolated sandbox labs for learning FileCleaner step by step | [**4. Explanation (`04-explanation/`)**](04-explanation/index.md)<br/>Six-phase fail-closed pipeline architecture, retention math, and ADRs |
| **Application (Work)** | [**2. How-To Guides (`02-how-to-guides/`)**](02-how-to-guides/index.md)<br/>Goal-oriented ASD-STE100 Issue 9 deployment, S3, daemon, and security SOPs | [**3. Reference (`03-reference/`)**](03-reference/index.md)<br/>Exhaustive configuration schema, invariants, CLI flags, exit codes, and error matrices |

---

## Explore Documentation by Quadrant

<div class="grid cards" markdown>

-   :material-school-outline:{ .lg .middle } **1. Tutorials (`01-tutorials/`)**

    ---

    *Practical · Study / Acquisition* — Follow reproducible local sandboxes covering retention planning, `backup_guard`
    freezes, 6-layer `check-in-use` probes, and live PostgreSQL 17 + MinIO S3 archival.

    - [Tutorials Quadrant Index](01-tutorials/index.md)
    - [Tutorial 1: Local Retention Sandbox](01-tutorials/01-local-retention-sandbox.md)
    - [Tutorial 2: Backup Guard & In-Use Lab](01-tutorials/02-backup-guard-and-in-use-lab.md)
    - [Tutorial 3: PostgreSQL WAL & S3 Docker Lab](01-tutorials/03-postgres-wal-s3-docker-lab.md)

-   :material-wrench-outline:{ .lg .middle } **2. How-To Guides (`02-how-to-guides/`)**

    ---

    *Practical · Work / Application* — Execute ASD-STE100 Issue 9 procedures for Docker and `systemd` deployment, S3
    archival, reactive daemon scheduling, freeze remediation, and pre-delete security hooks.

    - [How-To Guides Quadrant Index](02-how-to-guides/index.md)
    - [Deploy with Docker and systemd](02-how-to-guides/deployment/deploy-filecleaner-docker-and-systemd.md)
    - [Backup Retention & S3 Archival](02-how-to-guides/operations/configure-backup-retention-and-s3-archival.md)
    - [Reactive Daemon & Log Rotation](02-how-to-guides/operations/run-reactive-daemon-and-rotate-logs.md)
    - [Backup Freeze or S3 Failure](02-how-to-guides/incident-response/remediate-backup-freeze-or-s3-failure.md)
    - [Pre-Delete Hooks & TOCTOU Guards](02-how-to-guides/security/enforce-pre-delete-hooks-and-toctou-guards.md)

-   :material-table-search:{ .lg .middle } **3. Reference (`03-reference/`)**

    ---

    *Theoretical · Work / Application* — Look up exact TOML/YAML configuration keys, `INV-CFG-01`..`INV-CFG-14`
    invariants, CLI subcommands, exit codes (`0`, `1`, `2`, `3`, `4`, `10`), and error matrices.

    - [Reference Quadrant Index](03-reference/index.md)
    - [Configuration Schema & Invariants](03-reference/configuration/config-schema-and-invariants.md)
    - [CLI Commands & Exit Codes](03-reference/cli-and-scripts/cli-commands-and-exit-codes.md)
    - [Error & Alert Matrix](03-reference/troubleshooting-codes/error-and-alert-matrix.md)

-   :material-lightbulb-outline:{ .lg .middle } **4. Explanation (`04-explanation/`)**

    ---

    *Theoretical · Study / Acquisition* — Understand the fail-closed six-phase execution pipeline, formal retention
    calculus, TOCTOU symlink-swap defenses, and Architecture Decision Records.

    - [Explanation Quadrant Index](04-explanation/index.md)
    - [Fail-Closed 6-Phase Pipeline](04-explanation/architecture-overview/fail-closed-six-phase-pipeline.md)
    - [ADR-001: Pure-Rust Fail-Closed Engine](04-explanation/design-decisions/adr-001-pure-rust-fail-closed-engine.md)
    - [ADR-002: Versioned MkDocs Publishing](04-explanation/design-decisions/adr-002-diataxis-versioned-mkdocs-publishing.md)
    - [Retention Calculus & TOCTOU Defenses](04-explanation/deep-dives/retention-calculus-and-toctou-defenses.md)

-   :material-translate:{ .lg .middle } **Bilingual EN / FR Guides**

    ---

    Access synchronized English and French beginner quickstarts, complete user manuals, production cookbooks, and
    formal engineering specifications.

    - [5-Minute HOWTO (EN)](en/HOWTO.md) · [HOWTO 5 Minutes (FR)](fr/HOWTO.md)
    - [User & Operations Guide (EN)](en/USER_GUIDE.md) · [Guide Utilisateur (FR)](fr/GUIDE_UTILISATEUR.md)
    - [Cookbook Examples (EN)](en/COOKBOOK_EXAMPLES.md) · [Exemples & Recettes (FR)](fr/EXEMPLES_RECETTES.md)
    - [Formal Specifications (EN)](en/SPECIFICATIONS.md) · [Spécifications Formelles (FR)](fr/SPECIFICATIONS.md)

-   :material-history:{ .lg .middle } **Releases & Governance**

    ---

    Track versioned releases, living engineering traceability logs, root-cause analyses, team alignment records, and the
    published site at [filecleaner.lightpath.fr](https://filecleaner.lightpath.fr/).

    - [Changelog (`CHANGELOG.md`)](../CHANGELOG.md)
    - [Release Notes `v1.0.1`](RELEASE_NOTES_v1.0.1.md)
    - [Action Logbook (`ACTION_LOGBOOK.md`)](ACTION_LOGBOOK.md)
    - [RCA Register (`RCA_REGISTER.md`)](RCA_REGISTER.md)
    - [Dev-Team Alignment Session](team-sessions/team-session-2026-11-20.md)

</div>
