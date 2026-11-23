# Release Notes — FileCleaner v1.0.1

**Release Tag:** `v1.0.1` | **Date:** 2026-11-20 | **Stability:** Production Stable  
**Artifacts:** 9 Cross-Platform Native Archives (`SHA256SUMS.txt`) + Multi-Arch Docker Hub Image (`jmrenouard/filecleaner` for `linux/amd64`, `linux/arm64`)

---

## Executive Summary

**FileCleaner v1.0.1** hardens the initial `v1.0.0` release with post-release security and reliability audit fixes, TOCTOU symlink-swap protections, non-exclusive read-only dry-run probes, bounded hook `stderr` draining under background grandchildren, eager `rename_template` validation, recycled-PID daemon safety checks, a beginner-friendly bilingual 5-minute HOWTO, and official multi-platform **Docker Hub** container publishing (`jmrenouard/filecleaner`).

---

## Key Highlights & Remediations

### 1. Security & Runtime Hardening
- **Symlink-Swap TOCTOU Guard (`src/actions.rs`, `src/engine.rs`)**: Non-following file opens (`O_NOFOLLOW` + `(st_dev, st_ino)` identity checks on Unix; `FILE_FLAG_OPEN_REPARSE_POINT` + reparse-attribute rejection on Windows) and inode continuity validation prior to `copy_truncate` truncation.
- **Private `0o600` Artifacts & Audit Rotation (`src/audit.rs`, `src/daemon.rs`, `src/cli.rs`)**: Enforced `0o600` Unix permissions on generated starter configs, `.fc-tmp` staging files, `pid_file`, `state_file`, `stop_file`, and `audit.jsonl`, with automatic `50 MiB` `.1` audit rotation excluded from rule scans.
- **Pre-Delete Hook Isolation & Stderr Preservation (`src/hooks.rs`)**: Isolated Unix hook process groups (`SIGKILL` full group on timeout), detached child stdout, and replaced blocking channel reads with a bounded `Arc<Mutex<Vec<u8>>>` buffer (`64 KiB` cap, UTF-8 boundary safe) + `STDERR_DRAIN_GRACE` so failing hooks leaving lingering grandchildren preserve diagnostics without hanging.
- **Daemon PID Identity Protection (`src/daemon.rs`)**: Enforced `is_live_filecleaner_pid` (`is_pid_alive` + `is_filecleaner_process`) across `daemon run`, `daemon status`, and `daemon stop` so recycled OS PIDs from unrelated processes are safely treated as stale.
- **Supply-Chain Pinned Workflows (`.github/workflows/*.yml`)**: All GitHub Actions dependencies pinned to immutable 40-character commit SHAs with environment-passed variables (`permissions: contents: read` default).

### 2. Correctness, CLI & Retention Improvements
- **Non-Exclusive Dry-Run Probing & Single-Pass Proc Snapshot (`src/in_use.rs`, `src/engine.rs`)**: Read-only `LOCK_SH | LOCK_NB` / `FILE_SHARE_READ` lock probing during `plan` / `--dry-run`, single `/proc/*/fd` snapshot per sweep (`SweepLockProbe`), and batched `filecleaner check-in-use <PATH...>` probing.
- **Idempotent Multi-Sweep In-Place Actions (`src/actions.rs`, `src/scanner.rs`, `src/policy.rs`)**: Prevents `_date_date` or `.gz.gz` compounding across daemon passes and accurately tracks quotas across `copy_truncate` and `compress_remove_source = false`.
- **Ad-Hoc `--rename-template` & Eager Validation (`src/config.rs`, `src/cli.rs`)**: `Config::validate()` rejects invalid `strftime` `rename_template` values eagerly, and passing `--rename-template` in ad-hoc CLI mode activates `ActionStep::RenameByDate` automatically.

### 3. Multi-Arch Docker Hub Image (`jmrenouard/filecleaner`)
- **Non-Root Minimal Image**: Multi-stage build (`rust:1.100.0-bookworm` -> `debian:bookworm-slim`) running as unprivileged UID/GID `10001` (`fcuser:fcgroup`), with `/data`, `/etc/filecleaner`, `/var/lib/filecleaner`, `/var/log/filecleaner` volumes, `HEALTHCHECK`, and exec-form entrypoint.
- **Quickstart**:
  ```bash
  docker run --rm jmrenouard/filecleaner:1.0.1 --version
  docker run --rm -v "$(pwd)/data:/data" jmrenouard/filecleaner:1.0.1 plan --dir /data --max-age 14d
  ```

### 4. Documentation & Verification Metrics
- **Bilingual Guides (`EN` & `FR`)**: Added 5-minute beginner guides ([`docs/en/HOWTO.md`](en/HOWTO.md) and [`docs/fr/HOWTO.md`](fr/HOWTO.md)), aligned Cookbook `check-in-use --json` wrapper schemas, and automated TOML/YAML config block validation in `tests/governance_and_docs_test.rs`.
- **Verification**: `cargo fmt --all -- --check` clean, `cargo clippy --all-targets -- -D warnings` clean (`0` warnings across Linux, `x86_64-pc-windows-gnu`, `aarch64-apple-darwin`), **`124+` passing unit & integration tests**, and **`94.31%` line coverage** (`cargo llvm-cov --fail-under-lines 90`).
