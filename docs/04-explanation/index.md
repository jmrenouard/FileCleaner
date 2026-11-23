# Architecture & Design Explanations

This quadrant explains how FileCleaner enforces deterministic fail-closed safety guarantees, why its core pure-Rust
and Diátaxis architectural decisions were adopted, and how its retention calculus and TOCTOU defenses operate across
filesystems and object storage.

## Contents

| Document | Focus | Primary Topics |
| :--- | :--- | :--- |
| [Fail-Closed Six-Phase Pipeline](architecture-overview/fail-closed-six-phase-pipeline.md) | System Architecture | Strict phase isolation (`Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`), injectable traits, daemon event loop, and crash-safe staging |
| [ADR-001: Pure-Rust Fail-Closed Engine](design-decisions/adr-001-pure-rust-fail-closed-engine.md) | Engine Design Decision | Why FileCleaner mandates zero C/C++ `-sys` crates across 9 targets and prioritizes safety floors over retention caps |
| [ADR-002: Diátaxis and Versioned MkDocs Publishing](design-decisions/adr-002-diataxis-versioned-mkdocs-publishing.md) | Documentation Decision | Why FileCleaner separates operational docs into 4 Diátaxis quadrants and builds multi-version static bundles for IONOS Apache |
| [Retention Calculus and TOCTOU Defenses](deep-dives/retention-calculus-and-toctou-defenses.md) | Mathematical & Systems Deep Dive | Filter algebra, staleness freezes, quota conservation under in-place transforms, handle pinning (`O_NOFOLLOW`), and S3 SHA-256 proofs |

## Foundational Engineering Context

- **Formal Specifications (EN / FR)**: Read [Formal Technical Specifications](../en/SPECIFICATIONS.md) and
  [Spécifications formelles](../fr/SPECIFICATIONS.md) for the complete normative contract.
- **Dev-Team Alignment Record**: Read [Dev-Team Alignment Session](../team-sessions/team-session-2026-11-20.md)
  for the initial architecture alignment synthesis.
- **Root Cause & Trade-Off Traceability**: Read [RCA Register](../RCA_REGISTER.md) and
  [Action Logbook](../ACTION_LOGBOOK.md) for recorded dead-ends (`IMP-*`), platform approximations (`APP-*`), and
  postmortem fixes (`RCA-*`).

## Sibling Quadrants

- **Portal Home**: [Documentation Portal](../index.md)
- **Tutorials**: [Guided Sandbox Labs](../01-tutorials/index.md)
- **How-To Guides**: [Operational SOPs and Incident Runbooks](../02-how-to-guides/index.md)
- **Reference**: [Configuration, CLI & Error Specifications](../03-reference/index.md)
