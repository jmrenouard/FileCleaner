# Changelog

All notable changes to **FileCleaner** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.0.0] - 2026-11-20

### Added

- **Fail-Closed 6-Phase Retention Engine (`Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`)**
  - Deterministic newest-first ordering with normalized POSIX relative-path tie-breaking.
  - `TimeBasis` support for `modified` (`mtime`), `created` (`birthtime`, fail-closed without silent fallback on unsupported filesystems), and `filename_date` (`regex` + `chrono` `strftime`, fail-closed skip on unparseable names).
  - Glob (`globset`) and Rust `regex` include/exclude filters, `min_file_size` / `max_file_size` bounds, `recursive` + `max_depth`, and safe `follow_symlinks = false` default.
  - Flexible retention caps (`max_files`, `max_age`, `max_total_size`) with `CombineMode::{Any, All}`, `EvictionOrder::{OldestFirst, LargestFirst}`, human-readable SI/IEC byte sizes (`B`..`PB`/`PiB`), and compound durations (`ms`, `s`, `m`, `h`, `d`, `w`).

- **Backup Safety Guardrails (`backup_guard`)**
  - Minimum valid backup threshold (`min_valid_backup_bytes`, alias `min_valid_size`) filtering out corrupt/0-byte stubs prior to floor or freshness checks.
  - Protected newest valid backup floor (`min_keep_files`) strictly excluded from eviction (`E ∩ P_floor = ∅`).
  - Dead-man's-switch staleness freezes (`require_newer_within`, `freeze_if_newest_older_than`, `freeze_on_empty_valid`).
  - Non-destructive constraint conflict resolution (`RuleOutcome::Constrained`) ensuring safety floors and active locks always win, with optional strict exit code `3` via `--strict-constraints` and `--fail-on-freeze`.

- **6-Layer In-Use File Detection Matrix (`in_use` & `check-in-use`)**
  - Sidecar and partial-transfer suffix detection (`.lock`, `.lck`, `.part`, `.tmp`, `.processing`, configurable).
  - Unix cooperative `flock(LOCK_EX | LOCK_NB)` via `O_RDONLY` without altering `mtime`.
  - Linux `/proc/*/fd` open file descriptor `(st_dev, st_ino)` scan with non-blocking diagnostic logging for unprivileged `EACCES`/`EPERM`.
  - Windows kernel-enforced `OpenOptionsExt::share_mode(0)` probe.
  - Minimum idle age cooldown (`min_idle_age`, including future-skew protection) and batched two-point `(size, mtime)` stability verification (`stability_window`).

- **Shell-Injection-Free Pre-Delete Lifecycle Hooks (`pre_delete_hook`)**
  - Direct process execution via `Command::new` (`argv`, never `sh -c` or `cmd /C`) with token interpolation (`{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`) and `FILECLEANER_*` environment variables.
  - Configurable wall-clock `timeout` with process termination/reaping, graceful per-file skip exit code (`skip_exit_code = 42` default), and failure policies (`abort_file`, `abort_rule`, `warn_and_continue`).

- **Atomic Lifecycle Pipeline & Pure-Rust Compression (`actions`)**
  - Sequential `WorkingTarget` composition across `rename_by_date`, `compress`, `move`, `copy_truncate` (explicit opt-in with validation advisory), `trash`, and `delete`.
  - 100% pure-Rust compression codecs (`gzip` via `flate2`/`miniz_oxide`, `lz4` via `lz4_flex`, `brotli`, `zip` pure-Rust `deflate`) preserving original `atime`/`mtime` via Rust stdlib `std::fs::FileTimes` (`File::set_times`).
  - Crash-safe `.fc-tmp` staging + `sync_all()` + atomic `rename`, plus cross-filesystem (`EXDEV`) dual-read SHA-256 integrity verification before source unlink.
  - Strict `plan` / `--dry-run` parity with zero filesystem mutations or `.fc-tmp` artifacts.

- **CLI, Daemon Mode & Structured Audit Logging**
  - Complete CLI (`clap` v4): `run` (alias `clean`), `plan` (supporting both `--config` and ad-hoc `--dir` mode), `check-in-use <PATH...>`, `validate [PATH]`, `init` (`--profile backups|logs|spool`), and `daemon {run, status, stop, service-template}` with deterministic exit codes (`0`, `1`, `2`, `3`, `4`, `10`) and `--json` support.
  - Daemon scheduling across `interval`, `notify` (debounced filesystem watcher), and `hybrid` modes, atomic `pid_file` and live JSON `state_file`, cooperative `stop_file` + `SIGINT`/`SIGTERM` mid-file-safe shutdown, bounded `--max-sweeps`, and OS service templates for `systemd`, `launchd`, and Windows Task Scheduler.
  - Synchronous UTF-8 JSONL structured audit logging (`[audit].log_path` / `--audit-log`).

- **9-Target Cross-Platform Portability & Bilingual Documentation**
  - Zero C/C++ `-sys` dependencies, supporting Linux (`x86_64`/`aarch64` `gnu` & `musl`, plus `armv7-unknown-linux-gnueabihf`), Windows (`x86_64`/`aarch64` MSVC), and macOS (`aarch64`/`x86_64` Darwin), verified with **`94` passing tests** (`57` unit + `37` integration across `8` suites), **`0` Clippy warnings**, and **`92.67%` line coverage** across `8,075` lines (`cargo llvm-cov --fail-under-lines 90`).
  - Synchronized bilingual English & French documentation: `README.md` / `README.fr.md`, `docs/en/SPECIFICATIONS.md` / `docs/fr/SPECIFICATIONS.md`, `docs/en/USER_GUIDE.md` / `docs/fr/GUIDE_UTILISATEUR.md`, `docs/en/COOKBOOK_EXAMPLES.md` / `docs/fr/EXEMPLES_RECETTES.md`, custom SVG/PNG branding (`docs/assets/*`), and engineering governance registers (`docs/ACTION_LOGBOOK.md`, `docs/RCA_REGISTER.md`).
