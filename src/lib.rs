//! FileCleaner core library: deterministic directory retention, backup safety
//! guardrails, multi-probe in-use detection, lifecycle hooks, transforms, and daemon control.

pub mod actions;
pub mod audit;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod error;
pub mod hooks;
pub mod in_use;
pub mod policy;
pub mod scanner;

pub mod engine;

pub use actions::{
    compute_file_sha256, verify_file_sha256, ActionExecutor, ActionReport, WorkingTarget,
};
pub use audit::{
    format_bytes_human, AuditEntry, AuditEventKind, AuditLogger, RuleSummaryRow, SweepSummary,
};
pub use cli::{
    error_exit_code, execute_cli, run_from_env, CheckInUseArgs, Cli, Commands, CompressionArg,
    DaemonCommands, DaemonModeArg, InitArgs, InitFormat, InitProfile, ServiceTargetArg, SweepArgs,
    ValidateArgs,
};
pub use config::{
    parse_bytes, parse_duration_str, ActionStep, AuditConfig, BackupGuardConfig, CombineMode,
    CompressionFormat, Config, DaemonConfig, DaemonMode, EvictionOrder, HookConfig,
    HookFailurePolicy, InUseConfig, RuleConfig, TimeBasis,
};
pub use daemon::{
    daemon_status, daemon_stop, effective_watch_mode, generate_service_template, is_pid_alive,
    DaemonOptions, DaemonRunner, DaemonState, DaemonStatus, DaemonStatusReport, DaemonStopReport,
    ServiceTemplateTarget,
};
pub use engine::{
    compute_exit_code, RuleExecutionReport, RunOptions, SweepEngine, SweepReport,
    EXIT_CONFIG_OR_USAGE_ERROR, EXIT_DAEMON_ERROR, EXIT_IN_USE_BUSY, EXIT_OK,
    EXIT_OPERATIONAL_ERROR, EXIT_STRICT_SAFETY_GATE,
};
pub use error::{FileCleanerError, Result};
pub use hooks::{
    evaluate_hook, expand_hook_args, format_action_summary, hook_error_disposition, HookCandidate,
    HookFailureDisposition, HookRunner, HookVerdict, ProcessHookRunner,
};
pub use in_use::{
    InUseBatchResult, InUseDetector, InUseReason, InUseReport, LockProbe, OsLockProbe,
    ProcFdProbeOutcome,
};
pub use policy::{CandidateDecision, DecisionKind, RetentionEngine, RuleOutcome, SweepPlan};
pub use scanner::{Clock, FixedClock, ScanReport, ScannedFile, Scanner, SystemClock};
