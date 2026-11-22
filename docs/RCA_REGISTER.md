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

### RCA-004 — macOS `/var` -> `/private/var` `TempDir` Symlink Divergence in `hooks_pre_delete_test.rs:142` and Stock macOS `shasum` Portability (`PR #15` / Run `44715379817`)
- **Date / Linked Action**: `2026-11-20` (`ACT-005` / Issue `#14` / PR `#15`)
- **Observed Symptom**:
  - GitHub Actions CI Run `44715379817` passed 4 of 5 jobs (`Format & Clippy`, `Line Coverage Gate >= 90%`, `Cross-OS Test Suite (ubuntu-latest)`, and `Cross-OS Test Suite (windows-latest)` — including strict `cargo clippy --all-targets -- -D warnings` on Windows), passed `57/57` unit tests and `7/8` integration suites (including `daemon_and_cli_test` `6/6`) on `macos-latest`, and failed on a single assertion in [`tests/hooks_pre_delete_test.rs`](../tests/hooks_pre_delete_test.rs):
    - `hook_direct_argv_blocks_shell_injection_and_passes_all_env_vars` (`tests/hooks_pre_delete_test.rs:142`): `PWD=` comparison failed on `macos-latest`.
- **Root Cause Analysis (5 Whys)**:
  1. *Why did `captured.contains(&format!("PWD={}", workdir.display()))` fail on macOS?* Because on macOS `/var` is a symlink to `/private/var`; `tempfile::TempDir` creates paths under `/var/folders/...`, whereas `bash`'s `$(pwd)` inside the hook script resolves the physical path `/private/var/folders/...`.
  2. *Why might stock macOS environments also fail running `examples/hooks/*.sh` without GNU coreutils?* Because bare macOS ships BSD `shasum -a 256` (and `openssl`) rather than GNU `sha256sum`.
- **Applied Change / Fix**:
  - [`tests/hooks_pre_delete_test.rs`](../tests/hooks_pre_delete_test.rs): Updated the `PWD=` assertion in `hook_direct_argv_blocks_shell_injection_and_passes_all_env_vars` to accept either `workdir.display()` or `fs::canonicalize(&workdir)`.
  - [`examples/hooks/verify_checksum_before_delete.sh`](../examples/hooks/verify_checksum_before_delete.sh) & [`examples/hooks/archive_to_s3_mock.sh`](../examples/hooks/archive_to_s3_mock.sh): Added a `compute_sha256()` helper that checks `sha256sum`, falls back to `shasum -a 256` (stock macOS/BSD), and falls back to `openssl dgst -sha256`.
- **Result & Non-Regression Test**:
  - Verified locally in WSL2 (`cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings` across Linux, `x86_64-pc-windows-gnu`, and `aarch64-apple-darwin`, `cargo test` `95/95` including `example_hook_scripts_execute_veto_and_verify_checksums`, and `cargo llvm-cov --fail-under-lines 90`).

---

### RCA-005 — Intermittent macOS `FSEvents` Delayed Self-Unlink Sweep Count Failure in `daemon_notify_and_cooperative_stop_file_work_deterministically` (`main` Post-Merge Run `44716793832`)
- **Date / Linked Action**: `2026-11-20` (`ACT-006` / Issue `#16`)
- **Observed Symptom**:
  - Post-merge `push` CI Run `44716793832` on `main` (commit `aee4028`) passed 4 of 5 jobs (`Format & Clippy`, `Line Coverage Gate >= 90%`, `Cross-OS Test Suite (ubuntu-latest)`, and `Cross-OS Test Suite (windows-latest)`) and failed intermittently on `Cross-OS Test Suite (macos-latest)` at [`src/daemon.rs`](../src/daemon.rs#L1418):
    - `daemon::tests::daemon_notify_and_cooperative_stop_file_work_deterministically`: `assertion left == right failed` (`left: 2`, `right: 1` for `final_state.sweeps_completed`).
- **Root Cause Analysis (5 Whys)**:
  1. *Why did `final_state.sweeps_completed` equal `2` instead of `1` on `macos-latest`?* Because the notify watcher fired a second debounced sweep after sweep #1 had already completed and returned to `DaemonStatus::Idle`.
  2. *Why did a filesystem event trigger a second sweep after sweep #1?* Because sweep #1 itself unlinked `a_old.log` inside `watch_dir`, and macOS `FSEvents` delivers directory-change notifications asynchronously via `fseventsd` with non-deterministic coalescing latency. When the self-deletion notification for `a_old.log` arrives after the `30ms` debounce window (`cfg.daemon.debounce = Duration::from_millis(30)`), the daemon runs another sweep under `max_sweeps = Some(5)` before processing `stop_file`.
  3. *Why did the test fail intermittently?* Because the unit test asserted strict equality `assert_eq!(final_state.sweeps_completed, 1)` rather than verifying bounded notify execution (`1..=5` matching `max_sweeps = Some(5)`) alongside exact retention (`!old.exists()`, `newer.exists()`) and fast cooperative shutdown (`< 2s`, `DaemonStatus::Stopped`).
- **Applied Change / Fix**:
  - [`src/daemon.rs`](../src/daemon.rs): Updated `daemon_notify_and_cooperative_stop_file_work_deterministically` to assert `assert!((1..=5).contains(&final_state.sweeps_completed));` under `max_sweeps = Some(5)`, preserving all retention and cooperative stop-file assertions.
- **Result & Non-Regression Test**:
  - Verified locally in WSL2 (`cargo fmt --all -- --check` PASS, `cargo clippy --all-targets -- -D warnings` PASS with `0` warnings, `cargo test` PASS `95/95` (`57` unit + `38` integration across 8 suites), `cargo llvm-cov --all-targets --workspace --summary-only --fail-under-lines 90` PASS at `92.84%` line coverage across `8,082` lines). Pending PR & post-merge multi-OS CI verification across `ubuntu-latest`, `windows-latest`, and `macos-latest`.

---

### RCA-006 — Ponytail Security, Correctness, Scale, and CI Hardening Audit (`Issue #18`)
- **Date / Linked Action**: `2026-11-20` (`ACT-007` / Issue `#18`)
- **Observed Symptom**:
  - A full post-`v1.0.0` Ponytail engineering and security audit uncovered 6 critical security/correctness/scale defects alongside 8 companion CI, permission, latency, and code-deduplication improvements:
    1. **Symlink-swap TOCTOU & world-readable staging (`#1`)**: `copy_stream_with_sha256`, `compress_file_staged`, `execute_copy_truncate`, and `execute_pipeline` opened candidate files with default following semantics and default umask `.fc-tmp` staging, allowing a symlink swapped after `Scanner::scan_rule` to hijack reads, overwrites, or truncation.
    2. **Daemon in-place `.gz.gz` / `_date_date` infinite loop (`#2`)**: Repeated daemon sweeps re-matched already date-stamped or already compressed files on rules lacking terminal removal steps, appending duplicate `_YYYYMMDD_HHMMSS` stamps and `.gz.gz` extensions indefinitely.
    3. **In-place quota accounting error (`#3`)**: `RetentionEngine::plan` unconditionally decremented `remaining_files` and `remaining_bytes` for every evicted candidate, falsely reporting `max_files` / `max_total_size` satisfied on `RenameByDate`, `Compress` (`compress_remove_source = false`), and `CopyTruncate` (`min_file_size > 0` vs `0`) instead of `RuleOutcome::Constrained`.
    4. **Hook grandchild pipe deadlock & unbounded output (`#4`)**: `ProcessHookRunner` piped child stdout and joined a blocking stderr thread after child exit; background grandchildren inheriting FDs blocked `wait()` until timeout, while unconstrained stdout/stderr risked memory exhaustion.
    5. **Intrusive exclusive OS lock probing (`#5`)**: `plan`/`--dry-run` took exclusive `flock(LOCK_EX)` / Windows `share_mode(0)` probes on every file including protected floor $P_{\text{floor}}$ and recent files before checking sidecars, `/proc`, or `min_idle_age`.
    6. **Recycled-PID `SIGTERM` risk in `daemon stop` (`#6`)**: `daemon_stop` signaled any live PID found in `pid_file` without verifying executable identity, risking killing an unrelated process after an ungraceful crash and OS PID reuse.
    - Companion remediations (`#7`–`#14`): unpinned GitHub Actions tags and script interpolation in workflows (`#7`), default-umask audit/state files and unbounded `audit.jsonl` growth (`#8`), per-rule `/proc` re-walking + redundant `read_link` (`#9`), coarse stop polling (`#10`), missing adversarial regression tests (`#11`), duplicated path normalizers (`#12`), repeated rule regex/glob compilation and duplicate CLI atomic writer (`#13`), and serial `check-in-use` stability sleeps (`#14`).
- **Root Cause Analysis (5 Whys)**:
  1. *Why were TOCTOU and FD/PID hazards possible across multi-phase boundaries?* Because scan, lock-probe, retention, hook, and execution phases operated over plain `PathBuf` / numeric `pid` tokens without kernel handle pinning (`O_NOFOLLOW` / `FILE_FLAG_OPEN_REPARSE_POINT`), `(dev, ino)` continuity, or process image validation.
  2. *Why did daemon sweeps loop on in-place transforms and misreport quotas?* Because `Scanner` and `ActionExecutor` lacked idempotency checks for already stamped/compressed outputs, and `RetentionEngine::plan` treated all action pipelines identically to `Delete`.
  3. *Why did hook execution and lock probing introduce hang/contention/O(N) latency risks?* Because `ProcessHookRunner` blocked on inherited pipe EOF instead of `Stdio::null()` + bounded `mpsc` timeout, `InUseDetector` probed OS locks eagerly before cheaper/non-intrusive layers, and `cli check-in-use` looped per path.
- **Applied Change / Fix**:
  - [`src/actions.rs`](../src/actions.rs) & [`src/engine.rs`](../src/engine.rs): Added `open_staged_tmp_private` (`0o600`), `open_regular_nofollow_read` / `open_regular_nofollow_write` (`O_NOFOLLOW` + `(dev, ino)` verification on Unix; `FILE_FLAG_OPEN_REPARSE_POINT` + reparse-point bit check on Windows), `execute_copy_truncate` pre/post identity check, pre-execute symlink re-check, plus `has_compressed_extension` / `has_trailing_date_stamp` idempotency.
  - [`src/scanner.rs`](../src/scanner.rs) & [`src/config.rs`](../src/config.rs): Added `CompiledRuleMatchers` / `compile_rule_matchers`, `Scanner::compile_config` / `scan_rule_compiled`, scanner-level in-place idempotency skip, `<audit_log>.1` exclusion, and unified `crate::config::resolve_path_lexically`.
  - [`src/policy.rs`](../src/policy.rs): Added `compute_protected_floor_paths` and tracked `removes_file` / `reclaims_bytes` separately so `remaining_files`, `remaining_bytes`, and `RuleOutcome::Constrained` stay exact for in-place transforms and `CopyTruncate`.
  - [`src/hooks.rs`](../src/hooks.rs): Switched stdout to `Stdio::null()`, capped stderr to `64 KiB` via `sync_channel` (`50 ms` drain grace), isolated Unix `process_group(0)`, and added `kill_hook_process_tree` (`SIGKILL` to `-pgid` on timeout).
  - [`src/in_use.rs`](../src/in_use.rs) & [`src/cli.rs`](../src/cli.rs): Added `LockProbe::check_os_lock_readonly` (`LOCK_SH | LOCK_NB` / `FILE_SHARE_READ`), `SweepLockProbe` single-pass `OnceLock` `/proc` snapshot (removing redundant `read_link`), `InUseDetector::check_batch_scoped` (Layers 1/3/5 before OS locks, skipping `P_floor`), and batched `InUseDetector::check_paths`; deleted duplicate `write_atomic_cli_file`.
  - [`src/audit.rs`](../src/audit.rs) & [`src/daemon.rs`](../src/daemon.rs): Enforced `0o600` Unix permissions on `audit.jsonl`, `pid_file`, `state_file`, `stop_file`, and `init` configs; added `50 MiB` `.1` audit rotation (`DEFAULT_AUDIT_ROTATE_BYTES`); added `is_filecleaner_process` guard in `daemon_stop`; lowered `STOP_POLL_SLICE` to `200 ms`; deleted duplicate `normalize_lexical`.
  - [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) & [`.github/workflows/release.yml`](../.github/workflows/release.yml): Pinned all `uses:` steps to 40-char commit SHAs, replaced script `${{ ... }}` interpolation with `env:`, and scoped `contents: write` solely to `publish-release`.
- **Result & Non-Regression Test**:
  - Added regression tests across [`tests/actions_pipeline_test.rs`](../tests/actions_pipeline_test.rs), [`tests/hooks_pre_delete_test.rs`](../tests/hooks_pre_delete_test.rs), [`tests/retention_age_size_count_test.rs`](../tests/retention_age_size_count_test.rs), [`tests/in_use_detection_test.rs`](../tests/in_use_detection_test.rs), [`tests/daemon_and_cli_test.rs`](../tests/daemon_and_cli_test.rs), [`tests/governance_and_docs_test.rs`](../tests/governance_and_docs_test.rs), and unit modules.
  - Verified in WSL2 (`/var/tmp/filecleaner-target-verify`): `cargo fmt --all -- --check` PASS, `cargo clippy --all-targets -- -D warnings` PASS (Linux + `x86_64-pc-windows-gnu` + `aarch64-apple-darwin`), `cargo test --all-targets` PASS (`112/112`: `62` unit + `50` integration across 8 suites), and `cargo llvm-cov --fail-under-lines 90` PASS (`93.09%` lines, `90.85%` regions across `8,678` lines).

### RCA-007 — Post-v1.0.1 Audit Edge Cases (Hook Stderr Drain, Ad-Hoc Rename Template, Foreign PID Status, and Cookbook JSON Schema) (`Issue #20`)

- **Observed Symptom**:
  1. A failing pre-delete hook that left a background grandchild holding `fd 2` open did not deliver captured `stderr` over the one-shot channel unless EOF completed before returning, risking lost failure diagnostics.
  2. Malformed `rename_template` values (empty or invalid `strftime` specifiers such as `"%Q"`) passed `filecleaner validate` and failed only at action execution time; passing ad-hoc `--rename-template` without `--rename-by-date` silently ignored the template.
  3. Recycled OS PIDs held by non-FileCleaner processes caused `daemon status` and `claim_pid_file` (which checked only `is_pid_alive`) to treat an unrelated process as an active FileCleaner daemon.
  4. `docs/en/COOKBOOK_EXAMPLES.md` and `docs/fr/EXEMPLES_RECETTES.md` showed a flattened `check-in-use --json` payload without the real `{ all_safe, busy_count, reports: [...] }` envelope and `kind` tags.
  5. Test coverage of `src/actions.rs` started below `90%` (`88.39%`) due to untested multi-chunk, `CopyTruncate`, `DryRun`, collision, and error-boundary branches.
- **Root Cause Analysis (5 Whys)**:
  - `hooks.rs` waited to receive the full byte buffer on EOF instead of reading incrementally into a shared bounded buffer with a drain grace timeout; `config.rs` checked `move_date_subdir` in `validate()` while skipping `rename_template`; `cli.rs` gated `ActionStep::RenameByDate` only on `--rename-by-date`; `daemon_stop` used `is_filecleaner_process` while `daemon_status`/`claim_pid_file` checked `is_pid_alive` alone; documentation JSON and code blocks were not yet cross-checked by integration tests.
- **Applied Change / Fix**:
  - Updated `src/hooks.rs` (`Arc<Mutex<Vec<u8>>>` bounded capture up to `64 KiB` at UTF-8 char boundaries + `STDERR_DRAIN_GRACE`), `src/config.rs` (eager `rename_template` validation), `src/cli.rs` (`rename_by_date || rename_template.is_some()`), `src/daemon.rs` (`is_live_filecleaner_pid`), `src/scanner.rs` (lean regex reuse), fixed Cookbook JSON examples, published EN/FR `HOWTO.md`, and enforced doc config validation in `tests/governance_and_docs_test.rs`.
- **Result & Non-Regression Test**:
  - Verified in WSL2 (`CARGO_TARGET_DIR=/var/tmp/filecleaner-target-plan`): `cargo fmt --all` PASS, 3-target `cargo clippy --all-targets -- -D warnings` PASS (`0` warnings), `cargo test --all-targets` PASS (`124/124` tests across `src/lib.rs` and `9` integration suites including `tests/product_workflows_e2e_test.rs`), and `cargo llvm-cov --fail-under-lines 90` PASS (`94.31%` line coverage across `9,098` lines; `actions.rs` lifted to `90.21%`, all modules `>= 90.21%`).

### RCA-008 — Windows CI Test Failures (`share_mode(0)` Open Write Handle, NTFS `:` Filename Character, and Top-Level TOML Array) (`Issue #20` / `PR #21`)

- **Observed Symptom**:
  - `Cross-OS Test Suite (windows-latest)` failed in `src/lib.rs` on `cli::tests::execute_cli_covers_validate_check_in_use_init_sweep_and_daemon_branches` (destination `app_20231114.log` missing because file remained protected as busy) and `scanner::tests::scanner_rejects_non_directory_root_and_applies_max_depth_size_bounds_and_rfc3339_dates` (`InvalidFilename` / Win32 error `123`).
- **Root Cause Analysis (5 Whys)**:
  1. `src/cli.rs` kept `f_handle` (`OpenOptions::new().write(true)`) open when invoking `execute_cli(...)`; Unix permissive descriptors allowed rename anyway, whereas FileCleaner's own Windows `share_mode(0)` probe properly detected the open write handle and blocked eviction.
  2. `src/scanner.rs` wrote sample test filenames containing `:` (`09:00:00+00:00`), which are illegal on Windows NTFS.
  3. Inspection also revealed `tests/product_workflows_e2e_test.rs` serialized `Vec<String>` PowerShell hook arguments via `toml::to_string(&hook_args).unwrap_or_else(|_| "[]")`, which silently fell back to `"[]"` because top-level TOML serialization requires a table.
- **Applied Change / Fix**:
  - Explicitly `drop(f_handle);` in `src/cli.rs` before running `execute_cli(...)`.
  - Replaced colon timestamps in disk test filenames inside `src/scanner.rs` with Windows-safe ISO-8601 basic timezone notation (`%Y-%m-%dT%H%M%S%z`, `evt_2026-11-18T090000+0000.log`).
  - Serialized `hook_args` with `serde_json::to_string(&hook_args)` inside `tests/product_workflows_e2e_test.rs` so backslash-escaped TOML string arrays preserve Windows PowerShell arguments intact.
- **Result & Non-Regression Test**:
  - Re-verified in WSL2 (`fmt`, 3-target `clippy -D warnings`, `cargo test --all-targets` `124/124`, `cargo llvm-cov --fail-under-lines 90` `94.31%` across `9,098` lines) and pushed commit to trigger full Windows/macOS/Linux GitHub Actions re-run on `PR #21`.

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

### APP-002 — Fixed `50 MiB` Single-Generation `<audit_log>.1` JSONL Audit Rotation Threshold
- **Target Scope**: `src/audit.rs` (`DEFAULT_AUDIT_ROTATE_BYTES`), `src/scanner.rs` (`build_internal_guard`), `src/daemon.rs` (`build_ignored_paths`).
- **Nature of the Approximation**:
  - `AuditLogger::open` delegates to `AuditLogger::open_with_max_bytes(path, DEFAULT_AUDIT_ROTATE_BYTES)` with a compiled-in `50 MiB` (`52,428,800` bytes) cap and single-generation backup `<audit_log>.1` (overwriting `.1` on the next `50 MiB` boundary) rather than exposing new `[audit]` configuration keys in `AuditConfig` or unbounded numbered archive chains.
- **Justification & Impact**:
  - Prevents long-running `filecleaner daemon` instances from exhausting disk space with unbounded JSONL growth while preserving zero breaking changes to `AuditConfig` (`v1.0.0` schema contract) and zero external logrotate crate dependencies. Guarded identically alongside the primary audit log so sweep rules never prune `<audit_log>.1`.
- **Removal Condition**:
  - May be parameterized via optional `[audit].max_bytes` / `max_rotated_files` schema fields in a future `v1.1` minor configuration extension if operators require multi-generation retention inside FileCleaner itself.

---

### APP-003 — Pure-Rust TLS Cryptography via `rustls-rustcrypto` (`v0.1.0-pre.0`) Over `ureq` v3 (`rustls-no-provider`)
- **Target Scope**: `Cargo.toml`, `src/s3.rs` (`HttpS3Uploader`), `tests/governance_and_docs_test.rs`.
- **Nature of the Approximation**:
  - To preserve `IMP-001` (zero C/C++ `-sys` build dependencies, zero `ring`, zero `aws-lc-sys`, zero `openssl-sys`) across all 9 release cross-compilation targets (including `aarch64-unknown-linux-musl`, `armv7-unknown-linux-gnueabihf`, and `aarch64-pc-windows-msvc`), `HttpS3Uploader` configures `ureq` v3 (`default-features = false`, `rustls-no-provider`, `rustls-webpki-roots`) with `rustls` v0.23 and the pure-Rust `rustls-rustcrypto` `CryptoProvider` (`0.1.0-pre.0`, RustCrypto implementations supporting RSA, ECDSA, Ed25519, AES-GCM, and ChaCha20-Poly1305).
  - Note that `Cargo.lock` still records `ring` solely as an inactive optional dependency entry of `rustls-webpki` (`alloc,std` features only); `cargo tree` across Linux, Windows, and macOS confirms `ring` and `cc` are absent from the compiled build graph.
- **Justification & Impact**:
  - Guarantees deterministic single-binary cross-compilation without C cross-compilers or assembler toolchains while supporting standard TLS 1.2/1.3 endpoints including AWS S3 RSA certificates and MinIO TLS gateways. Slightly lower raw TLS throughput than hand-tuned C assembly (`aws-lc-rs`) is negligible compared to WAN/LAN S3 archival I/O.
- **Removal Condition**:
  - Upgrade `rustls-rustcrypto` to a stable `0.1.x` / `1.0.x` release once published by the RustCrypto organization, without reintroducing C/C++ build dependencies.

---

## 4. Unimplemented Ideas & Future Roadmap (`IDEA`)

| ID | Target Release | Priority | Title | Target Module / Area | Description & Expected Value |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **IDEA-001** | `v1.1` | High | **S3 / Object-Storage Archival Sink Action (`UploadS3`)** | `src/actions.rs`, `src/config.rs` | Stream or upload evicted artifacts to S3-compatible object storage before local deletion, verifying remote `ETag` / SHA-256 digest before unlinking the local file. |
| **IDEA-002** | `v1.1` | High | **Prometheus & OpenTelemetry Metrics Endpoint for Daemon Mode** | `src/daemon.rs`, `src/audit.rs` | Expose an optional lightweight HTTP `/metrics` endpoint in `filecleaner daemon run` reporting swept file counts, reclaimed bytes, backup-guard freezes, in-use skips, and hook latencies. |
| **IDEA-003** | `v1.1` | Medium | **Native OS Recycle Bin / Freedesktop Trash Specification Support** | `src/actions.rs` | Supplement configurable `trash_dir` quarantines with native Freedesktop `.Trash-<uid>` (`info/*.trashinfo`) on Linux and Windows Shell / macOS Trash APIs when `trash_dir` is omitted. |
| **IDEA-004** | `v1.1` | Medium | **macOS `libproc` FD Inspection & Windows Restart Manager (`RmGetList`) Attribution** | `src/in_use.rs` | Enrich `filecleaner check-in-use` output with lock-holder PID and process executable name on macOS (`proc_pidinfo`) and Windows (`rstrtmgr.dll` `RmGetList`) matching Linux `/proc/*/fd` precision. |
