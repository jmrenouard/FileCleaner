# Action Logbook — FileCleaner Project

> **Governance Rule ([`AGENTS.md`](../AGENTS.md) §2 & [`.agents/rules/project-governance.md`](../.agents/rules/project-governance.md))**:
> This Action Logbook **MUST be systematically updated on every request, every change, and every result** (strictly in English).
> - Every new request is logged under **"1. Ongoing Actions (`IN PROGRESS`)"** as soon as its GitHub Issue is opened.
> - Once validated by Rust formatting, Clippy, and unit/integration tests (`cargo fmt`, `cargo clippy`, `cargo test`) and its Pull Request is created/merged, the entry is completed with the exact **Changes** and quantified **Result**, then moved under **"2. Completed Actions (`COMPLETED`)"**.

---

## 1. Ongoing Actions (`IN PROGRESS`)

| Action ID | Date / Time (ISO) | Issue / Branch | Status | Request, Ongoing Changes & Expected Results |
| :--- | :--- | :--- | :--- | :--- |
| *(None)* | — | — | — | All actions completed. |

---

## 2. Completed Actions (`COMPLETED`)

### ACT-002 — Implement Directory Scanner, Multi-Probe In-Use Detector, and Backup-Guarded Retention Engine (`Issue #7`)
- **Timestamp**: `2026-11-20T09:57:09Z`
- **Issue / Branch**: `#7` (`feat/issue-7-retention-safety-inuse`)
- **Request**: Implement the Wave 2 core scanning, 6-layer active-writer detection, and mathematical backup-guarded retention planner (`src/scanner.rs`, `src/in_use.rs`, `src/policy.rs`) with full unit and integration test suites (`tests/retention_age_size_count_test.rs`, `tests/backup_guards_test.rs`, `tests/in_use_detection_test.rs`).
- **Changes**:
  - [`.gitignore`](../.gitignore): Added `/.worktrees/` exclusion so isolated parallel agent worktrees stay untracked.
  - [`src/scanner.rs`](../src/scanner.rs): Added `Clock` (`SystemClock`, `FixedClock`), `ScannedFile`, `ScanReport`, and `Scanner::scan_rule` enforcing non-symlink default traversal, `.fc-tmp`/control-file exclusion, glob/regex include/exclude precedence, byte-range bounds, fail-closed `Created`/`FilenameDate` timestamp resolution, and deterministic newest-first ordering.
  - [`src/in_use.rs`](../src/in_use.rs): Added `InUseReason`, `InUseReport`, `InUseBatchResult`, injectable `LockProbe` + `OsLockProbe` (Unix `flock`, Linux `/proc/*/fd` inode scan with fail-closed `EACCES` diagnostics, Windows `share_mode(0)`), `ProcFdProbeOutcome`, `InUseDetector::check_batch` (with batched single-sleep two-point `stability_window` & future-mtime fail-closed `min_idle_age`), and `check_single` / `check_single_with`.
  - [`src/policy.rs`](../src/policy.rs): Added `RuleOutcome` (`Ok`, `Frozen`, `Constrained`), `DecisionKind`, `CandidateDecision`, `SweepPlan`, and `RetentionEngine::plan` implementing `F_valid`, staleness dead-man's-switch freezes, unconditional `min_keep_files` floor, busy-file exclusion, `CombineMode::{Any, All}`, and `EvictionOrder::{OldestFirst, LargestFirst}`.
  - [`src/lib.rs`](../src/lib.rs): Exported `in_use`, `policy`, and `scanner` modules with public re-exports.
  - Integration Tests: [`tests/retention_age_size_count_test.rs`](../tests/retention_age_size_count_test.rs), [`tests/backup_guards_test.rs`](../tests/backup_guards_test.rs), and [`tests/in_use_detection_test.rs`](../tests/in_use_detection_test.rs).
  - Governance Registers: Updated [`docs/ACTION_LOGBOOK.md`](ACTION_LOGBOOK.md) and [`docs/RCA_REGISTER.md`](RCA_REGISTER.md) (`IMP-003`).
- **Result**:
  - `cargo fmt --all -- --check`: PASS (`0` diffs).
  - `cargo clippy --all-targets -- -D warnings`: PASS (`0` warnings).
  - `cargo test`: PASS (`32` unit tests + `16` integration tests = `48 passed; 0 failed`).
  - `cargo llvm-cov --all-targets --workspace --summary-only --fail-under-lines 90`: PASS (`92.00%` line coverage across `3,112` lines: `config.rs` `94.61%`, `error.rs` `100.00%`, `in_use.rs` `87.23%`, `main.rs` `100.00%`, `policy.rs` `92.38%`, `scanner.rs` `90.79%`).
  - Independent Audit: Approved by `@verifier`.

### ACT-001 — Establish Governance, Bilingual Specifications, Brand Identity, and Cargo v1.0.0 Contract (`Issue #1`)
- **Timestamp**: `2026-11-20T09:26:41Z`
- **Issue / Branch**: `#1` (`docs/issue-1-governance-specs-branding`)
- **Request**: Initialize the `jmrenouard/FileCleaner` repository with Apache-2.0 license, `.gitignore`/`.gitattributes`, SSH remote, MicroLLM-derived governance (`AGENTS.md`, `.agents/rules/*`, `.github/dependabot.yml`, `docs/ACTION_LOGBOOK.md`, `docs/RCA_REGISTER.md`), official brand identity assets (`docs/assets/logo.svg`, `docs/assets/logo.png`, `docs/assets/header-banner.svg`), formal bilingual specifications (`docs/en/SPECIFICATIONS.md`, `docs/fr/SPECIFICATIONS.md`), dev-team session transcript (`docs/team-sessions/team-session-2026-11-20.md`), and the initial compiling Rust `v1.0.0` Cargo scaffold (`Cargo.toml`, `src/error.rs`, `src/config.rs`, `src/lib.rs`, `src/main.rs`, `tests/cli_validate.rs`).
- **Changes**:
  - **Repository Root & GitHub Foundation** (root commit `efac08c` on `main`): `.gitignore`, `.gitattributes`, `LICENSE` (Apache-2.0, Copyright 2026 Jean-Marie Renouard / LightPath), public repo `jmrenouard/FileCleaner` over SSH (`git@github.com:jmrenouard/FileCleaner.git`).
  - **Governance & Automation (English-Only)**: [`AGENTS.md`](../AGENTS.md), [`.agents/rules/github-workflow.md`](../.agents/rules/github-workflow.md), [`.agents/rules/git-workflow.md`](../.agents/rules/git-workflow.md), [`.agents/rules/project-governance.md`](../.agents/rules/project-governance.md), [`.agents/rules/language-policy.md`](../.agents/rules/language-policy.md), [`.agents/rules/latex-math-in-markdown.md`](../.agents/rules/latex-math-in-markdown.md), [`.github/dependabot.yml`](../.github/dependabot.yml), [`docs/ACTION_LOGBOOK.md`](ACTION_LOGBOOK.md), [`docs/RCA_REGISTER.md`](RCA_REGISTER.md) (`IMP-001`, `IMP-002`, `APP-001`, `IDEA-001`..`IDEA-004`).
  - **Brand Identity Assets**: [`docs/assets/logo.svg`](assets/logo.svg), [`docs/assets/logo.png`](assets/logo.png) (`512x512`), [`docs/assets/header-banner.svg`](assets/header-banner.svg) (`1260x420`).
  - **Bilingual Formal Specifications & Team Alignment**: [`docs/en/SPECIFICATIONS.md`](en/SPECIFICATIONS.md), [`docs/fr/SPECIFICATIONS.md`](fr/SPECIFICATIONS.md) (`62/62` matched headings, `22/22` column-0 display math equations, zero `\operatorname`), [`docs/team-sessions/team-session-2026-11-20.md`](team-sessions/team-session-2026-11-20.md).
  - **Rust v1.0.0 Cargo Scaffold & Tests**: [`Cargo.toml`](../Cargo.toml), [`Cargo.lock`](../Cargo.lock), [`src/error.rs`](../src/error.rs), [`src/config.rs`](../src/config.rs), [`src/lib.rs`](../src/lib.rs), [`src/main.rs`](../src/main.rs), [`tests/cli_validate.rs`](../tests/cli_validate.rs) (pure-Rust dependencies verified via `cargo tree`; aligned schema covering `Config`, `DaemonConfig`/`DaemonMode`, `AuditConfig`, `RuleConfig`, `TimeBasis`, `CombineMode`, `EvictionOrder`, `BackupGuardConfig`, `InUseConfig`, `HookConfig`/`HookFailurePolicy`, `ActionStep`, `CompressionFormat`, `parse_bytes`, `parse_duration_str`, `Config::from_file`, and `Config::validate`).
- **Result**:
  - `cargo fmt --all -- --check`: PASS (`0` formatting diffs).
  - `cargo clippy --all-targets -- -D warnings`: PASS (`0` warnings, `0` errors).
  - `cargo test`: PASS (`13` unit tests in `src/lib.rs` + `1` integration test in `tests/cli_validate.rs` = `14 passed; 0 failed`).
  - `cargo llvm-cov --summary-only --fail-under-lines 90`: PASS (`94.80%` line coverage across `1,269` lines: `src/config.rs` `94.61%`, `src/error.rs` `100.00%`, `src/main.rs` `100.00%`).
  - Spec validation: All canonical TOML/YAML examples in `docs/en/SPECIFICATIONS.md` §2.6 verified with `filecleaner validate` (exit `0`).
  - GitHub tracking: Issue `#1` / PR `#2` (`docs/issue-1-governance-specs-branding`).
