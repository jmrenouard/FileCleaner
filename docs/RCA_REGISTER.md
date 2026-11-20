# RCA Register — Errors, Technical Dead-Ends, Approximations & Unimplemented Ideas

> **Governance Rule ([`AGENTS.md`](../AGENTS.md) §3 & [`.agents/rules/project-governance.md`](../.agents/rules/project-governance.md))**:
> This register guarantees full engineering traceability for the **FileCleaner** project (strictly in English). It **MUST be systematically updated on every request, every change, and every result** across its four sections:
> 1. **Errors & Bugs Encountered (Root Cause Analysis — `RCA-XXX`)**
> 2. **Technical Dead-Ends & Abandoned Leads (`IMP-XXX`)**
> 3. **Current Approximations & Simplifying Assumptions (`APP-XXX`)**
> 4. **Unimplemented Ideas & Future Roadmap (`IDEA-XXX`)**

---

## 1. Errors & Bugs Encountered (Root Cause Analysis — `RCA`)

### RCA-001 — WSL2 `/tmp` `tmpfs` Exhaustion (`ENOSPC`) Under Concurrent `CARGO_TARGET_DIR` Build Caches
- **Date / Linked Action**: `2026-11-20` (`ACT-004` / Issue `#12`)
- **Observed Symptom**:
  - During Wave 3 `cargo test` compilation inside WSL2 Ubuntu-24.04, Rust build and `TempDir` creation failed with `No space left on device (os error 28)` (`ENOSPC`) while `/tmp` `tmpfs` reached `7.7G / 7.7G` (`100%` full).
- **Root Cause Analysis (5 Whys)**:
  1. *Why did `/tmp` run out of disk space?* Because cumulative Cargo build and LLVM coverage target caches from Wave 1 and Wave 2 workers (`/tmp/filecleaner-target`, `/tmp/filecleaner-target-core`, `/tmp/filecleaner-target-actions`) consumed `5.0G` inside the `7.7G` RAM-backed `/tmp` `tmpfs`.
  2. *Why were multiple large target caches kept in `/tmp`?* Because isolated worktree builds defaulted to `/tmp/filecleaner-target-*` without pruning stale worker caches or using persistent Linux storage (`/var/tmp`, `769G` free).
- **Applied Change / Fix**:
  - Cleared stale `/tmp/filecleaner-target*` caches from `/tmp` to restore healthy `TempDir` execution and standardized Wave 3+ WSL2 builds and coverage runs on `CARGO_TARGET_DIR="/var/tmp/filecleaner-target-*"` (strictly outside Dropbox and off RAM-limited `/tmp`).
- **Result & Non-Regression Test**:
  - Full `cargo test` (`94/94` passing) and `cargo llvm-cov` (`92.67%`–`92.68%` coverage) completed with zero `ENOSPC` errors.

---

### RCA-002 — Cross-OS GitHub Actions CI Failures & Warnings on `windows-latest` and `macos-latest` (`PR #15` / Run `44712069431`)
- **Date / Linked Action**: `2026-11-20` (`ACT-005` / Issue `#14` / PR `#15`)
- **Observed Symptom**:
  - While `ubuntu-latest` passed `fmt-and-clippy`, `coverage-gate >= 90%`, and `cross-os-test` (`94/94`), GitHub Actions CI Run `44712069431` failed on `windows-latest` and `macos-latest` with:
    1. `src/in_use.rs:956`: `assertion failed: direct_missing.fatal_error.is_some()` on both Windows and macOS.
    2. `src/scanner.rs:909`: `assertion left != right failed` (`resolved_ts == modified_ts`) on `macos-latest`.
    3. `src/scanner.rs:982`: `overflow when adding duration to SystemTime` panic on `windows-latest`.
    4. Windows compiler warnings (`src/daemon.rs:514-515` unused `mut signal_sent` and `target_pid`; `tests/hooks_pre_delete_test.rs` unused imports/helpers; `tests/retention_age_size_count_test.rs:6` unused `FileCleanerError`) under `#[cfg(unix)]`-gated test/signal paths.
- **Root Cause Analysis (5 Whys)**:
  1. *Why did `src/in_use.rs:956` fail on Windows/macOS?* Because `OsLockProbe::check_proc_fd_batch` only scans `/proc/*/fd` on `#[cfg(target_os = "linux")]` and returns an empty `ProcFdProbeOutcome` on non-Linux platforms, yet the test unconditionally asserted `fatal_error.is_some()`.
  2. *Why did `src/scanner.rs:909` fail on macOS APFS?* Because APFS clamps `st_birthtime` backward to `mtime` whenever `mtime` is explicitly set older than `birthtime` (`1_500_000_000` in 2017 vs. 2026 creation time), making `created_ts == modified_ts`.
  3. *Why did `src/scanner.rs:982` panic on Windows?* Because Windows `SystemTime` (`FILETIME`) caps out around year 30828 AD, so unchecked `UNIX_EPOCH + Duration::from_secs(u64::MAX / 2)` overflows inside `SystemTime::add`.
  4. *Why were Windows-specific warnings and handle-lifetime subtleties missed before pushing?* Because local verification ran exclusively on WSL2 Linux (`ubuntu-latest`) and `cross-os-test` in `.github/workflows/ci.yml` ran `cargo test --all-targets` without `cargo clippy --all-targets -- -D warnings`.
- **Applied Change / Fix**:
  - [`src/in_use.rs`](../src/in_use.rs): Gated `direct_missing.fatal_error.is_some()` with `#[cfg(target_os = "linux")]` and asserted empty/non-fatal outcome under `#[cfg(not(target_os = "linux"))]`.
  - [`src/scanner.rs`](../src/scanner.rs): Updated `scan_rule_time_basis_created_succeeds_or_fails_without_fallback` to advance `mtime` into the future (`>= 1_900_000_000`) so APFS/NTFS/ext4 preserve distinct `birthtime`, and switched overflow tests in `system_time_overflow_and_lexical_path_resolution_handle_edge_cases` to `checked_add` / `checked_sub`.
  - [`src/daemon.rs`](../src/daemon.rs): Replaced `mut signal_sent` / `target_pid` in `daemon_stop` with clean `#[cfg(unix)]` / `#[cfg(not(unix))]` branches, added bounded Windows `PermissionDenied` retry on atomic state rename, scoped file-handle drops before daemon deletions, and enabled cross-OS stop-file shutdown coverage.
  - Integration Tests ([`tests/hooks_pre_delete_test.rs`](../tests/hooks_pre_delete_test.rs), [`tests/retention_age_size_count_test.rs`](../tests/retention_age_size_count_test.rs), [`tests/in_use_detection_test.rs`](../tests/in_use_detection_test.rs), [`tests/daemon_and_cli_test.rs`](../tests/daemon_and_cli_test.rs)): Gated Unix-only imports/helpers cleanly, added cross-platform and `#[cfg(windows)]` hook (`powershell.exe` / `ping.exe`) and `share_mode(0)` lock tests, and removed Notify/mtime timing races.
  - [`.github/workflows/ci.yml`](../.github/workflows/ci.yml): Added `components: clippy` and `cargo clippy --all-targets -- -D warnings` to `cross-os-test` across all 3 runner OSes.
- **Result & Non-Regression Test**:
  - Verified locally in WSL2 (`cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo llvm-cov --fail-under-lines 90`) and queued for multi-OS GitHub Actions re-verification across `ubuntu-latest`, `windows-latest`, and `macos-latest`.

---

### RCA-003 — `windows-latest` `clippy::needless_bool` and `macos-latest` Zombie `is_pid_alive` Timeout (`PR #15` / Run `44714006240`)
- **Date / Linked Action**: `2026-11-20` (`ACT-005` / Issue `#14` / PR `#15`)
- **Observed Symptom**:
  - GitHub Actions CI Run `44714006240` passed all three `ubuntu-latest` jobs (`fmt-and-clippy`, `coverage-gate >= 90%`, `cross-os-test`), passed `57/57` unit tests and `7/8` integration suites on `macos-latest`, and surfaced two platform-specific defects:
    1. `windows-latest` (`cargo clippy --all-targets -- -D warnings`): failed on `src/actions.rs:914` with `clippy::needless_bool` (`this if guard returns a bool literal and is followed by another`).
    2. `macos-latest` (`tests/daemon_and_cli_test.rs:643`): `cli_daemon_subprocess_lifecycle_notify_status_duplicate_pid_stop_and_templates` timed out in `filecleaner daemon stop --timeout 10s` (`timed out after 10000ms waiting for daemon PID ... to stop`).
- **Root Cause Analysis (5 Whys)**:
  1. *Why did `src/actions.rs:914` trigger `clippy::needless_bool` on Windows?* Because `#[cfg(unix)]` compiled out the `libc::EXDEV` branch on Windows, collapsing `is_cross_device_error` to `if err.kind() == io::ErrorKind::CrossesDevices { return true; } false`.
  2. *Why did `daemon stop` time out on macOS?* Because the test spawns `filecleaner daemon run` as a child process and holds its un-waited `std::process::Child` handle until after `daemon stop` returns; when the daemon exits, it remains a zombie (`Z`) until reaped. On Linux `is_linux_zombie` checks `/proc/<pid>/stat`, whereas on macOS `kill(pid, 0)` continued returning `0` on the zombie process.
  3. *Why did `is_pid_alive` on Windows use `tasklist.exe`?* Because no native Win32 handle check was wired, risking unnecessary process-spawn overhead and differing exit semantics for un-waited `Child` handles (`STILL_ACTIVE`).
- **Applied Change / Fix**:
  - [`src/actions.rs`](../src/actions.rs): Rewrote `is_cross_device_error` with clean `#[cfg(unix)]` and `#[cfg(not(unix))]` boolean expressions.
  - [`src/daemon.rs`](../src/daemon.rs): Added `is_non_linux_unix_zombie` (`ps -o stat= -p <pid>`) under `#[cfg(all(unix, not(target_os = "linux")))]` so macOS/BSD zombies report `false`, and replaced `tasklist.exe` under `#[cfg(windows)]` with direct `kernel32` FFI (`OpenProcess` + `GetExitCodeProcess` checking `STILL_ACTIVE == 259` + `CloseHandle`).
  - Local Cross-Target Verification: Added `x86_64-pc-windows-gnu` and `aarch64-apple-darwin` targets in WSL2 to run `cargo clippy --lib --bins --tests --target <triple> -- -D warnings` locally prior to pushing.
- **Result & Non-Regression Test**:
  - Cross-target Clippy (`x86_64-pc-windows-gnu`, `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`) passed with zero warnings, alongside full WSL2 `cargo fmt`, `cargo test`, and `cargo llvm-cov --fail-under-lines 90`.

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

### IMP-004 — Retaining Deprecated `filetime = "0.2"` Crate Alongside `[lints.cargo] unused_dependencies = "allow"`
- **Context**: Removing Wave 1 manifest lint suppressions in [`Cargo.toml`](../Cargo.toml) during Wave 3 integration (`ACT-004` / Issue `#12`).
- **Why It Is a Dead-End**:
  - `src/actions.rs` preserves timestamps using Rust standard library `std::fs::File::set_times` (`std::fs::FileTimes`, stable since Rust `1.75`) instead of `filetime::set_file_times` (which is deprecated in `filetime 0.2.30`). Keeping `filetime = "0.2"` in `Cargo.toml` triggered `cargo::unused_dependencies` warnings once `[lints.cargo]` was removed.
- **Adopted Decision**:
  - Removed `filetime = "0.2"` from [`Cargo.toml`](../Cargo.toml) (and [`Cargo.lock`](../Cargo.lock)) and removed `[lints.cargo] unused_dependencies = "allow"`, leaving zero unused dependencies and zero compiler/manifest warnings.

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
