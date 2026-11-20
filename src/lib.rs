//! FileCleaner core library: deterministic directory retention, backup safety
//! guardrails, multi-probe in-use detection, lifecycle hooks, transforms, and daemon control.

pub mod config;
pub mod error;

pub use config::{
    parse_bytes, parse_duration_str, ActionStep, AuditConfig, BackupGuardConfig, CombineMode,
    CompressionFormat, Config, DaemonConfig, DaemonMode, EvictionOrder, HookConfig,
    HookFailurePolicy, InUseConfig, RuleConfig, TimeBasis,
};
pub use error::{FileCleanerError, Result};
