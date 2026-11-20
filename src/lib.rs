//! FileCleaner core library: deterministic directory retention, backup safety
//! guardrails, multi-probe in-use detection, lifecycle hooks, transforms, and daemon control.

pub mod config;
pub mod error;
pub mod in_use;
pub mod policy;
pub mod scanner;

pub use config::{
    parse_bytes, parse_duration_str, ActionStep, AuditConfig, BackupGuardConfig, CombineMode,
    CompressionFormat, Config, DaemonConfig, DaemonMode, EvictionOrder, HookConfig,
    HookFailurePolicy, InUseConfig, RuleConfig, TimeBasis,
};
pub use error::{FileCleanerError, Result};
pub use in_use::{
    InUseBatchResult, InUseDetector, InUseReason, InUseReport, LockProbe, OsLockProbe,
    ProcFdProbeOutcome,
};
pub use policy::{CandidateDecision, DecisionKind, RetentionEngine, RuleOutcome, SweepPlan};
pub use scanner::{Clock, FixedClock, ScanReport, ScannedFile, Scanner, SystemClock};
