# Dev-Team Session — FileCleaner v1.0.0 Architecture & Scope Alignment

**Date**: 2026-11-20  
**Topic**: FileCleaner v1.0.0 Architecture, Safety Invariants, Cross-Platform Probes & Scope Alignment  
**Participants**: Product Manager (PM), Principal Architect (Architect), Senior Rust Developer (Developer), Quality Assurance Lead (QA), Swarm Coordinator  
**Normative Downstream Artifacts**: [`docs/en/SPECIFICATIONS.md`](../en/SPECIFICATIONS.md) · [`docs/fr/SPECIFICATIONS.md`](../fr/SPECIFICATIONS.md)

---

## 1. Purpose of Session

Align the multi-disciplinary engineering team before implementation waves begin on **FileCleaner** (`filecleaner` v1.0.0, pure-Rust retention, backup safety guardrail, in-use detection, lifecycle action pipeline, and daemon engine). Because FileCleaner executes destructive and transformative filesystem operations (`Delete`, `Trash`, `Move`, `CopyTruncate`, `Compress`, `RenameByDate`), every ambiguity in rule precedence, cross-platform lock semantics, dry-run parity, or hook failure handling constitutes a direct data-loss risk.

---

## 2. Persona Assessments

### 2.1 Product Manager (PM)

- **First Reaction**: Ambitious v1.0.0 scope bundling retention policies, backup dead-man's-switches, 5+ layer in-use file detection, logrotate-style transforms, external hook scripts, and a cross-platform daemon; user trust, dry-run predictability, and clear precedence rules matter most.
- **Key Concerns**:
  1. **Scope Density**: Delivering all subsystems with bilingual EN/FR documentation and $\ge 90\%$ line coverage simultaneously risks delivery drag without modularization.
  2. **Precedence & Auditability**: Users need deterministic, easy-to-explain outcomes when `max_files`, `max_age`, `max_total_size`, `min_keep_files`, and staleness gates interact, backed by strict dry-run guarantees and append-only audit logs.
  3. **Cross-Platform Lock Divergence**: Ensuring consistent user expectations across Linux, macOS, and Windows despite differing OS file-locking semantics.
- **First Action**: Define explicit user personas, rule-evaluation precedence, dry-run parity guarantees, and stable CLI exit-code contracts.
- **Open Question Raised**: *What could move to v1.1 if cutting roughly a third of initial scope?*

### 2.2 Principal Architect (Architect)

- **First Reaction**: Destructive data-lifecycle engine where a single precedence flaw or TOCTOU race condition can wipe active files or sole remaining backups; demands strict fail-closed architectural boundaries from day one.
- **Key Concerns**:
  1. **Deterministic Pipeline & `CopyTruncate` Race**: The `Plan -> Execute` pipeline must produce identical decisions in dry-run (`plan` / `--dry-run`) and live execution; `CopyTruncate` carries an inherent write-vs-truncate tail window.
  2. **Cross-OS Divergence & `EXDEV`**: Windows exclusive share-mode locks vs Unix `flock` vs permission-gated Linux `/proc/*/fd`; non-atomic cross-filesystem moves (`EXDEV`); differing daemon service managers (`systemd`, `launchd`, Windows Task Scheduler).
  3. **Testability**: Injectable traits (`Fs`, `Clock`, `LockProbe` / `InUseProbe`, `HookRunner`) are mandatory to achieve $\ge 90\%$ deterministic line coverage.
- **First Action**: Lock the `Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute` pipeline order, symlink non-traversal defaults, and explicit error-handling policies.
- **Open Questions Raised**: *When `/proc` returns `EACCES`, should FileCleaner fail-closed and skip? Should `CopyTruncate` require explicit configuration opt-in?*

### 2.3 Senior Rust Developer (Developer)

- **First Reaction**: Retention selection math is straightforward in Rust; cross-platform lock detection, race-free transforms (`CopyTruncate`, streaming compression, cross-FS SHA-256 verified moves), and backup safety gates carry the real engineering complexity.
- **Key Concerns**:
  1. **OS Probe & Growth-Window Mechanics**: Reconciling Windows `share_mode(0)`, POSIX `flock(LOCK_EX | LOCK_NB)`, Linux `/proc/*/fd` inode matching, `CopyTruncate` EOF-to-truncate windows, and batching the two-point stability window to avoid sequential sleep latency.
  2. **Idempotent Execution & `EXDEV` Safety**: Deterministic ordering, crash-safe `.fc-tmp` staging, symlink non-traversal, and fail-closed error handling.
  3. **Pure-Rust Portability**: Zero C `-sys` crates (`flate2` with `miniz_oxide`, `lz4_flex`, `brotli`, `zip` with pure-Rust `deflate`) to support 9-target cross-compilation cleanly, plus trait-abstracted OS calls.
- **First Action**: Implement the `discover -> lock-probe -> safety-gate -> transform/prune` state machine, lock pure-Rust compression codecs, and ship `--dry-run` / `plan` parity prior to the daemon loop.
- **Open Question Raised**: *Do transformed outputs (renamed or compressed files) remaining in the same monitored directory count toward `min_keep_files` and `require_newer_within`?*

### 2.4 Quality Assurance Lead (QA)

- **First Reaction**: Destructive retention combined with cross-platform lock probes, `CopyTruncate`, and external veto hooks creates high failure-surface risk; achieving $\ge 90\%$ coverage across Linux, macOS, Windows, and ARM targets requires deterministic fixtures.
- **Key Concerns**:
  1. **Lock-Detection Gaps & TOCTOU Races**: `/proc/*/fd` is Linux-only and privilege-scoped; Windows and POSIX enforce locking differently; files can transition between idle and active between scan and unlink.
  2. **Edge-Case Traps**: Trailing-byte loss during `CopyTruncate`, symlinks, hardlinks, sparse files, zero-byte or truncated backup stubs satisfying naive file-count floors, and hung pre-delete veto scripts.
  3. **Cross-OS Verification Parity**: Deterministic `mtime`/permission assertions, failure-injection tests (checksum mismatch, simulated `EXDEV`, hook timeout), and strict `--dry-run` vs live parity.
- **First Action**: Build a destructive-safety test matrix covering dry-run/live parity, active-writer fixtures per probe strategy, staleness and zero-byte backup traps, and hook timeout termination.
- **Open Question Raised**: *When constraints conflict (e.g., `max_total_size` or `max_files` remains exceeded because `min_keep_files` or in-use locks block eviction), what are the exact rule outcome, exit code, and daemon alert contract?*

---

## 3. Coordinator Synthesis & Architectural Resolutions

The following resolutions are **normative** and directly govern [`docs/en/SPECIFICATIONS.md`](../en/SPECIFICATIONS.md), [`docs/fr/SPECIFICATIONS.md`](../fr/SPECIFICATIONS.md), and all v1.0.0 implementation and verification waves.

### Resolution 1 — Lock Divergence, `/proc` `EACCES`, and `CopyTruncate` Opt-In (4/4 Consensus)

- **Fail-Closed Active Probes**: Any active-writer trigger across sidecars, Unix `flock(LOCK_EX | LOCK_NB)`, Linux `/proc/*/fd`, Windows `share_mode(0)`, `min_idle_age`, or the two-point `(size, mtime)` stability window unconditionally places the file in $S_{\text{busy}}$, guaranteeing $E \cap S_{\text{busy}} = \emptyset$.
- **Linux `/proc/*/fd` Privilege Boundary**: When non-owned PID directories under `/proc/<pid>/fd` return `EACCES` or `EPERM`, FileCleaner logs a diagnostic telemetry note (and does **not** treat unprivileged `EACCES` on unrelated system processes as a blanket lock on every file, because `flock`, `min_idle_age`, sidecars, and stability window remain active); however, any direct probe failure on the *target candidate itself* (e.g., unreadable/un-stat-able candidate or failing `flock` with `EWOULDBLOCK`/`EACCES`) fails closed and places the file in $S_{\text{busy}}$.
- **Injectable Test Traits**: `Clock`, `LockProbe` / `InUseProbe`, `FsOps`, and `HookRunner` MUST be abstracted behind traits so unit tests deterministically simulate time progression, cross-FS `EXDEV`, lock contention, and hook timeouts.
- **Explicit `CopyTruncate` Opt-In**: `ActionStep::CopyTruncate` MUST be explicitly opted into by the user in `RuleConfig.actions` (`init` templates never enable it by default). Specifications and `validate` warnings MUST explicitly document the inherent copy-to-truncate tail-byte race window for active append writers.

### Resolution 2 — Strict Pipeline Precedence, Dry-Run Parity & `Constrained` Conflict Contract (4/4 Consensus)

- **Mandatory Pipeline Order**: Every rule sweep MUST execute phases in strict order:
  1. `Scan` (discovery, symlink non-traversal default, glob/regex include-exclude, timestamp resolution & deterministic ordering)
  2. `InUseProbe` (partitioning $S_{\text{busy}}$)
  3. `BackupGuard` (evaluating $\mathcal{F}_{\text{valid}} = \{f \in \mathcal{F} \mid \text{size}(f) \ge B_{\min}\}$, staleness freeze, and protected floor $P_{\text{floor}}$)
  4. `RetentionEval` (computing eviction set $E \subseteq \mathcal{F} \setminus (P_{\text{floor}} \cup S_{\text{busy}})$ and deterministic `SweepPlan`)
  5. `PreHook` (synchronous external check per candidate in $E$, skipped during dry-run unless `run_in_dry_run = true`)
  6. `Execute` (ordered `ActionStep` transformations/deletions, or zero-mutation `WouldAct` emission in `plan` / `--dry-run`)
- **Strict Dry-Run / Live Parity**: `filecleaner plan` and `filecleaner run --dry-run` execute phases 1–4 identically to live `filecleaner run`, yielding an identical `SweepPlan` without any filesystem mutation.
- **Conflict Resolution (`RuleOutcome::Constrained`)**: When `max_total_size` or `max_files` remains exceeded after selecting all eligible evictions because `BackupGuard` ($P_{\text{floor}}$) and/or `InUseProbe` ($S_{\text{busy}}$) prevented further deletions:
  - Safety invariants (`BackupGuard` and `InUseProbe`) **ALWAYS win**.
  - The rule finishes with `RuleOutcome::Constrained`.
  - Excess diagnostics (`remaining_files`, `target_max_files`, `remaining_bytes`, `target_max_total_size`, `protected_floor_count`, `busy_skipped_count`) are recorded in the JSONL audit log, the daemon live JSON state file (`constrained_rules`), and `plan`/`run` output.
  - Default process exit code remains `0` so non-fatal safety protections do not trigger false cron failures; passing `--strict-constraints` elevates `Constrained` to exit code `3` (matching strict safety gate enforcement alongside `--fail-on-freeze`).
- **Transformed Outputs & Backup Counting**: Files produced by in-place transforms (`RenameByDate`, `Compress`) that remain inside the monitored `rule.path`, continue to match the rule's include/exclude filters, and have $\text{size}(f) \ge \text{min\_valid\_backup\_bytes}$ **continue to count** toward `min_keep_files` and staleness (`t_max`). Files relocated outside `rule.path` via `Move` or `Trash` leave $\mathcal{F}$ and no longer count toward that rule's monitored floor.

### Resolution 3 — Pure-Rust Portability & $\ge 90\%$ Line Coverage (3/4 Consensus)

- **Zero C `-sys` Crates**: Compression and hashing MUST rely exclusively on pure-Rust crates (`flate2` default `miniz_oxide`, `lz4_flex`, `brotli`, `zip` with `default-features = false` + `deflate`, `sha2`) to guarantee seamless 9-target cross-compilation including `musl` and ARM64.
- **CI Coverage Gate**: `cargo llvm-cov --fail-under-lines 90` enforced across the workspace.

### Resolution 4 — Full v1.0.0 Scope Retained via Parallel Swarm Execution

- **PM Cutline Proposal vs User Mandate**: Although the PM identified `daemon` mode and secondary codecs (`lz4`, `brotli`, `zip`) as candidates for deferral to v1.1, the Coordinator resolved to **retain the full v1.0.0 scope** (`RenameByDate`, `Compress` across all 4 codecs, `Move` with SHA-256 verification, `CopyTruncate`, `Trash`, `Delete`, `PreHook`, and `daemon` with `interval`/`notify`/`hybrid` modes and OS service templates) by modularizing implementation across parallel swarm workers followed by a dedicated verification pass.

---

## 4. Actionable Traceability Matrix

| Resolution ID | Owner Module(s) | Specification Section (EN & FR) |
| :--- | :--- | :--- |
| **RES-01** Fail-closed probes & `/proc` `EACCES` | `src/in_use.rs`, `src/traits.rs` | Section 5 (Multi-Probe In-Use Matrix) & Section 9.4 |
| **RES-02** Strict 6-phase pipeline & `DryRun` parity | `src/planner.rs`, `src/engine.rs` | Section 1.2, Section 4, Section 7.3 |
| **RES-03** `RuleOutcome::Constrained` & exit codes | `src/planner.rs`, `src/cli.rs`, `src/audit.rs`, `src/daemon.rs` | Section 4.6, Section 8.3, Section 9.2, Section 9.3 |
| **RES-04** Transformed outputs in monitored directory | `src/scanner.rs`, `src/retention.rs`, `src/actions.rs` | Section 4.2, Section 7.1, Section 7.2 |
| **RES-05** Explicit `CopyTruncate` opt-in & race bounds | `src/config.rs`, `src/actions.rs` | Section 2.5, Section 7.2.4 |
| **RES-06** Pure-Rust stack & $\ge 90\%$ `llvm-cov` | `Cargo.toml`, `tests/` | Section 1.4, Section 7.2.2 |
