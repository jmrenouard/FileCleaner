# RCA Register — Errors, Technical Dead-Ends, Approximations & Unimplemented Ideas

> **Governance Rule ([`AGENTS.md`](../AGENTS.md) §3 & [`.agents/rules/project-governance.md`](../.agents/rules/project-governance.md))**:
> This register guarantees full engineering traceability for the **FileCleaner** project (strictly in English). It **MUST be systematically updated on every request, every change, and every result** across its four sections:
> 1. **Errors & Bugs Encountered (Root Cause Analysis — `RCA-XXX`)**
> 2. **Technical Dead-Ends & Abandoned Leads (`IMP-XXX`)**
> 3. **Current Approximations & Simplifying Assumptions (`APP-XXX`)**
> 4. **Unimplemented Ideas & Future Roadmap (`IDEA-XXX`)**

---

## 1. Errors & Bugs Encountered (Root Cause Analysis — `RCA`)

*(No runtime or CI regressions encountered yet in Wave 1 bootstrap.)*

---

## 2. Technical Dead-Ends & Abandoned Leads (`IMP`)

### IMP-001 — C/C++ `-sys` Compression Bindings (`zstd-sys`, `liblz4-sys`, `bzip2-sys`, `zlib-ng-sys`) vs. Pure-Rust Compression Crates
- **Context**: Selecting compression libraries in [`Cargo.toml`](../Cargo.toml) to support `gzip`, `lz4`, `brotli`, and `zip` archive transforms across a 9-target cross-compilation release matrix (Linux `x86_64`/`aarch64` `gnu` & `musl`, macOS `x86_64`/`aarch64`, Windows `x86_64`/`aarch64` MSVC & GNU).
- **Why It Is a Dead-End**:
  - Crates wrapping C/C++ libraries (`*-sys`) depend on external `cc` cross-compilers, target sysroots, `cmake`, or `musl-gcc` / `zig` toolchains during `cross` and GitHub Actions matrix builds. Cross-compiling `zstd-sys` or `liblz4-sys` to `aarch64-unknown-linux-musl` or Windows ARM64 frequently fails on mismatched headers, linkers, or ABI flags.
- **Adopted Decision**:
  - Mandated **pure-Rust compression backends only**: `flate2` with default `miniz_oxide` (pure Rust deflate/gzip), `lz4_flex` (pure Rust LZ4 frame/block), `brotli` (pure Rust Brotli), and `zip` with `default-features = false, features = ["deflate"]` (using `flate2`/`miniz_oxide` without `zstd-sys` or `bzip2-sys`). Zero C compiler toolchains are required for any target.

---

### IMP-002 — Pushing `.github/workflows/*.yml` Over HTTPS `gho_` OAuth Credentials
- **Context**: Repository remote configuration in GitHub initialization (`ACT-001` / Issue `#1`) and upcoming CI/CD release workflows.
- **Why It Is a Dead-End**:
  - GitHub CLI (`gh auth login`) defaults to HTTPS credential helpers using OAuth tokens (`gho_*`) with `'gist', 'read:org', 'repo', 'user'` scopes, which lack the `'workflow'` scope. Any `git push` touching `.github/workflows/*.yml` is rejected by GitHub with `refusing to allow an OAuth App to create or update workflow ... without workflow scope`.
- **Adopted Decision**:
  - Configured the `origin` remote strictly over SSH (`git remote set-url origin git@github.com:jmrenouard/FileCleaner.git`) immediately after repository creation, using authenticated SSH keys that possess full repository and workflow push permissions.

---

### IMP-003 — Swarm `workspace:"share"` VCS Unknown Status vs. Explicit `git worktree` `.worktrees/<role>` Isolation
- **Context**: Coordinating parallel Wave 2 development across core retention (`Issue #7`), actions/hooks (`Issue #8`), and bilingual documentation (`docs/bilingual-guides-draft`) inside a single Dropbox-synced Git repository on Windows.
- **Why It Is a Dead-End**:
  - Spawning swarm workers with `workspace:"share"` on Windows inside a Dropbox-synced folder risks exposing an `Unknown` or unlinked VCS state to workers without deterministic branch provenance, complicates Windows/WSL2 path mapping, and breaks clean `git -C` integration from the root controller.
- **Adopted Decision**:
  - Explicitly excluded `/.worktrees/` in `.git/info/exclude` (and `.gitignore`) and provisioned dedicated `git worktree` directories (`.worktrees/core`, `.worktrees/actions`, `.worktrees/docs`, `.worktrees/daemon`) anchored directly to `main`, giving each author an isolated branch and filesystem checkout while `@devops` owns all `git`/`gh` staging, merging, and worktree cleanup from Windows PowerShell.

---

## 3. Current Approximations & Simplifying Assumptions (`APP`)

### APP-001 — Unix Advisory `flock` + Linux `/proc/*/fd` vs. Windows `share_mode(0)` File-Lock Detection Semantics
- **Target Scope**: Planned `src/in_use.rs` multi-probe in-use file detection subsystem (v1.0.0 architecture).
- **Nature of the Approximation**:
  - Operating systems expose fundamentally different file-locking guarantees:
    - **Windows**: `OpenOptionsExt::share_mode(0)` performs a kernel-enforced exclusive-access probe against open handles held by external processes (even processes that did not opt into cooperative locking).
    - **Linux**: `flock(LOCK_EX | LOCK_NB)` detects only cooperative advisory locks; to catch active un-flocked writers (e.g., `mysqldump`, `pg_dump`, `rsync`, `tar`), FileCleaner additionally scans `/proc/*/fd` symlink targets matching the candidate's `(dev, ino)` pair.
    - **macOS / BSD (non-Linux Unix)**: `/proc/*/fd` does not exist in the default OS mount; detection relies on `flock(LOCK_EX | LOCK_NB)`, sidecar lock/temp markers (`.lock`, `.lck`, `.part`, `.tmp`, `.processing`), `min_idle_age` cooldown, and the two-point `(size, mtime)` stability window.
- **Justification & Impact**:
  - Provides high-precision, zero-external-binary (`lsof`-free) lock detection across Linux, macOS, and Windows while remaining 100% pure Rust (`std` + `libc` on Unix). On non-Linux Unix systems, un-flocked active writers without sidecar files are caught by `min_idle_age` and the stability window rather than an immediate FD table scan.
- **Removal Condition**:
  - Scheduled for v1.1 enhancement (`IDEA-004`) via native BSD/macOS `libproc` (`proc_pidinfo` / `PROC_PIDLISTFDS`) or `fanotify` / Restart Manager diagnostics without shelling out to external binaries.

---

## 4. Unimplemented Ideas & Future Roadmap (`IDEA`)

| ID | Target Release | Priority | Title | Target Module / Area | Description & Expected Value |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **IDEA-001** | `v1.1` | High | **S3 / Object-Storage Archival Sink Action (`UploadS3`)** | `src/actions.rs`, `src/config.rs` | Stream or upload evicted artifacts to S3-compatible object storage before local deletion, verifying remote `ETag` / SHA-256 digest before unlinking the local file. |
| **IDEA-002** | `v1.1` | High | **Prometheus & OpenTelemetry Metrics Endpoint for Daemon Mode** | `src/daemon.rs`, `src/audit.rs` | Expose an optional lightweight HTTP `/metrics` endpoint in `filecleaner daemon run` reporting swept file counts, reclaimed bytes, backup-guard freezes, in-use skips, and hook latencies. |
| **IDEA-003** | `v1.1` | Medium | **Native OS Recycle Bin / Freedesktop Trash Specification Support** | `src/actions.rs` | Supplement configurable `trash_dir` quarantines with native Freedesktop `.Trash-<uid>` (`info/*.trashinfo`) on Linux and Windows Shell / macOS Trash APIs when `trash_dir` is omitted. |
| **IDEA-004** | `v1.1` | Medium | **macOS `libproc` FD Inspection & Windows Restart Manager (`RmGetList`) Attribution** | `src/in_use.rs` | Enrich `filecleaner check-in-use` output with lock-holder PID and process executable name on macOS (`proc_pidinfo`) and Windows (`rstrtmgr.dll` `RmGetList`) matching Linux `/proc/*/fd` precision. |
