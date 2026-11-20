//! FileCleaner core library: deterministic directory retention, backup safety
//! guardrails, multi-probe in-use detection, lifecycle hooks, transforms, and daemon control.

pub mod actions;
pub mod audit;
pub mod config;
pub mod error;
pub mod hooks;

pub use actions::{
    compute_file_sha256, verify_file_sha256, ActionExecutor, ActionReport, WorkingTarget,
};
pub use audit::{
    format_bytes_human, AuditEntry, AuditEventKind, AuditLogger, RuleSummaryRow, SweepSummary,
};
pub use config::{
    parse_bytes, parse_duration_str, ActionStep, AuditConfig, BackupGuardConfig, CombineMode,
    CompressionFormat, Config, DaemonConfig, DaemonMode, EvictionOrder, HookConfig,
    HookFailurePolicy, InUseConfig, RuleConfig, TimeBasis,
};
pub use error::{FileCleanerError, Result};
pub use hooks::{
    evaluate_hook, expand_hook_args, format_action_summary, hook_error_disposition, HookCandidate,
    HookFailureDisposition, HookRunner, HookVerdict, ProcessHookRunner,
};
