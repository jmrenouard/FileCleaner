//! Command-line interface definitions and deterministic subcommand dispatch (`clap` v4 derive).

use crate::config::{
    parse_bytes, parse_duration_str, ActionStep, AuditConfig, BackupGuardConfig, CombineMode,
    CompressionFormat, Config, DaemonConfig, DaemonMode, EvictionOrder, HookConfig,
    HookFailurePolicy, InUseConfig, RuleConfig, TimeBasis,
};
use crate::daemon::{
    daemon_status, daemon_stop, generate_service_template, write_atomic_bytes_with_mode,
    DaemonOptions, DaemonRunner, ServiceTemplateTarget,
};
use crate::engine::{
    RunOptions, SweepEngine, EXIT_CONFIG_OR_USAGE_ERROR, EXIT_DAEMON_ERROR, EXIT_IN_USE_BUSY,
    EXIT_OK, EXIT_OPERATIONAL_ERROR,
};
use crate::error::{FileCleanerError, Result};
use crate::in_use::{InUseDetector, InUseReport};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Top-level FileCleaner CLI entrypoint.
#[derive(Debug, Parser)]
#[command(
    name = "filecleaner",
    version,
    about = "Deterministic Rust directory retention, backup safety guardrails, and lifecycle cleaner"
)]
pub struct Cli {
    /// Path to a TOML or YAML configuration file.
    #[arg(short = 'c', long = "config", global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Simulate sweeps without mutating filesystems inside monitored paths.
    #[arg(long = "dry-run", global = true)]
    pub dry_run: bool,

    /// Emit structured JSON output on stdout.
    #[arg(long = "json", global = true)]
    pub json: bool,

    /// Emit detailed per-file decision diagnostics.
    #[arg(short = 'v', long = "verbose", global = true)]
    pub verbose: bool,

    /// Suppress non-error informational output (ignored when `--json` is set).
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    /// Override structured JSONL audit log destination path.
    #[arg(long = "audit-log", global = true, value_name = "PATH")]
    pub audit_log: Option<PathBuf>,

    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Commands,
}

/// Supported FileCleaner subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Execute a single synchronous 6-phase retention sweep.
    #[command(visible_alias = "clean")]
    Run(SweepArgs),

    /// Compute and display a strictly read-only sweep plan (`dry_run = true`).
    Plan(SweepArgs),

    /// Run the 6-layer multi-probe in-use detector on one or more candidate paths.
    #[command(name = "check-in-use")]
    CheckInUse(CheckInUseArgs),

    /// Validate a TOML or YAML configuration file against all schema and safety invariants.
    Validate(ValidateArgs),

    /// Generate an annotated, validated starter configuration file.
    Init(InitArgs),

    /// Manage and inspect foreground daemon mode and OS service templates.
    Daemon {
        /// Daemon subcommand action.
        #[command(subcommand)]
        action: DaemonCommands,
    },
}

/// Arguments shared by `run` (`clean`) and `plan`.
#[derive(Debug, Clone, Args, Default)]
pub struct SweepArgs {
    /// Restrict sweep execution to specific named rule(s) from `--config`.
    #[arg(long = "rule", value_name = "NAME")]
    pub rule: Vec<String>,

    /// Return exit code `3` if any rule triggers a `BackupGuard` staleness or empty-valid freeze.
    #[arg(long = "fail-on-freeze")]
    pub fail_on_freeze: bool,

    /// Return exit code `3` if safety floors or in-use locks prevent satisfying retention caps.
    #[arg(long = "strict-constraints")]
    pub strict_constraints: bool,

    /// Ad-hoc target directory to sweep without a configuration file.
    #[arg(long = "dir", value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Ad-hoc include glob pattern (repeatable).
    #[arg(long = "include", value_name = "GLOB")]
    pub include: Vec<String>,

    /// Ad-hoc exclude glob pattern (repeatable).
    #[arg(long = "exclude", value_name = "GLOB")]
    pub exclude: Vec<String>,

    /// Traverse subdirectories recursively in ad-hoc mode.
    #[arg(long = "recursive")]
    pub recursive: bool,

    /// Maximum number of retained files in ad-hoc mode.
    #[arg(long = "max-files", value_name = "N")]
    pub max_files: Option<usize>,

    /// Maximum age before eviction in ad-hoc mode (e.g., `"30d"`, `"12h"`).
    #[arg(long = "max-age", value_name = "DURATION")]
    pub max_age: Option<String>,

    /// Maximum cumulative byte budget in ad-hoc mode (e.g., `"10GiB"`, `"500MB"`).
    #[arg(long = "max-size", value_name = "BYTES")]
    pub max_size: Option<String>,

    /// Minimum number of newest valid backups protected unconditionally in ad-hoc mode.
    #[arg(long = "min-keep", value_name = "N")]
    pub min_keep: Option<usize>,

    /// Minimum byte size for a file to count as a valid backup in ad-hoc mode.
    #[arg(long = "min-valid-backup-bytes", value_name = "BYTES")]
    pub min_valid_backup_bytes: Option<String>,

    /// Dead-man's-switch freshness window requiring a newer valid backup in ad-hoc mode.
    #[arg(long = "require-newer-within", value_name = "DURATION")]
    pub require_newer_within: Option<String>,

    /// Absolute UTC staleness cutoff timestamp (`RFC 3339` or `YYYY-MM-DD`) in ad-hoc mode.
    #[arg(long = "freeze-before-date", value_name = "TIMESTAMP")]
    pub freeze_before_date: Option<String>,

    /// Minimum idle age since `mtime` before touching a candidate in ad-hoc mode.
    #[arg(long = "min-idle-age", value_name = "DURATION")]
    pub min_idle_age: Option<String>,

    /// Prepend `RenameByDate` step in ad-hoc action pipeline.
    #[arg(long = "rename-by-date")]
    pub rename_by_date: bool,

    /// Optional `strftime` template for `RenameByDate` in ad-hoc mode.
    #[arg(long = "rename-template", value_name = "STRFTIME")]
    pub rename_template: Option<String>,

    /// Compress evicted candidates using pure-Rust codec in ad-hoc mode.
    #[arg(long = "compress", value_enum, value_name = "CODEC")]
    pub compress: Option<CompressionArg>,

    /// Move evicted candidates to destination directory in ad-hoc mode.
    #[arg(long = "move-to", value_name = "DIR")]
    pub move_to: Option<PathBuf>,

    /// Quarantine evicted candidates into trash directory in ad-hoc mode.
    #[arg(long = "trash-dir", value_name = "DIR")]
    pub trash_dir: Option<PathBuf>,

    /// Synchronous pre-delete hook executable path/name in ad-hoc mode.
    #[arg(long = "pre-hook", value_name = "COMMAND")]
    pub pre_hook: Option<String>,
}

/// Arguments for `filecleaner check-in-use <PATHS...>`.
#[derive(Debug, Clone, Args)]
pub struct CheckInUseArgs {
    /// Candidate file paths to probe.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    /// Require minimum idle age since `mtime` (e.g., `"10s"`).
    #[arg(long = "idle-age", value_name = "DURATION")]
    pub idle_age: Option<String>,

    /// Two-point `(size, mtime)` stability verification window (e.g., `"2s"`).
    #[arg(long = "stability", value_name = "DURATION")]
    pub stability: Option<String>,

    /// Override companion sidecar lock/temp suffixes (repeatable).
    #[arg(long = "sidecar-ext", value_name = "EXT")]
    pub sidecar_ext: Vec<String>,

    /// Disable Unix advisory `flock` probe.
    #[arg(long = "no-flock")]
    pub no_flock: bool,

    /// Disable Linux `/proc/*/fd` open file descriptor scan.
    #[arg(long = "no-proc-fd")]
    pub no_proc_fd: bool,

    /// Disable Windows `share_mode(0)` probe.
    #[arg(long = "no-windows-share")]
    pub no_windows_share: bool,
}

/// Arguments for `filecleaner validate [PATH]`.
#[derive(Debug, Clone, Args)]
pub struct ValidateArgs {
    /// Configuration path (`.toml`, `.yaml`, or `.yml`). May also be supplied via `--config`.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
}

/// Arguments for `filecleaner init`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Destination path for generated starter configuration.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Starter rule profile template.
    #[arg(long = "profile", value_enum, default_value_t = InitProfile::Backups)]
    pub profile: InitProfile,

    /// Output serialization format (`toml` or `yaml`).
    #[arg(long = "format", value_enum)]
    pub format: Option<InitFormat>,

    /// Overwrite an existing destination file.
    #[arg(long = "force")]
    pub force: bool,
}

/// Subcommands under `filecleaner daemon`.
#[derive(Debug, Subcommand)]
pub enum DaemonCommands {
    /// Start foreground daemon scheduling (`interval`, `notify`, or `hybrid`).
    Run {
        /// Override `daemon.mode` from configuration.
        #[arg(long = "mode", value_enum)]
        mode: Option<DaemonModeArg>,

        /// Exit cleanly after completing `N` sweep cycles.
        #[arg(long = "max-sweeps", value_name = "N")]
        max_sweeps: Option<u64>,

        /// Record backup-guard freezes during sweeps.
        #[arg(long = "fail-on-freeze")]
        fail_on_freeze: bool,

        /// Record constrained rules during sweeps.
        #[arg(long = "strict-constraints")]
        strict_constraints: bool,
    },

    /// Inspect daemon PID liveness and live JSON state metrics.
    Status {
        /// Override `daemon.pid_file` path.
        #[arg(long = "pid-file", value_name = "PATH")]
        pid_file: Option<PathBuf>,

        /// Override `daemon.state_file` path.
        #[arg(long = "state-file", value_name = "PATH")]
        state_file: Option<PathBuf>,
    },

    /// Cooperatively stop a running daemon via `stop_file` and `SIGTERM` (Unix).
    Stop {
        /// Override `daemon.pid_file` path.
        #[arg(long = "pid-file", value_name = "PATH")]
        pid_file: Option<PathBuf>,

        /// Override `daemon.stop_file` path.
        #[arg(long = "stop-file", value_name = "PATH")]
        stop_file: Option<PathBuf>,

        /// Maximum wait duration before returning exit code `4`.
        #[arg(long = "timeout", default_value = "10s", value_name = "DURATION")]
        timeout: String,
    },

    /// Emit a ready-to-install OS service definition (`systemd`, `launchd`, `windows-task`).
    #[command(name = "service-template")]
    ServiceTemplate {
        /// Target service manager (`systemd`, `launchd`, or `windows-task`).
        #[arg(long = "init-system", visible_alias = "target", value_enum)]
        init_system: ServiceTargetArg,

        /// Optional service account username (`User=` / `UserName` / `UserId`).
        #[arg(long = "user", value_name = "USER")]
        user: Option<String>,

        /// Optional file path to write the rendered service template instead of stdout.
        #[arg(short = 'o', long = "output", value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

/// Pure-Rust compression codec CLI selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompressionArg {
    Gzip,
    Lz4,
    Brotli,
    Zip,
}

impl From<CompressionArg> for CompressionFormat {
    fn from(value: CompressionArg) -> Self {
        match value {
            CompressionArg::Gzip => Self::Gzip,
            CompressionArg::Lz4 => Self::Lz4,
            CompressionArg::Brotli => Self::Brotli,
            CompressionArg::Zip => Self::Zip,
        }
    }
}

/// Starter configuration profile selector for `filecleaner init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InitProfile {
    Backups,
    Logs,
    Spool,
}

/// Starter configuration format selector for `filecleaner init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InitFormat {
    Toml,
    Yaml,
}

/// Daemon scheduling mode CLI selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DaemonModeArg {
    Interval,
    Notify,
    Hybrid,
}

impl From<DaemonModeArg> for DaemonMode {
    fn from(value: DaemonModeArg) -> Self {
        match value {
            DaemonModeArg::Interval => Self::Interval,
            DaemonModeArg::Notify => Self::Notify,
            DaemonModeArg::Hybrid => Self::Hybrid,
        }
    }
}

/// OS service template selector for `filecleaner daemon service-template`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServiceTargetArg {
    Systemd,
    Launchd,
    #[value(name = "windows-task")]
    WindowsTask,
}

impl From<ServiceTargetArg> for ServiceTemplateTarget {
    fn from(value: ServiceTargetArg) -> Self {
        match value {
            ServiceTargetArg::Systemd => Self::Systemd,
            ServiceTargetArg::Launchd => Self::Launchd,
            ServiceTargetArg::WindowsTask => Self::WindowsTask,
        }
    }
}

#[derive(Debug, Serialize)]
struct CheckInUseJsonOutput {
    all_safe: bool,
    busy_count: usize,
    reports: Vec<InUseReport>,
}

/// Parses CLI arguments from the process environment and returns the deterministic exit code.
pub fn run_from_env() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    execute_cli(cli)
}

/// Executes a parsed [`Cli`] command and returns its deterministic exit code (`0`, `1`, `2`, `3`, `4`, or `10`).
pub fn execute_cli(cli: Cli) -> i32 {
    let json = cli.json;
    let verbose = cli.verbose;
    let quiet = cli.quiet;
    let global_dry_run = cli.dry_run;
    let global_config = cli.config;
    let global_audit_log = cli.audit_log;

    match cli.command {
        Commands::Run(args) => run_sweep_cmd(
            args,
            global_config,
            global_audit_log,
            global_dry_run,
            json,
            verbose,
            quiet,
        ),
        Commands::Plan(args) => run_sweep_cmd(
            args,
            global_config,
            global_audit_log,
            true,
            json,
            verbose,
            quiet,
        ),
        Commands::CheckInUse(args) => run_check_in_use_cmd(args, json, quiet),
        Commands::Validate(args) => run_validate_cmd(args, global_config, json, quiet),
        Commands::Init(args) => run_init_cmd(args, json, quiet),
        Commands::Daemon { action } => run_daemon_cmd(
            action,
            global_config,
            global_audit_log,
            global_dry_run,
            json,
            quiet,
        ),
    }
}

fn run_sweep_cmd(
    args: SweepArgs,
    global_config: Option<PathBuf>,
    global_audit_log: Option<PathBuf>,
    dry_run: bool,
    json: bool,
    verbose: bool,
    quiet: bool,
) -> i32 {
    let config = match resolve_sweep_config(&args, global_config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => return emit_error(&err, json),
    };

    let opts = RunOptions {
        rule_filter: args.rule,
        dry_run,
        audit_log_override: global_audit_log,
        fail_on_freeze: args.fail_on_freeze,
        strict_constraints: args.strict_constraints,
        shutdown: None,
    };

    match SweepEngine::run_live(&config, &opts) {
        Ok(report) => {
            if json {
                match report.to_json_pretty() {
                    Ok(rendered) => println!("{rendered}"),
                    Err(err) => return emit_error(&err, false),
                }
            } else if !quiet {
                if verbose {
                    for rr in &report.rule_reports {
                        for dec in &rr.plan.decisions {
                            println!(
                                "[{}] {:?}: {} ({})",
                                rr.rule_name,
                                dec.decision,
                                dec.file.path.display(),
                                dec.reason
                            );
                        }
                    }
                }
                print!("{}", report.format_table());
            }
            report.exit_code
        }
        Err(err) => emit_error(&err, json),
    }
}

fn run_check_in_use_cmd(args: CheckInUseArgs, json: bool, quiet: bool) -> i32 {
    let min_idle_age = match args.idle_age.as_deref() {
        Some(raw) => match parse_duration_str(raw) {
            Ok(d) => Some(d),
            Err(err) => return emit_error(&err, json),
        },
        None => None,
    };
    let stability_window = match args.stability.as_deref() {
        Some(raw) => match parse_duration_str(raw) {
            Ok(d) => Some(d),
            Err(err) => return emit_error(&err, json),
        },
        None => None,
    };

    let mut cfg = InUseConfig {
        min_idle_age,
        stability_window,
        check_proc_fd: !args.no_proc_fd,
        ..InUseConfig::default()
    };

    #[cfg(unix)]
    {
        cfg.check_os_locks = !args.no_flock;
        let _ = args.no_windows_share;
    }
    #[cfg(windows)]
    {
        cfg.check_os_locks = !args.no_windows_share;
        let _ = args.no_flock;
    }
    #[cfg(not(any(unix, windows)))]
    {
        cfg.check_os_locks = !(args.no_flock || args.no_windows_share);
    }

    if !args.sidecar_ext.is_empty() {
        cfg.sidecar_suffixes = args.sidecar_ext;
    }

    let reports = InUseDetector::check_paths(&args.paths, &cfg);
    let busy_count = reports.iter().filter(|r| r.is_busy).count();
    let all_safe = busy_count == 0;

    if json {
        let payload = CheckInUseJsonOutput {
            all_safe,
            busy_count,
            reports,
        };
        match serde_json::to_string_pretty(&payload) {
            Ok(out) => println!("{out}"),
            Err(err) => {
                return emit_error(&FileCleanerError::Serialization(err.to_string()), false)
            }
        }
    } else if !quiet {
        for report in &reports {
            if report.is_busy {
                let joined = report
                    .reasons
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ");
                println!("BUSY: {} ({joined})", report.path.display());
            } else {
                println!("SAFE: {}", report.path.display());
            }
        }
    }

    if all_safe {
        EXIT_OK
    } else {
        EXIT_IN_USE_BUSY
    }
}

fn run_validate_cmd(
    args: ValidateArgs,
    global_config: Option<PathBuf>,
    json: bool,
    quiet: bool,
) -> i32 {
    let config_path = match args.path.or(global_config) {
        Some(path) => path,
        None => {
            let err = FileCleanerError::ConfigValidation(
                "validate requires <PATH> or --config <PATH>".to_string(),
            );
            return emit_error(&err, json);
        }
    };

    match Config::load_validated(&config_path) {
        Ok(loaded) => {
            let copy_truncate_rules: Vec<&str> = loaded
                .rules
                .iter()
                .filter(|r| r.actions.contains(&ActionStep::CopyTruncate))
                .map(|r| r.name.as_str())
                .collect();

            if json {
                let payload = serde_json::json!({
                    "valid": true,
                    "path": config_path,
                    "rules": loaded.rules.len(),
                    "rule_names": loaded.rules.iter().map(|r| &r.name).collect::<Vec<_>>(),
                    "warnings": copy_truncate_rules.iter().map(|name| format!(
                        "rule '{name}' uses copy_truncate: bytes appended between copy and truncate cannot be recovered"
                    )).collect::<Vec<_>>(),
                });
                match serde_json::to_string_pretty(&payload) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(err) => {
                        return emit_error(&FileCleanerError::Serialization(err.to_string()), false)
                    }
                }
            } else if !quiet {
                println!(
                    "configuration valid: {} rule(s) loaded from {}",
                    loaded.rules.len(),
                    config_path.display()
                );
                for name in copy_truncate_rules {
                    eprintln!(
                        "warning: rule '{name}' enables copy_truncate; interstitial bytes written between copy and truncate cannot be recovered"
                    );
                }
            }
            EXIT_OK
        }
        Err(err) => {
            if json {
                let payload = serde_json::json!({
                    "valid": false,
                    "path": config_path,
                    "error": err.to_string(),
                });
                if let Ok(rendered) = serde_json::to_string_pretty(&payload) {
                    println!("{rendered}");
                }
                error_exit_code(&err)
            } else {
                emit_error(&err, false)
            }
        }
    }
}

fn run_init_cmd(args: InitArgs, json: bool, quiet: bool) -> i32 {
    let format = resolve_init_format(args.format, args.output.as_deref());
    let output_path = args.output.unwrap_or_else(|| match format {
        InitFormat::Toml => PathBuf::from("filecleaner.toml"),
        InitFormat::Yaml => PathBuf::from("filecleaner.yaml"),
    });

    if output_path.exists() && !args.force {
        let err = FileCleanerError::io(
            &output_path,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "output configuration already exists (pass --force to overwrite)",
            ),
        );
        return emit_error(&err, json);
    }

    let template = render_starter_config(args.profile, format);
    if let Err(err) = validate_rendered_template(&output_path, format, &template) {
        return emit_error(&err, json);
    }

    if let Err(err) = write_atomic_bytes_with_mode(&output_path, template.as_bytes(), 0o600) {
        return emit_error(&err, json);
    }

    if json {
        let payload = serde_json::json!({
            "created": output_path,
            "profile": format!("{:?}", args.profile).to_ascii_lowercase(),
            "format": match format {
                InitFormat::Toml => "toml",
                InitFormat::Yaml => "yaml",
            },
            "dry_run": true,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(rendered) => println!("{rendered}"),
            Err(err) => {
                return emit_error(&FileCleanerError::Serialization(err.to_string()), false)
            }
        }
    } else if !quiet {
        println!(
            "initialized {} starter configuration at {} (dry_run = true)",
            match args.profile {
                InitProfile::Backups => "backups",
                InitProfile::Logs => "logs",
                InitProfile::Spool => "spool",
            },
            output_path.display()
        );
    }

    EXIT_OK
}

fn run_daemon_cmd(
    action: DaemonCommands,
    global_config: Option<PathBuf>,
    global_audit_log: Option<PathBuf>,
    global_dry_run: bool,
    json: bool,
    quiet: bool,
) -> i32 {
    match action {
        DaemonCommands::Run {
            mode,
            max_sweeps,
            fail_on_freeze,
            strict_constraints,
        } => {
            let Some(cfg_path) = global_config else {
                return emit_error(
                    &FileCleanerError::ConfigValidation(
                        "daemon run requires --config <PATH>".to_string(),
                    ),
                    json,
                );
            };
            let config = match Config::load_validated(&cfg_path) {
                Ok(cfg) => cfg,
                Err(err) => return emit_error(&err, json),
            };
            let opts = DaemonOptions {
                mode_override: mode.map(Into::into),
                max_sweeps,
                dry_run: global_dry_run,
                audit_log_override: global_audit_log,
                fail_on_freeze,
                strict_constraints,
                shutdown_signal: None,
                install_signal_handler: true,
            };
            match DaemonRunner::run(&config, &opts) {
                Ok(final_state) => {
                    if json {
                        if let Ok(rendered) = serde_json::to_string_pretty(&final_state) {
                            println!("{rendered}");
                        }
                    } else if !quiet {
                        println!(
                            "daemon stopped cleanly after {} sweep(s) ({} files processed)",
                            final_state.sweeps_completed, final_state.total_files_processed
                        );
                    }
                    EXIT_OK
                }
                Err(err) => emit_error(&err, json),
            }
        }
        DaemonCommands::Status {
            pid_file,
            state_file,
        } => {
            let (resolved_pid, resolved_state) =
                match resolve_status_paths(global_config.as_deref(), pid_file, state_file) {
                    Ok(pair) => pair,
                    Err(err) => return emit_error(&err, json),
                };

            match daemon_status(resolved_pid.as_deref(), resolved_state.as_deref()) {
                Ok(report) => {
                    if json {
                        match serde_json::to_string_pretty(&report) {
                            Ok(rendered) => println!("{rendered}"),
                            Err(err) => {
                                return emit_error(
                                    &FileCleanerError::Serialization(err.to_string()),
                                    false,
                                )
                            }
                        }
                    } else if !quiet {
                        if report.running {
                            println!(
                                "daemon running (pid={})",
                                report
                                    .pid
                                    .map_or_else(|| "?".to_string(), |p| p.to_string())
                            );
                        } else if report.stale_pid {
                            println!(
                                "daemon stale (dead pid={})",
                                report
                                    .pid
                                    .map_or_else(|| "?".to_string(), |p| p.to_string())
                            );
                        } else {
                            println!("daemon stopped");
                        }
                    }
                    if report.running {
                        EXIT_OK
                    } else {
                        EXIT_DAEMON_ERROR
                    }
                }
                Err(err) => emit_error(&err, json),
            }
        }
        DaemonCommands::Stop {
            pid_file,
            stop_file,
            timeout,
        } => {
            let timeout_dur = match parse_duration_str(&timeout) {
                Ok(d) => d,
                Err(err) => return emit_error(&err, json),
            };
            let (resolved_pid, resolved_stop) =
                match resolve_stop_paths(global_config.as_deref(), pid_file, stop_file) {
                    Ok(pair) => pair,
                    Err(err) => return emit_error(&err, json),
                };

            match daemon_stop(
                resolved_pid.as_deref(),
                resolved_stop.as_deref(),
                timeout_dur,
            ) {
                Ok(report) => {
                    if json {
                        match serde_json::to_string_pretty(&report) {
                            Ok(rendered) => println!("{rendered}"),
                            Err(err) => {
                                return emit_error(
                                    &FileCleanerError::Serialization(err.to_string()),
                                    false,
                                )
                            }
                        }
                    } else if !quiet {
                        if report.was_running {
                            println!("daemon stopped cleanly");
                        } else {
                            println!("daemon is not running");
                        }
                    }
                    EXIT_OK
                }
                Err(err) => emit_error(&err, json),
            }
        }
        DaemonCommands::ServiceTemplate {
            init_system,
            user,
            output,
        } => {
            let cfg_path =
                global_config.unwrap_or_else(|| PathBuf::from("/etc/filecleaner/filecleaner.toml"));
            let rendered =
                generate_service_template(init_system.into(), &cfg_path, user.as_deref());
            if let Some(out_path) = output {
                if let Err(err) =
                    write_atomic_bytes_with_mode(&out_path, rendered.as_bytes(), 0o644)
                {
                    return emit_error(&err, json);
                }
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "written": out_path,
                            "init_system": format!("{init_system:?}").to_ascii_lowercase(),
                        })
                    );
                } else if !quiet {
                    println!("wrote service template to {}", out_path.display());
                }
            } else if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "init_system": format!("{init_system:?}").to_ascii_lowercase(),
                        "config_path": cfg_path,
                        "template": rendered,
                    })
                );
            } else {
                print!("{rendered}");
            }
            EXIT_OK
        }
    }
}

fn resolve_status_paths(
    config_path: Option<&Path>,
    pid_override: Option<PathBuf>,
    state_override: Option<PathBuf>,
) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    if let Some(cfg_path) = config_path {
        let cfg = Config::load_validated(cfg_path)?;
        Ok((
            pid_override.or(cfg.daemon.pid_file),
            state_override.or(cfg.daemon.state_file),
        ))
    } else {
        Ok((pid_override, state_override))
    }
}

fn resolve_stop_paths(
    config_path: Option<&Path>,
    pid_override: Option<PathBuf>,
    stop_override: Option<PathBuf>,
) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    if let Some(cfg_path) = config_path {
        let cfg = Config::load_validated(cfg_path)?;
        Ok((
            pid_override.or(cfg.daemon.pid_file),
            stop_override.or(cfg.daemon.stop_file),
        ))
    } else {
        Ok((pid_override, stop_override))
    }
}

fn resolve_sweep_config(args: &SweepArgs, global_config: Option<&Path>) -> Result<Config> {
    match (global_config, &args.dir) {
        (Some(_), Some(_)) => Err(FileCleanerError::ConfigValidation(
            "cannot combine --config with ad-hoc --dir".to_string(),
        )),
        (None, None) => Err(FileCleanerError::ConfigValidation(
            "either --config <PATH> or ad-hoc --dir <PATH> is required".to_string(),
        )),
        (Some(path), None) => Config::load_validated(path),
        (None, Some(dir)) => build_adhoc_config(args, dir),
    }
}

fn build_adhoc_config(args: &SweepArgs, dir: &Path) -> Result<Config> {
    if args.move_to.is_some() && args.trash_dir.is_some() {
        return Err(FileCleanerError::ConfigValidation(
            "cannot specify both --move-to and --trash-dir in ad-hoc mode".to_string(),
        ));
    }

    let max_age = match args.max_age.as_deref() {
        Some(raw) => Some(parse_duration_str(raw)?),
        None => None,
    };
    let max_total_size = match args.max_size.as_deref() {
        Some(raw) => Some(parse_bytes(raw)?),
        None => None,
    };
    let min_valid_backup_bytes = match args.min_valid_backup_bytes.as_deref() {
        Some(raw) => parse_bytes(raw)?,
        None => 0,
    };
    let require_newer_within = match args.require_newer_within.as_deref() {
        Some(raw) => Some(parse_duration_str(raw)?),
        None => None,
    };
    let freeze_if_newest_older_than = match args.freeze_before_date.as_deref() {
        Some(raw) => Some(parse_utc_timestamp(raw)?),
        None => None,
    };
    let min_idle_age = match args.min_idle_age.as_deref() {
        Some(raw) => Some(parse_duration_str(raw)?),
        None => None,
    };

    let guard_enabled = args.min_keep.is_some()
        || args.min_valid_backup_bytes.is_some()
        || require_newer_within.is_some()
        || freeze_if_newest_older_than.is_some();

    let mut actions = Vec::new();
    if args.rename_by_date {
        actions.push(ActionStep::RenameByDate);
    }
    if args.compress.is_some() {
        actions.push(ActionStep::Compress);
    }
    if args.move_to.is_some() {
        actions.push(ActionStep::Move);
    } else if args.trash_dir.is_some() {
        actions.push(ActionStep::Trash);
    }
    if actions.is_empty() {
        actions.push(ActionStep::Delete);
    }

    let pre_delete_hook = args.pre_hook.as_ref().map(|cmd| HookConfig {
        command: cmd.clone(),
        args: Vec::new(),
        timeout: Duration::from_secs(30),
        on_failure: HookFailurePolicy::AbortFile,
        skip_exit_code: Some(42),
        run_in_dry_run: false,
        working_dir: None,
        env: BTreeMap::new(),
    });

    let rule = RuleConfig {
        name: "adhoc".to_string(),
        path: dir.to_path_buf(),
        enabled: true,
        recursive: args.recursive,
        max_depth: None,
        follow_symlinks: false,
        include_globs: args.include.clone(),
        exclude_globs: args.exclude.clone(),
        include_regex: None,
        exclude_regex: None,
        min_file_size: None,
        max_file_size: None,
        time_basis: TimeBasis::Modified,
        filename_date_regex: None,
        filename_date_format: None,
        max_files: args.max_files,
        max_age,
        max_total_size,
        combine_mode: CombineMode::Any,
        eviction_order: EvictionOrder::OldestFirst,
        dry_run: false,
        backup_guard: BackupGuardConfig {
            enabled: guard_enabled,
            min_keep_files: args.min_keep.unwrap_or(0),
            min_valid_backup_bytes,
            require_newer_within,
            freeze_if_newest_older_than,
            freeze_on_empty_valid: guard_enabled,
        },
        in_use: InUseConfig {
            min_idle_age,
            ..InUseConfig::default()
        },
        pre_delete_hook,
        actions,
        move_to: args.move_to.clone(),
        move_date_subdir: None,
        move_overwrite: false,
        trash_dir: args.trash_dir.clone(),
        compression: args.compress.map(Into::into).unwrap_or_default(),
        compression_level: None,
        compress_remove_source: true,
        compress_overwrite: false,
        copy_truncate_to: None,
        rename_template: args.rename_template.clone(),
    };

    let cfg = Config {
        dry_run: false,
        audit_log: None,
        daemon: DaemonConfig::default(),
        audit: AuditConfig::default(),
        global_trash_dir: None,
        rules: vec![rule],
    };
    cfg.validate()?;
    Ok(cfg)
}

fn parse_utc_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    let trimmed = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Ok(ndt.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        if let Some(ndt) = date.and_hms_opt(0, 0, 0) {
            return Ok(ndt.and_utc());
        }
    }
    Err(FileCleanerError::ConfigValidation(format!(
        "invalid UTC timestamp '{raw}' (expected RFC 3339 or YYYY-MM-DD)"
    )))
}

fn resolve_init_format(explicit: Option<InitFormat>, output: Option<&Path>) -> InitFormat {
    if let Some(fmt) = explicit {
        return fmt;
    }
    if let Some(ext) = output
        .and_then(|p| p.extension())
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        if ext == "yaml" || ext == "yml" {
            return InitFormat::Yaml;
        }
    }
    InitFormat::Toml
}

fn validate_rendered_template(output_path: &Path, format: InitFormat, raw: &str) -> Result<()> {
    let parsed: Config = match format {
        InitFormat::Toml => toml::from_str(raw).map_err(|err| FileCleanerError::ConfigParse {
            path: output_path.to_path_buf(),
            reason: err.to_string(),
        })?,
        InitFormat::Yaml => {
            serde_yaml_ng::from_str(raw).map_err(|err| FileCleanerError::ConfigParse {
                path: output_path.to_path_buf(),
                reason: err.to_string(),
            })?
        }
    };
    parsed.validate()
}

fn render_starter_config(profile: InitProfile, format: InitFormat) -> String {
    match (profile, format) {
        (InitProfile::Backups, InitFormat::Toml) => {
            r#"# FileCleaner Starter Configuration — Backups Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run = true
global_trash_dir = "/var/tmp/filecleaner-trash"

[daemon]
mode = "hybrid"
interval = "5m"
watch = true
debounce = "2s"
sweep_on_start = true
pid_file = "/run/filecleaner/filecleaner.pid"
state_file = "/run/filecleaner/state.json"
stop_file = "/run/filecleaner/filecleaner.stop"

[audit]
log_path = "/var/log/filecleaner/audit.jsonl"
jsonl = true

[[rules]]
name = "nightly-backups"
path = "/var/backups/app"
enabled = true
recursive = true
max_depth = 2
follow_symlinks = false
include_globs = ["*.sql", "*.sql.gz", "*.dump"]
exclude_globs = ["pinned/**"]
min_file_size = "1KiB"
time_basis = "filename_date"
filename_date_regex = 'backup_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_age = "30d"
max_total_size = "50GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
compress_overwrite = false
move_to = "/var/archive/app-backups"
move_date_subdir = "%Y/%m"
move_overwrite = false

[rules.backup_guard]
enabled = true
min_keep_files = 3
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "10s"
stability_window = "2s"
"#
            .to_string()
        }
        (InitProfile::Backups, InitFormat::Yaml) => {
            r#"# FileCleaner Starter Configuration — Backups Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run: true
global_trash_dir: /var/tmp/filecleaner-trash

daemon:
  mode: hybrid
  interval: 5m
  watch: true
  debounce: 2s
  sweep_on_start: true
  pid_file: /run/filecleaner/filecleaner.pid
  state_file: /run/filecleaner/state.json
  stop_file: /run/filecleaner/filecleaner.stop

audit:
  log_path: /var/log/filecleaner/audit.jsonl
  jsonl: true

rules:
  - name: nightly-backups
    path: /var/backups/app
    enabled: true
    recursive: true
    max_depth: 2
    follow_symlinks: false
    include_globs:
      - "*.sql"
      - "*.sql.gz"
      - "*.dump"
    exclude_globs:
      - "pinned/**"
    min_file_size: 1KiB
    time_basis: filename_date
    filename_date_regex: 'backup_(\d{8}_\d{6})'
    filename_date_format: "%Y%m%d_%H%M%S"
    max_files: 14
    max_age: 30d
    max_total_size: 50GiB
    combine_mode: any
    eviction_order: oldest_first
    actions:
      - compress
      - move
    compression: gzip
    compress_remove_source: true
    compress_overwrite: false
    move_to: /var/archive/app-backups
    move_date_subdir: "%Y/%m"
    move_overwrite: false
    backup_guard:
      enabled: true
      min_keep_files: 3
      min_valid_backup_bytes: 1MiB
      require_newer_within: 36h
      freeze_on_empty_valid: true
    in_use:
      enabled: true
      check_sidecar_locks: true
      check_os_locks: true
      check_proc_fd: true
      min_idle_age: 10s
      stability_window: 2s
"#
            .to_string()
        }
        (InitProfile::Logs, InitFormat::Toml) => {
            r#"# FileCleaner Starter Configuration — Logs Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run = true
global_trash_dir = "/var/tmp/filecleaner-trash"

[daemon]
mode = "interval"
interval = "10m"
watch = false
sweep_on_start = true

[audit]
log_path = "/var/log/filecleaner/logs-audit.jsonl"
jsonl = true

[[rules]]
name = "rotated-app-logs"
path = "/var/log/myapp"
enabled = true
recursive = false
follow_symlinks = false
include_globs = ["*.log"]
exclude_globs = ["*.keep"]
time_basis = "modified"
max_files = 30
max_age = "14d"
max_total_size = "2GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["rename_by_date", "compress", "trash"]
rename_template = "%Y%m%d_%H%M%S"
compression = "gzip"
compress_remove_source = true
compress_overwrite = false
trash_dir = "/var/tmp/filecleaner-trash/myapp"

[rules.backup_guard]
enabled = true
min_keep_files = 3
min_valid_backup_bytes = "0B"
freeze_on_empty_valid = false

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "10s"
stability_window = "2s"
"#
            .to_string()
        }
        (InitProfile::Logs, InitFormat::Yaml) => {
            r#"# FileCleaner Starter Configuration — Logs Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run: true
global_trash_dir: /var/tmp/filecleaner-trash

daemon:
  mode: interval
  interval: 10m
  watch: false
  sweep_on_start: true

audit:
  log_path: /var/log/filecleaner/logs-audit.jsonl
  jsonl: true

rules:
  - name: rotated-app-logs
    path: /var/log/myapp
    enabled: true
    recursive: false
    follow_symlinks: false
    include_globs:
      - "*.log"
    exclude_globs:
      - "*.keep"
    time_basis: modified
    max_files: 30
    max_age: 14d
    max_total_size: 2GiB
    combine_mode: any
    eviction_order: oldest_first
    actions:
      - rename_by_date
      - compress
      - trash
    rename_template: "%Y%m%d_%H%M%S"
    compression: gzip
    compress_remove_source: true
    compress_overwrite: false
    trash_dir: /var/tmp/filecleaner-trash/myapp
    backup_guard:
      enabled: true
      min_keep_files: 3
      min_valid_backup_bytes: 0B
      freeze_on_empty_valid: false
    in_use:
      enabled: true
      check_sidecar_locks: true
      check_os_locks: true
      check_proc_fd: true
      min_idle_age: 10s
      stability_window: 2s
"#
            .to_string()
        }
        (InitProfile::Spool, InitFormat::Toml) => {
            r#"# FileCleaner Starter Configuration — Spool Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run = true
global_trash_dir = "/var/tmp/filecleaner-trash"

[daemon]
mode = "hybrid"
interval = "2m"
watch = true
debounce = "1s"
sweep_on_start = true

[audit]
log_path = "/var/log/filecleaner/spool-audit.jsonl"
jsonl = true

[[rules]]
name = "incoming-spool"
path = "/var/spool/incoming"
enabled = true
recursive = true
max_depth = 3
follow_symlinks = false
include_globs = ["*.csv", "*.parquet"]
exclude_globs = ["quarantine/**", "*.keep"]
min_file_size = "1B"
time_basis = "modified"
max_files = 100
max_age = "12h"
max_total_size = "10GiB"
combine_mode = "any"
eviction_order = "largest_first"
actions = ["compress", "move"]
compression = "lz4"
compress_remove_source = true
compress_overwrite = false
move_to = "/var/spool/archived"
move_date_subdir = "%Y/%m/%d"
move_overwrite = false

[rules.backup_guard]
enabled = true
min_keep_files = 3
min_valid_backup_bytes = "1B"
freeze_on_empty_valid = false

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "30s"
stability_window = "2s"
"#
            .to_string()
        }
        (InitProfile::Spool, InitFormat::Yaml) => {
            r#"# FileCleaner Starter Configuration — Spool Profile
# Safety first: dry_run starts as `true`; review `filecleaner plan` before enabling live sweeps.

dry_run: true
global_trash_dir: /var/tmp/filecleaner-trash

daemon:
  mode: hybrid
  interval: 2m
  watch: true
  debounce: 1s
  sweep_on_start: true

audit:
  log_path: /var/log/filecleaner/spool-audit.jsonl
  jsonl: true

rules:
  - name: incoming-spool
    path: /var/spool/incoming
    enabled: true
    recursive: true
    max_depth: 3
    follow_symlinks: false
    include_globs:
      - "*.csv"
      - "*.parquet"
    exclude_globs:
      - "quarantine/**"
      - "*.keep"
    min_file_size: 1B
    time_basis: modified
    max_files: 100
    max_age: 12h
    max_total_size: 10GiB
    combine_mode: any
    eviction_order: largest_first
    actions:
      - compress
      - move
    compression: lz4
    compress_remove_source: true
    compress_overwrite: false
    move_to: /var/spool/archived
    move_date_subdir: "%Y/%m/%d"
    move_overwrite: false
    backup_guard:
      enabled: true
      min_keep_files: 3
      min_valid_backup_bytes: 1B
      freeze_on_empty_valid: false
    in_use:
      enabled: true
      check_sidecar_locks: true
      check_os_locks: true
      check_proc_fd: true
      min_idle_age: 30s
      stability_window: 2s
"#
            .to_string()
        }
    }
}

fn emit_error(err: &FileCleanerError, json: bool) -> i32 {
    let code = error_exit_code(err);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "error": err.to_string(),
                "exit_code": code,
            })
        );
    } else {
        eprintln!("error: {err}");
    }
    code
}

/// Maps a [`FileCleanerError`] variant to FileCleaner's deterministic exit code table.
pub fn error_exit_code(err: &FileCleanerError) -> i32 {
    match err {
        FileCleanerError::ConfigParse { .. }
        | FileCleanerError::ConfigFormat { .. }
        | FileCleanerError::ConfigValidation(_)
        | FileCleanerError::InvalidGlob { .. }
        | FileCleanerError::InvalidRegex { .. }
        | FileCleanerError::InvalidDuration(_)
        | FileCleanerError::InvalidByteSize(_) => EXIT_CONFIG_OR_USAGE_ERROR,
        FileCleanerError::Daemon(_) => EXIT_DAEMON_ERROR,
        _ => EXIT_OPERATIONAL_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn adhoc_config_builder_and_timestamp_parser_cover_all_options() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("incoming");
        let dest = tmp.path().join("archive");
        fs::create_dir_all(&dir).unwrap();

        let args = SweepArgs {
            dir: Some(dir.clone()),
            include: vec!["*.sql".to_string()],
            exclude: vec!["keep/**".to_string()],
            recursive: true,
            max_files: Some(5),
            max_age: Some("10d".to_string()),
            max_size: Some("100MiB".to_string()),
            min_keep: Some(2),
            require_newer_within: Some("24h".to_string()),
            freeze_before_date: Some("2026-11-18".to_string()),
            min_idle_age: Some("30s".to_string()),
            rename_by_date: true,
            compress: Some(CompressionArg::Lz4),
            move_to: Some(dest.clone()),
            pre_hook: Some("/bin/true".to_string()),
            ..SweepArgs::default()
        };

        let cfg = resolve_sweep_config(&args, None).unwrap();
        assert_eq!(cfg.rules.len(), 1);
        let rule = &cfg.rules[0];
        assert_eq!(
            rule.actions,
            vec![
                ActionStep::RenameByDate,
                ActionStep::Compress,
                ActionStep::Move
            ]
        );
        assert_eq!(rule.compression, CompressionFormat::Lz4);
        assert_eq!(rule.backup_guard.min_keep_files, 2);
        assert!(rule.backup_guard.freeze_if_newest_older_than.is_some());
        assert_eq!(rule.in_use.min_idle_age, Some(Duration::from_secs(30)));
        assert!(rule.pre_delete_hook.is_some());

        // Invalid combinations exit with ConfigValidation.
        assert!(matches!(
            resolve_sweep_config(&SweepArgs::default(), None),
            Err(FileCleanerError::ConfigValidation(_))
        ));
        let conflict_move_trash = SweepArgs {
            dir: Some(dir),
            move_to: Some(dest),
            trash_dir: Some(tmp.path().join("trash")),
            ..SweepArgs::default()
        };
        assert!(matches!(
            resolve_sweep_config(&conflict_move_trash, None),
            Err(FileCleanerError::ConfigValidation(_))
        ));
        assert!(parse_utc_timestamp("2026-11-20T09:05:00Z").is_ok());
        assert!(parse_utc_timestamp("2026-11-20 09:05:00").is_ok());
        assert!(parse_utc_timestamp("not-a-date").is_err());
    }

    #[test]
    fn starter_templates_validate_for_every_profile_and_format() {
        for profile in [InitProfile::Backups, InitProfile::Logs, InitProfile::Spool] {
            for format in [InitFormat::Toml, InitFormat::Yaml] {
                let raw = render_starter_config(profile, format);
                let path = match format {
                    InitFormat::Toml => Path::new("starter.toml"),
                    InitFormat::Yaml => Path::new("starter.yaml"),
                };
                validate_rendered_template(path, format, &raw).unwrap();
            }
        }
        assert_eq!(
            resolve_init_format(None, Some(Path::new("cfg.yml"))),
            InitFormat::Yaml
        );
        assert_eq!(resolve_init_format(None, None), InitFormat::Toml);
    }

    #[test]
    fn execute_cli_covers_validate_check_in_use_init_sweep_and_daemon_branches() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("watch");
        fs::create_dir_all(&watch_dir).unwrap();
        let f_sample = watch_dir.join("a.log");
        fs::write(&f_sample, b"hello").unwrap();

        // 1. Validate missing path -> exit 2, CopyTruncate warning -> exit 0.
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Validate(ValidateArgs { path: None }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );

        let ct_cfg = tmp.path().join("ct.toml");
        let ct_dest = tmp.path().join("rotated");
        fs::write(
            &ct_cfg,
            format!(
                r#"
[[rules]]
name = "active-log"
path = "{}"
actions = ["copy_truncate"]
copy_truncate_to = "{}"
"#,
                watch_dir.display().to_string().replace('\\', "\\\\"),
                ct_dest.display().to_string().replace('\\', "\\\\"),
            ),
        )
        .unwrap();
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: true,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Validate(ValidateArgs { path: None }),
            }),
            EXIT_OK
        );
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Validate(ValidateArgs { path: None }),
            }),
            EXIT_OK
        );

        // 2. Plan & Run error branches: quiet mode, --config + --dir conflict, invalid duration/bytes, unknown --rule.
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: false,
                verbose: false,
                quiet: true,
                audit_log: None,
                command: Commands::Plan(SweepArgs::default()),
            }),
            EXIT_OK
        );
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: true,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Run(SweepArgs {
                    dir: Some(watch_dir.clone()),
                    ..SweepArgs::default()
                }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Run(SweepArgs {
                    dir: Some(watch_dir.clone()),
                    min_valid_backup_bytes: Some("not-bytes".to_string()),
                    ..SweepArgs::default()
                }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Run(SweepArgs {
                    rule: vec!["nonexistent".to_string()],
                    ..SweepArgs::default()
                }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );

        // 3. check-in-use invalid durations, quiet mode, and CompressionArg variants.
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: true,
                audit_log: None,
                command: Commands::CheckInUse(CheckInUseArgs {
                    paths: vec![f_sample.clone()],
                    idle_age: Some("bad-dur".to_string()),
                    stability: None,
                    sidecar_ext: vec![],
                    no_flock: true,
                    no_proc_fd: true,
                    no_windows_share: true,
                }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: true,
                audit_log: None,
                command: Commands::CheckInUse(CheckInUseArgs {
                    paths: vec![f_sample],
                    idle_age: None,
                    stability: Some("bad-stab".to_string()),
                    sidecar_ext: vec![],
                    no_flock: true,
                    no_proc_fd: true,
                    no_windows_share: true,
                }),
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );
        assert_eq!(
            CompressionFormat::from(CompressionArg::Gzip),
            CompressionFormat::Gzip
        );
        assert_eq!(
            CompressionFormat::from(CompressionArg::Brotli),
            CompressionFormat::Brotli
        );
        assert_eq!(
            CompressionFormat::from(CompressionArg::Zip),
            CompressionFormat::Zip
        );
        assert_eq!(DaemonMode::from(DaemonModeArg::Notify), DaemonMode::Notify);
        assert_eq!(DaemonMode::from(DaemonModeArg::Hybrid), DaemonMode::Hybrid);

        // 4. Daemon error/override branches: run without config, status/stop with explicit overrides & JSON.
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::Run {
                        mode: None,
                        max_sweeps: Some(0),
                        fail_on_freeze: false,
                        strict_constraints: false,
                    }
                },
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );

        let stale_pid = tmp.path().join("stale_cli.pid");
        fs::write(&stale_pid, "99999997\n").unwrap();
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::Status {
                        pid_file: Some(stale_pid.clone()),
                        state_file: None,
                    }
                },
            }),
            EXIT_DAEMON_ERROR
        );
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::Stop {
                        pid_file: Some(stale_pid),
                        stop_file: None,
                        timeout: "1s".to_string(),
                    }
                },
            }),
            EXIT_OK
        );
        assert_eq!(
            execute_cli(Cli {
                config: None,
                dry_run: false,
                json: false,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::Stop {
                        pid_file: None,
                        stop_file: None,
                        timeout: "bad-timeout".to_string(),
                    }
                },
            }),
            EXIT_CONFIG_OR_USAGE_ERROR
        );

        // Service template --json both stdout and --output.
        let tpl_out = tmp.path().join("tpl.service");
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg.clone()),
                dry_run: false,
                json: true,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::ServiceTemplate {
                        init_system: ServiceTargetArg::Systemd,
                        user: None,
                        output: Some(tpl_out),
                    }
                },
            }),
            EXIT_OK
        );
        assert_eq!(
            execute_cli(Cli {
                config: Some(ct_cfg),
                dry_run: false,
                json: true,
                verbose: false,
                quiet: false,
                audit_log: None,
                command: Commands::Daemon {
                    action: DaemonCommands::ServiceTemplate {
                        init_system: ServiceTargetArg::Launchd,
                        user: None,
                        output: None,
                    }
                },
            }),
            EXIT_OK
        );

        // Also validate error_exit_code for operational error.
        assert_eq!(
            error_exit_code(&FileCleanerError::Serialization("x".to_string())),
            EXIT_OPERATIONAL_ERROR
        );
    }
}
