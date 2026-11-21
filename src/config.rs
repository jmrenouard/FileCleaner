//! Configuration schema, human-unit parsers, and invariant validation.

use crate::error::{FileCleanerError, Result};
use chrono::format::{Item, StrftimeItems};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Minimum S3 multipart upload part size (`5 MiB`), matching S3 protocol limits.
pub const MIN_S3_MULTIPART_PART_SIZE: u64 = 5 * 1024 * 1024;
const DEFAULT_S3_MULTIPART_PART_SIZE: u64 = 16 * 1024 * 1024;

/// Top-level FileCleaner configuration loaded from TOML or YAML.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Global dry-run switch preventing all filesystem mutations when `true`.
    #[serde(default)]
    pub dry_run: bool,

    /// Optional top-level path alias for JSONL audit logging (`[audit].log_path` preferred).
    #[serde(default)]
    pub audit_log: Option<PathBuf>,

    /// Global daemon scheduling, watcher, and control file settings.
    #[serde(default)]
    pub daemon: DaemonConfig,

    /// Structured JSONL audit logging settings.
    #[serde(default)]
    pub audit: AuditConfig,

    /// Fallback quarantine directory when a rule selects `ActionStep::Trash`
    /// without specifying a rule-local `trash_dir`.
    #[serde(default)]
    pub global_trash_dir: Option<PathBuf>,

    /// Fallback S3 / S3-compatible archival sink settings when a rule selects
    /// `ActionStep::UploadS3` without specifying a rule-local `s3` block.
    #[serde(default)]
    pub s3: Option<S3Config>,

    /// Ordered list of directory retention and lifecycle rules.
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

impl Config {
    /// Returns the effective audit log path, preferring `[audit].log_path` over top-level `audit_log`.
    pub fn effective_audit_log(&self) -> Option<&Path> {
        self.audit.log_path.as_deref().or(self.audit_log.as_deref())
    }

    /// Returns the effective S3 configuration for `rule`, preferring `rule.s3` over global `self.s3`.
    pub fn effective_s3<'a>(&'a self, rule: &'a RuleConfig) -> Option<&'a S3Config> {
        rule.s3.as_ref().or(self.s3.as_ref())
    }
}

/// Execution scheduling mode for `filecleaner daemon run`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMode {
    /// Fixed-interval polling loop using `daemon.interval`.
    #[serde(alias = "Interval")]
    Interval,
    /// Event-driven filesystem watcher using `notify` with `daemon.debounce`.
    #[serde(alias = "Notify")]
    Notify,
    /// Combined event-driven watcher and fallback periodic polling interval.
    #[default]
    #[serde(alias = "Hybrid")]
    Hybrid,
}

/// Daemon mode scheduling, watcher, and lifecycle control files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonConfig {
    /// Scheduling mode (`interval`, `notify`, or `hybrid`).
    #[serde(default)]
    pub mode: DaemonMode,

    /// Periodic sweep interval (e.g. `"60s"`, `"5m"`).
    #[serde(
        default = "default_daemon_interval",
        alias = "poll_interval",
        with = "serde_duration"
    )]
    pub interval: Duration,

    /// Enable event-driven filesystem watcher triggers via `notify`.
    #[serde(default)]
    pub watch: bool,

    /// Coalescing debounce delay for filesystem watcher events.
    #[serde(default = "default_debounce", with = "serde_duration")]
    pub debounce: Duration,

    /// Execute an immediate sweep upon daemon startup before entering the wait loop.
    #[serde(default = "default_true")]
    pub sweep_on_start: bool,

    /// Optional PID file path written on daemon start and removed on shutdown.
    #[serde(default)]
    pub pid_file: Option<PathBuf>,

    /// Optional live JSON daemon state output path.
    #[serde(default)]
    pub state_file: Option<PathBuf>,

    /// Optional stop-trigger file path watched for cooperative shutdown.
    #[serde(default)]
    pub stop_file: Option<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            mode: DaemonMode::Hybrid,
            interval: default_daemon_interval(),
            watch: false,
            debounce: default_debounce(),
            sweep_on_start: true,
            pid_file: None,
            state_file: None,
            stop_file: None,
        }
    }
}

/// Structured audit logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditConfig {
    /// Optional path where JSONL audit entries are appended.
    #[serde(default)]
    pub log_path: Option<PathBuf>,

    /// Emit machine-readable JSON Lines entries.
    #[serde(default = "default_true")]
    pub jsonl: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            log_path: None,
            jsonl: true,
        }
    }
}

/// Per-directory retention, safety guardrail, hook, and action rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleConfig {
    /// Unique identifier for the rule across the configuration.
    pub name: String,

    /// Root directory monitored by this rule.
    pub path: PathBuf,

    /// Whether the rule participates in sweeps (defaults to `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Traverse subdirectories recursively when `true`.
    #[serde(default)]
    pub recursive: bool,

    /// Maximum directory recursion depth (ignored unless `recursive` is `true`).
    #[serde(default)]
    pub max_depth: Option<usize>,

    /// Follow symbolic links during traversal (defaults to `false` for safety).
    #[serde(default)]
    pub follow_symlinks: bool,

    /// Glob patterns selecting eligible files (empty selects all files).
    #[serde(default)]
    pub include_globs: Vec<String>,

    /// Glob patterns excluding files or subdirectories from selection.
    #[serde(default)]
    pub exclude_globs: Vec<String>,

    /// Optional regular expression matching candidate relative paths or names.
    #[serde(default)]
    pub include_regex: Option<String>,

    /// Optional regular expression excluding candidate relative paths or names.
    #[serde(default)]
    pub exclude_regex: Option<String>,

    /// Optional lower file-size bound for candidate selection (`min_file_size <= size <= max_file_size`).
    #[serde(default, with = "serde_opt_bytes")]
    pub min_file_size: Option<u64>,

    /// Optional upper file-size bound for candidate selection.
    #[serde(default, with = "serde_opt_bytes")]
    pub max_file_size: Option<u64>,

    /// Timestamp extraction strategy used to order and age files.
    #[serde(default)]
    pub time_basis: TimeBasis,

    /// Regular expression capturing timestamp substring when `time_basis = "filename_date"`.
    #[serde(default)]
    pub filename_date_regex: Option<String>,

    /// Chrono `strftime` format used to parse the captured filename date string.
    #[serde(default)]
    pub filename_date_format: Option<String>,

    /// Maximum number of retained files matching this rule.
    #[serde(default)]
    pub max_files: Option<usize>,

    /// Maximum age permitted before a candidate becomes eligible for eviction.
    #[serde(default, with = "serde_opt_duration")]
    pub max_age: Option<Duration>,

    /// Maximum aggregate byte budget across retained files in this rule.
    #[serde(default, with = "serde_opt_bytes")]
    pub max_total_size: Option<u64>,

    /// Logical combination mode when multiple retention limits are specified.
    #[serde(default)]
    pub combine_mode: CombineMode,

    /// Order in which excess files are selected for eviction.
    #[serde(default)]
    pub eviction_order: EvictionOrder,

    /// Force dry-run planning without mutating filesystem state for this rule.
    #[serde(default)]
    pub dry_run: bool,

    /// Backup safety guardrails protecting valid backups and preventing stale wipes.
    #[serde(default)]
    pub backup_guard: BackupGuardConfig,

    /// Multi-probe active-file protection settings.
    #[serde(default)]
    pub in_use: InUseConfig,

    /// Optional external veto command executed prior to mutating/deleting a file.
    #[serde(default, alias = "pre_hook")]
    pub pre_delete_hook: Option<HookConfig>,

    /// Ordered pipeline of lifecycle actions executed on each evicted candidate.
    #[serde(default = "default_actions")]
    pub actions: Vec<ActionStep>,

    /// Destination directory required when `actions` includes `ActionStep::Move`.
    #[serde(default)]
    pub move_to: Option<PathBuf>,

    /// Optional `chrono` `strftime` date subdirectory template appended to `move_to`.
    #[serde(default)]
    pub move_date_subdir: Option<String>,

    /// Overwrite existing destination files during `ActionStep::Move`.
    #[serde(default)]
    pub move_overwrite: bool,

    /// Rule-specific quarantine directory used when `actions` includes `ActionStep::Trash`.
    #[serde(default)]
    pub trash_dir: Option<PathBuf>,

    /// Codec selected when `actions` includes `ActionStep::Compress`.
    #[serde(default)]
    pub compression: CompressionFormat,

    /// Optional codec-specific compression level.
    #[serde(default)]
    pub compression_level: Option<u32>,

    /// Remove source file after `ActionStep::Compress` succeeds (defaults to `true`).
    #[serde(default = "default_true")]
    pub compress_remove_source: bool,

    /// Overwrite existing archive destination during `ActionStep::Compress`.
    #[serde(default)]
    pub compress_overwrite: bool,

    /// Destination directory used by `ActionStep::CopyTruncate` (falls back to `move_to`).
    #[serde(default)]
    pub copy_truncate_to: Option<PathBuf>,

    /// Optional filename template when `actions` includes `ActionStep::RenameByDate`.
    #[serde(default)]
    pub rename_template: Option<String>,

    /// Optional rule-specific S3 / S3-compatible archival sink settings (overrides global `[s3]`).
    #[serde(default)]
    pub s3: Option<S3Config>,
}

/// Source of truth for candidate file timestamps.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeBasis {
    /// Filesystem modification timestamp (`mtime`).
    #[default]
    #[serde(alias = "Modified")]
    Modified,
    /// Filesystem creation / birth timestamp (`birthtime`).
    #[serde(alias = "Created")]
    Created,
    /// Timestamp parsed from the filename using `filename_date_regex` and `filename_date_format`.
    #[serde(alias = "FilenameDate")]
    FilenameDate,
}

/// Combination semantics when multiple retention thresholds are configured.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CombineMode {
    /// Evict a file if it violates any configured threshold (union).
    #[default]
    #[serde(alias = "Any")]
    Any,
    /// Evict a file only if it satisfies all configured thresholds (intersection).
    #[serde(alias = "All")]
    All,
}

/// Ordering applied when selecting excess files for count or size caps.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvictionOrder {
    /// Evict oldest candidates first (`t(f)` ascending).
    #[default]
    #[serde(alias = "OldestFirst")]
    OldestFirst,
    /// Evict largest candidates first (`size(f)` descending, ties oldest first).
    #[serde(alias = "LargestFirst")]
    LargestFirst,
}

/// Backup safety guardrails enforcing minimum valid copies and freshness bounds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupGuardConfig {
    /// Enable backup safety guardrails for this rule.
    #[serde(default)]
    pub enabled: bool,

    /// Minimum number of valid backups strictly protected from eviction.
    #[serde(default)]
    pub min_keep_files: usize,

    /// Minimum byte size for a file to be considered a valid, non-empty backup.
    #[serde(default, alias = "min_valid_size", with = "serde_bytes")]
    pub min_valid_backup_bytes: u64,

    /// Dead-man's-switch freshness window: freeze eviction if newest valid backup is older.
    #[serde(default, alias = "max_newest_age", with = "serde_opt_duration")]
    pub require_newer_within: Option<Duration>,

    /// Absolute UTC staleness cutoff: freeze eviction if newest valid backup predates this instant.
    #[serde(default, alias = "min_newest_timestamp")]
    pub freeze_if_newest_older_than: Option<chrono::DateTime<chrono::Utc>>,

    /// Freeze eviction when zero valid backups satisfy `min_valid_backup_bytes`.
    #[serde(default = "default_true", alias = "freeze_on_empty")]
    pub freeze_on_empty_valid: bool,
}

impl Default for BackupGuardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_keep_files: 0,
            min_valid_backup_bytes: 0,
            require_newer_within: None,
            freeze_if_newest_older_than: None,
            freeze_on_empty_valid: true,
        }
    }
}

/// Multi-probe active-writer and lock-detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InUseConfig {
    /// Enable in-use detection probes before planning/executing eviction.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Check companion sidecar lock/temp files (`.lock`, `.part`, etc.).
    #[serde(default = "default_true")]
    pub check_sidecar_locks: bool,

    /// Suffixes treated as active sidecar lock or partial-transfer files.
    #[serde(default = "default_sidecar_suffixes")]
    pub sidecar_suffixes: Vec<String>,

    /// Probe OS-native locks (`flock` on Unix, `share_mode(0)` on Windows).
    #[serde(default = "default_true")]
    pub check_os_locks: bool,

    /// Probe `/proc/*/fd` open file descriptors on Linux.
    #[serde(default = "default_true")]
    pub check_proc_fd: bool,

    /// Minimum elapsed age since `mtime` before a file can be touched.
    #[serde(default, with = "serde_opt_duration")]
    pub min_idle_age: Option<Duration>,

    /// Two-point `(size, mtime)` stability verification window.
    #[serde(default, with = "serde_opt_duration")]
    pub stability_window: Option<Duration>,
}

impl Default for InUseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_sidecar_locks: true,
            sidecar_suffixes: default_sidecar_suffixes(),
            check_os_locks: true,
            check_proc_fd: true,
            min_idle_age: None,
            stability_window: None,
        }
    }
}

/// Pre-action / pre-delete lifecycle hook configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookConfig {
    /// Executable command path or name.
    pub command: String,

    /// Argument templates supporting `{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`.
    #[serde(default)]
    pub args: Vec<String>,

    /// Maximum runtime allowed before terminating the hook process.
    #[serde(default = "default_hook_timeout", with = "serde_duration")]
    pub timeout: Duration,

    /// Failure handling policy when the hook exits non-zero (excluding `skip_exit_code`) or times out.
    #[serde(default)]
    pub on_failure: HookFailurePolicy,

    /// Exit code interpreted as a benign per-file skip veto (default `42`).
    #[serde(default = "default_skip_exit_code")]
    pub skip_exit_code: Option<i32>,

    /// Execute the hook even when running under `--dry-run` / `plan` (defaults to `false`).
    #[serde(default)]
    pub run_in_dry_run: bool,

    /// Optional working directory for hook execution.
    #[serde(default)]
    pub working_dir: Option<PathBuf>,

    /// Additional static environment variables passed to the hook process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Action taken when a hook fails unexpectedly or times out.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookFailurePolicy {
    /// Skip only the current file and proceed to subsequent files.
    #[default]
    #[serde(alias = "AbortFile")]
    AbortFile,
    /// Abort all remaining actions for the current rule.
    #[serde(alias = "AbortRule")]
    AbortRule,
    /// Log a warning and continue executing lifecycle actions on the file.
    #[serde(alias = "WarnAndContinue")]
    WarnAndContinue,
}

/// Ordered transformation or cleanup steps applied to each evicted candidate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStep {
    /// Rename file according to timestamp and `rename_template`.
    #[serde(alias = "RenameByDate")]
    RenameByDate,
    /// Compress file using the configured pure-Rust `CompressionFormat`.
    #[serde(alias = "Compress")]
    Compress,
    /// Move file atomically or cross-filesystem with SHA-256 verification.
    #[serde(alias = "Move")]
    Move,
    /// Atomically copy content and truncate active file in place (explicit opt-in).
    #[serde(alias = "CopyTruncate")]
    CopyTruncate,
    /// Upload candidate to S3 / S3-compatible object storage with mandatory SHA-256 verification.
    #[serde(alias = "UploadS3")]
    UploadS3,
    /// Move file to quarantine directory (`trash_dir` or `global_trash_dir`).
    #[serde(alias = "Trash")]
    Trash,
    /// Permanently unlink the candidate file.
    #[serde(alias = "Delete")]
    Delete,
}

/// Post-upload cryptographic integrity verification strategy for `ActionStep::UploadS3`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum S3VerifyMode {
    /// Fail-closed verification requiring `HeadObject` `x-amz-checksum-sha256` and `Content-Length` match.
    #[default]
    #[serde(alias = "ChecksumSha256")]
    ChecksumSha256,
    /// Explicit opt-in streaming `GetObject` readback computing SHA-256 for legacy S3 gateways.
    #[serde(alias = "ReadbackSha256")]
    ReadbackSha256,
}

/// S3 / S3-compatible archival sink settings.
///
/// Credentials are read exclusively from configured environment variables; plaintext secret fields are disallowed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3Config {
    /// Target S3 bucket name.
    pub bucket: String,

    /// AWS or S3-compatible signing region (e.g., `"us-east-1"`).
    pub region: String,

    /// Optional custom endpoint URL (e.g., `"https://minio.internal:9000"`).
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Optional static key prefix prepended to uploaded object keys.
    #[serde(default)]
    pub prefix: Option<String>,

    /// Optional `chrono` `strftime` date subdirectory template appended after `prefix`.
    #[serde(default)]
    pub key_date_subdir: Option<String>,

    /// Use path-style URLs (`<endpoint>/<bucket>/<key>`) rather than virtual-hosted style.
    #[serde(default)]
    pub force_path_style: bool,

    /// Permit plaintext `http://` custom endpoints (defaults to `false`; `https://` required otherwise).
    #[serde(default)]
    pub allow_insecure_http: bool,

    /// Permit overwriting an existing remote object key (defaults to `false`).
    #[serde(default)]
    pub overwrite: bool,

    /// Optional S3 storage class header value (e.g., `"STANDARD_IA"`, `"GLACIER"`).
    #[serde(default)]
    pub storage_class: Option<String>,

    /// Post-upload SHA-256 verification strategy (`checksum_sha256` default).
    #[serde(default)]
    pub verify_mode: S3VerifyMode,

    /// Environment variable name supplying the S3 access key ID.
    #[serde(default = "default_s3_access_key_env")]
    pub access_key_env: String,

    /// Environment variable name supplying the S3 secret access key.
    #[serde(default = "default_s3_secret_key_env")]
    pub secret_key_env: String,

    /// Optional environment variable name supplying a temporary S3 session token.
    #[serde(default = "default_s3_session_token_env")]
    pub session_token_env: Option<String>,

    /// Per-request network timeout (default `"60s"`).
    #[serde(default = "default_s3_timeout", with = "serde_duration")]
    pub timeout: Duration,

    /// Maximum retry attempts for transient 5xx or transport failures (default `3`).
    #[serde(default = "default_s3_max_retries")]
    pub max_retries: u32,

    /// Multipart threshold and part size in bytes (default `"16MiB"`, minimum `"5MiB"`).
    #[serde(default = "default_s3_multipart_part_size", with = "serde_bytes")]
    pub multipart_part_size: u64,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: String::new(),
            endpoint: None,
            prefix: None,
            key_date_subdir: None,
            force_path_style: false,
            allow_insecure_http: false,
            overwrite: false,
            storage_class: None,
            verify_mode: S3VerifyMode::ChecksumSha256,
            access_key_env: default_s3_access_key_env(),
            secret_key_env: default_s3_secret_key_env(),
            session_token_env: default_s3_session_token_env(),
            timeout: default_s3_timeout(),
            max_retries: default_s3_max_retries(),
            multipart_part_size: default_s3_multipart_part_size(),
        }
    }
}

/// Pure-Rust compression formats supported by `ActionStep::Compress`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressionFormat {
    /// Standard gzip stream via `flate2` (`miniz_oxide`).
    #[default]
    #[serde(alias = "Gzip")]
    Gzip,
    /// High-speed LZ4 frame via `lz4_flex`.
    #[serde(alias = "Lz4")]
    Lz4,
    /// High-ratio Brotli stream via `brotli`.
    #[serde(alias = "Brotli")]
    Brotli,
    /// Deflate-compressed ZIP archive via `zip`.
    #[serde(alias = "Zip")]
    Zip,
}

impl Config {
    /// Parses a TOML or YAML configuration file, auto-detecting format from its file extension.
    ///
    /// Call [`Config::validate`] afterward to enforce semantic invariants, or use
    /// [`Config::load_validated`] to parse and validate in one step.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|err| FileCleanerError::io(path, err))?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        match ext.as_str() {
            "toml" => toml::from_str::<Self>(&raw).map_err(|err| FileCleanerError::ConfigParse {
                path: path.to_path_buf(),
                reason: err.to_string(),
            }),
            "yaml" | "yml" => {
                serde_yaml_ng::from_str::<Self>(&raw).map_err(|err| FileCleanerError::ConfigParse {
                    path: path.to_path_buf(),
                    reason: err.to_string(),
                })
            }
            _ => Err(FileCleanerError::ConfigFormat {
                path: path.to_path_buf(),
                extension: ext,
            }),
        }
    }

    /// Parses a configuration file and immediately enforces [`Config::validate`].
    pub fn load_validated(path: impl AsRef<Path>) -> Result<Self> {
        let config = Self::from_file(path)?;
        config.validate()?;
        Ok(config)
    }

    /// Validates all structural, regex, glob, retention, and path-safety invariants.
    pub fn validate(&self) -> Result<()> {
        if self.daemon.interval.is_zero() {
            return Err(FileCleanerError::ConfigValidation(
                "daemon.interval must be greater than zero".to_string(),
            ));
        }

        if let Some(audit_log) = &self.audit_log {
            if audit_log.as_os_str().is_empty() {
                return Err(FileCleanerError::ConfigValidation(
                    "audit_log cannot be empty".to_string(),
                ));
            }
        }

        if self.rules.is_empty() {
            return Err(FileCleanerError::ConfigValidation(
                "configuration must define at least one rule".to_string(),
            ));
        }

        if let Some(global_trash) = &self.global_trash_dir {
            if global_trash.as_os_str().is_empty() {
                return Err(FileCleanerError::ConfigValidation(
                    "global_trash_dir cannot be empty".to_string(),
                ));
            }
        }

        if let Some(global_s3) = &self.s3 {
            validate_s3_config("global [s3]", global_s3)?;
        }

        let mut seen_names = HashSet::with_capacity(self.rules.len());

        for rule in &self.rules {
            let trimmed_name = rule.name.trim();
            if trimmed_name.is_empty() {
                return Err(FileCleanerError::ConfigValidation(
                    "rule name must not be empty".to_string(),
                ));
            }
            if !seen_names.insert(trimmed_name.to_string()) {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "duplicate rule name '{trimmed_name}'"
                )));
            }
            if rule.path.as_os_str().is_empty() {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' must specify a non-empty path"
                )));
            }
            if rule.actions.is_empty() {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' must specify at least one action step"
                )));
            }

            let compiled = crate::scanner::compile_rule_matchers(rule)?;

            if let (Some(min_sz), Some(max_sz)) = (rule.min_file_size, rule.max_file_size) {
                if min_sz > max_sz {
                    return Err(FileCleanerError::ConfigValidation(format!(
                        "rule '{trimmed_name}' has min_file_size ({min_sz}) > max_file_size ({max_sz})"
                    )));
                }
            }

            if let Some(max_files) = rule.max_files {
                if max_files < rule.backup_guard.min_keep_files {
                    return Err(FileCleanerError::ConfigValidation(format!(
                        "rule '{trimmed_name}' has max_files ({max_files}) < backup_guard.min_keep_files ({})",
                        rule.backup_guard.min_keep_files
                    )));
                }
            }

            for (idx, step) in rule.actions.iter().enumerate() {
                if matches!(step, ActionStep::Delete | ActionStep::Trash)
                    && idx + 1 < rule.actions.len()
                {
                    return Err(FileCleanerError::ConfigValidation(format!(
                        "rule '{trimmed_name}' has terminal action {step:?} followed by additional steps"
                    )));
                }
            }

            if rule.actions.contains(&ActionStep::Move) && rule.move_to.is_none() {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' includes ActionStep::Move without move_to destination"
                )));
            }

            for (field, maybe_fmt) in [
                ("move_date_subdir", &rule.move_date_subdir),
                ("rename_template", &rule.rename_template),
            ] {
                if let Some(fmt) = maybe_fmt {
                    if fmt.trim().is_empty()
                        || StrftimeItems::new(fmt).any(|item| matches!(item, Item::Error))
                    {
                        return Err(FileCleanerError::ConfigValidation(format!(
                            "rule '{trimmed_name}' has invalid {field} '{fmt}'"
                        )));
                    }
                }
            }

            if rule.actions.contains(&ActionStep::CopyTruncate)
                && rule.copy_truncate_to.is_none()
                && rule.move_to.is_none()
            {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' includes ActionStep::CopyTruncate without copy_truncate_to or move_to destination"
                )));
            }

            if rule.actions.contains(&ActionStep::Trash)
                && rule.trash_dir.is_none()
                && self.global_trash_dir.is_none()
            {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' includes ActionStep::Trash without trash_dir or global_trash_dir"
                )));
            }

            if let Some(rule_s3) = &rule.s3 {
                validate_s3_config(&format!("rule '{trimmed_name}' s3"), rule_s3)?;
            }

            if rule.actions.contains(&ActionStep::UploadS3) && self.effective_s3(rule).is_none() {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "rule '{trimmed_name}' includes ActionStep::UploadS3 without rule s3 or global [s3] configuration"
                )));
            }

            if let Some(hook) = &rule.pre_delete_hook {
                if hook.command.trim().is_empty() {
                    return Err(FileCleanerError::ConfigValidation(format!(
                        "rule '{trimmed_name}' pre_delete_hook command must not be empty"
                    )));
                }
                if hook.timeout.is_zero() {
                    return Err(FileCleanerError::ConfigValidation(format!(
                        "rule '{trimmed_name}' pre_delete_hook timeout must be greater than zero"
                    )));
                }
            }

            if rule.recursive {
                if let Some(move_dest) = &rule.move_to {
                    validate_destination_not_in_recursive_root(
                        trimmed_name,
                        "move_to",
                        &rule.path,
                        move_dest,
                        &compiled.exclude_globs,
                        compiled.exclude_regex.as_ref(),
                    )?;
                }

                if let Some(ct_dest) = &rule.copy_truncate_to {
                    validate_destination_not_in_recursive_root(
                        trimmed_name,
                        "copy_truncate_to",
                        &rule.path,
                        ct_dest,
                        &compiled.exclude_globs,
                        compiled.exclude_regex.as_ref(),
                    )?;
                }

                let effective_trash = rule
                    .trash_dir
                    .as_ref()
                    .or(self.global_trash_dir.as_ref())
                    .filter(|_| {
                        rule.actions.contains(&ActionStep::Trash) || rule.trash_dir.is_some()
                    });

                if let Some(trash_dest) = effective_trash {
                    validate_destination_not_in_recursive_root(
                        trimmed_name,
                        "trash_dir",
                        &rule.path,
                        trash_dest,
                        &compiled.exclude_globs,
                        compiled.exclude_regex.as_ref(),
                    )?;
                }
            }
        }

        Ok(())
    }
}

/// Validates S3 archival sink configuration invariants (`INV-CFG-11` and `INV-CFG-12`).
fn validate_s3_config(scope: &str, s3: &S3Config) -> Result<()> {
    let bucket = s3.bucket.trim();
    if bucket.is_empty()
        || bucket.contains('/')
        || bucket.contains('\\')
        || bucket.contains("..")
        || bucket.chars().any(char::is_whitespace)
    {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope} bucket must be a non-empty valid bucket name without slashes or whitespace"
        )));
    }

    let region = s3.region.trim();
    if region.is_empty() || region.contains('/') || region.chars().any(char::is_whitespace) {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope} region must be non-empty without slashes or whitespace"
        )));
    }

    validate_env_var_name(scope, "access_key_env", &s3.access_key_env)?;
    validate_env_var_name(scope, "secret_key_env", &s3.secret_key_env)?;
    if let Some(token_env) = &s3.session_token_env {
        validate_env_var_name(scope, "session_token_env", token_env)?;
    }

    if let Some(ep_raw) = &s3.endpoint {
        let ep = ep_raw.trim();
        let rest = if let Some(after) = ep.strip_prefix("https://") {
            after
        } else if let Some(after) = ep.strip_prefix("http://") {
            if !s3.allow_insecure_http {
                return Err(FileCleanerError::ConfigValidation(format!(
                    "{scope} endpoint '{ep}' uses plaintext http:// while allow_insecure_http is false"
                )));
            }
            after
        } else {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope} endpoint '{ep}' must use https:// (or http:// with allow_insecure_http = true)"
            )));
        };

        let host = rest.split('/').next().unwrap_or("");
        if host.is_empty() || ep.chars().any(char::is_whitespace) {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope} endpoint '{ep}' must include a valid host without whitespace"
            )));
        }
    }

    if let Some(prefix) = &s3.prefix {
        if has_traversal_or_backslash(prefix) {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope} prefix '{prefix}' must not contain '..' segments or backslashes"
            )));
        }
    }

    if let Some(date_subdir) = &s3.key_date_subdir {
        if date_subdir.trim().is_empty()
            || has_traversal_or_backslash(date_subdir)
            || StrftimeItems::new(date_subdir).any(|item| matches!(item, Item::Error))
        {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope} has invalid key_date_subdir '{date_subdir}'"
            )));
        }
    }

    if let Some(storage_class) = &s3.storage_class {
        if storage_class.trim().is_empty() || storage_class.chars().any(char::is_control) {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope} storage_class must be non-empty without control characters"
            )));
        }
    }

    if s3.timeout.is_zero() {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope} timeout must be greater than zero"
        )));
    }

    if s3.multipart_part_size < MIN_S3_MULTIPART_PART_SIZE {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope} multipart_part_size ({}) must be at least {MIN_S3_MULTIPART_PART_SIZE} bytes (5 MiB)",
            s3.multipart_part_size
        )));
    }

    Ok(())
}

fn validate_env_var_name(scope: &str, field: &str, raw: &str) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('=')
        || trimmed.contains('\0')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope} {field} must be a non-empty environment variable name"
        )));
    }
    Ok(())
}

fn has_traversal_or_backslash(raw: &str) -> bool {
    raw.contains('\\') || raw.split('/').any(|seg| seg == "..")
}

/// Rejects destination directories nested inside a recursive scan root unless explicitly excluded.
fn validate_destination_not_in_recursive_root(
    rule_name: &str,
    field_name: &str,
    root: &Path,
    dest: &Path,
    exclude_globs: &[globset::GlobMatcher],
    exclude_regex: Option<&Regex>,
) -> Result<()> {
    let norm_root = resolve_path_lexically(root);
    let norm_dest = resolve_path_lexically(dest);

    if norm_dest == norm_root {
        return Err(FileCleanerError::ConfigValidation(format!(
            "rule '{rule_name}' {field_name} ('{}') must not equal recursive rule path",
            dest.display()
        )));
    }

    if let Ok(rel) = norm_dest.strip_prefix(&norm_root) {
        if !is_relative_path_excluded(rel, exclude_globs, exclude_regex) {
            return Err(FileCleanerError::ConfigValidation(format!(
                "rule '{rule_name}' {field_name} ('{}') resides inside recursive rule path ('{}') without matching exclude_globs/exclude_regex",
                dest.display(),
                root.display()
            )));
        }
    }

    Ok(())
}

/// Checks whether a relative path or any of its parent segments matches the rule's exclusions.
fn is_relative_path_excluded(
    rel: &Path,
    exclude_globs: &[globset::GlobMatcher],
    exclude_regex: Option<&Regex>,
) -> bool {
    let mut prefix = PathBuf::new();
    for component in rel.components() {
        prefix.push(component);
        let slash_prefix = to_forward_slash_str(&prefix);
        if exclude_globs
            .iter()
            .any(|g| g.is_match(&prefix) || g.is_match(&slash_prefix))
        {
            return true;
        }
        if let Some(re) = exclude_regex {
            if re.is_match(&slash_prefix) {
                return true;
            }
        }
    }

    let probe_child = rel.join("__filecleaner_probe__");
    let slash_child = to_forward_slash_str(&probe_child);
    if exclude_globs
        .iter()
        .any(|g| g.is_match(&probe_child) || g.is_match(&slash_child))
    {
        return true;
    }
    if let Some(re) = exclude_regex {
        if re.is_match(&slash_child) {
            return true;
        }
    }

    false
}

fn to_forward_slash_str(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(os) => Some(os.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Normalizes a path by canonicalizing the deepest existing ancestor and appending remaining
/// normalized components cleanly without requiring the final leaf directory to exist yet.
pub(crate) fn resolve_path_lexically(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };

    let mut stack: Vec<Component<'_>> = Vec::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                }
            }
            other => stack.push(other),
        }
    }

    let mut lexical = PathBuf::new();
    for comp in stack {
        lexical.push(comp.as_os_str());
    }

    // Attempt to resolve symlinks on the deepest existing ancestor so alias comparisons match.
    let mut existing = lexical.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        if let Some(name) = existing.file_name() {
            tail.push(name.to_os_string());
        } else {
            break;
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => break,
        }
    }

    if let Ok(mut canon) = fs::canonicalize(existing) {
        for seg in tail.into_iter().rev() {
            canon.push(seg);
        }
        canon
    } else {
        lexical
    }
}

/// Parses human-readable byte sizes (`B`, `KB`, `KiB`, `MB`, `MiB`, `GB`, `GiB`, `TB`, `TiB`, `PB`, `PiB`).
///
/// Accepts plain integers (`"1024"`), SI decimal units (`"10MB"` = `10_000_000`),
/// IEC binary units (`"10MiB"` = `10_485_760`), and fractional values (`"1.5 GiB"`).
pub fn parse_bytes(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(FileCleanerError::InvalidByteSize(input.to_string()));
    }

    let split_idx = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());

    let (num_str, unit_raw) = trimmed.split_at(split_idx);
    let num_str = num_str.trim();
    let unit = unit_raw.trim().to_ascii_lowercase();

    if num_str.is_empty() || num_str.starts_with('-') {
        return Err(FileCleanerError::InvalidByteSize(input.to_string()));
    }

    let multiplier: u64 = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kib" => 1024,
        "kb" => 1_000,
        "m" | "mib" => 1024_u64.pow(2),
        "mb" => 1_000_u64.pow(2),
        "g" | "gib" => 1024_u64.pow(3),
        "gb" => 1_000_u64.pow(3),
        "t" | "tib" => 1024_u64.pow(4),
        "tb" => 1_000_u64.pow(4),
        "p" | "pib" => 1024_u64.pow(5),
        "pb" => 1_000_u64.pow(5),
        _ => return Err(FileCleanerError::InvalidByteSize(input.to_string())),
    };

    if !num_str.contains('.') {
        let integer: u64 = num_str
            .parse()
            .map_err(|_| FileCleanerError::InvalidByteSize(input.to_string()))?;
        return integer
            .checked_mul(multiplier)
            .ok_or_else(|| FileCleanerError::InvalidByteSize(input.to_string()));
    }

    if num_str.matches('.').count() > 1 {
        return Err(FileCleanerError::InvalidByteSize(input.to_string()));
    }

    let float_val: f64 = num_str
        .parse()
        .map_err(|_| FileCleanerError::InvalidByteSize(input.to_string()))?;
    if !float_val.is_finite() || float_val.is_sign_negative() {
        return Err(FileCleanerError::InvalidByteSize(input.to_string()));
    }

    let scaled = float_val * (multiplier as f64);
    if !scaled.is_finite() || scaled > (u64::MAX as f64) {
        return Err(FileCleanerError::InvalidByteSize(input.to_string()));
    }

    Ok(scaled.round() as u64)
}

/// Parses human-readable duration strings supporting `ms`, `s`, `m`, `h`, `d`, `w`
/// as single tokens (`"30d"`, `"500ms"`) or compound sequences (`"1d 12h"`, `"2m30s"`).
pub fn parse_duration_str(input: &str) -> Result<Duration> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(FileCleanerError::InvalidDuration(input.to_string()));
    }

    // Bare integer fallback treated as seconds when no suffix is present.
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        let secs: u64 = trimmed
            .parse()
            .map_err(|_| FileCleanerError::InvalidDuration(input.to_string()))?;
        return Ok(Duration::from_secs(secs));
    }

    let bytes = trimmed.as_bytes();
    let mut idx = 0usize;
    let mut total_millis: u128 = 0;
    let mut parsed_any = false;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let num_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == num_start {
            return Err(FileCleanerError::InvalidDuration(input.to_string()));
        }
        let value: u128 = trimmed[num_start..idx]
            .parse()
            .map_err(|_| FileCleanerError::InvalidDuration(input.to_string()))?;

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let unit_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
            idx += 1;
        }
        if idx == unit_start {
            return Err(FileCleanerError::InvalidDuration(input.to_string()));
        }

        let unit = trimmed[unit_start..idx].to_ascii_lowercase();
        let unit_millis: u128 = match unit.as_str() {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            "w" => 604_800_000,
            _ => return Err(FileCleanerError::InvalidDuration(input.to_string())),
        };

        let delta = value
            .checked_mul(unit_millis)
            .ok_or_else(|| FileCleanerError::InvalidDuration(input.to_string()))?;
        total_millis = total_millis
            .checked_add(delta)
            .ok_or_else(|| FileCleanerError::InvalidDuration(input.to_string()))?;
        parsed_any = true;
    }

    if !parsed_any {
        return Err(FileCleanerError::InvalidDuration(input.to_string()));
    }

    let millis_u64 = u64::try_from(total_millis)
        .map_err(|_| FileCleanerError::InvalidDuration(input.to_string()))?;
    Ok(Duration::from_millis(millis_u64))
}

fn default_true() -> bool {
    true
}

fn default_daemon_interval() -> Duration {
    Duration::from_secs(60)
}

fn default_debounce() -> Duration {
    Duration::from_secs(2)
}

fn default_hook_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_skip_exit_code() -> Option<i32> {
    Some(42)
}

fn default_actions() -> Vec<ActionStep> {
    vec![ActionStep::Delete]
}

fn default_s3_access_key_env() -> String {
    "AWS_ACCESS_KEY_ID".to_string()
}

fn default_s3_secret_key_env() -> String {
    "AWS_SECRET_ACCESS_KEY".to_string()
}

fn default_s3_session_token_env() -> Option<String> {
    Some("AWS_SESSION_TOKEN".to_string())
}

fn default_s3_timeout() -> Duration {
    Duration::from_secs(60)
}

fn default_s3_max_retries() -> u32 {
    3
}

fn default_s3_multipart_part_size() -> u64 {
    DEFAULT_S3_MULTIPART_PART_SIZE
}

fn default_sidecar_suffixes() -> Vec<String> {
    vec![
        ".lock".to_string(),
        ".lck".to_string(),
        ".part".to_string(),
        ".tmp".to_string(),
        ".processing".to_string(),
    ]
}

mod serde_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(val: &u64, ser: S) -> std::result::Result<S::Ok, S::Error> {
        ser.serialize_u64(*val)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deser: D) -> std::result::Result<u64, D::Error> {
        struct BytesVisitor;
        impl<'de> serde::de::Visitor<'de> for BytesVisitor {
            type Value = u64;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a non-negative integer or byte-size string such as '10 MiB'")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<u64, E> {
                Ok(v)
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<u64, E> {
                u64::try_from(v).map_err(|_| E::custom(format!("negative byte size {v}")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<u64, E> {
                parse_bytes(v).map_err(E::custom)
            }
        }
        deser.deserialize_any(BytesVisitor)
    }
}

mod serde_opt_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(
        val: &Option<u64>,
        ser: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match val {
            Some(v) => ser.serialize_some(v),
            None => ser.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deser: D,
    ) -> std::result::Result<Option<u64>, D::Error> {
        struct OptBytesVisitor;
        impl<'de> serde::de::Visitor<'de> for OptBytesVisitor {
            type Value = Option<u64>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("null, a non-negative integer, or a byte-size string")
            }

            fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> std::result::Result<Self::Value, D2::Error> {
                serde_bytes::deserialize(deserializer).map(Some)
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(Some(v))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                u64::try_from(v)
                    .map(Some)
                    .map_err(|_| E::custom(format!("negative byte size {v}")))
            }

            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                parse_bytes(v).map(Some).map_err(E::custom)
            }
        }
        deser.deserialize_any(OptBytesVisitor)
    }
}

mod serde_duration {
    use super::*;

    pub fn serialize<S: Serializer>(
        val: &Duration,
        ser: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if val.subsec_nanos() == 0 {
            ser.serialize_str(&format!("{}s", val.as_secs()))
        } else {
            ser.serialize_str(&format!("{}ms", val.as_millis()))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deser: D,
    ) -> std::result::Result<Duration, D::Error> {
        struct DurationVisitor;
        impl<'de> serde::de::Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a non-negative integer seconds or duration string such as '30d'")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Duration, E> {
                Ok(Duration::from_secs(v))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Duration, E> {
                u64::try_from(v)
                    .map(Duration::from_secs)
                    .map_err(|_| E::custom(format!("negative duration {v}")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Duration, E> {
                parse_duration_str(v).map_err(E::custom)
            }
        }
        deser.deserialize_any(DurationVisitor)
    }
}

mod serde_opt_duration {
    use super::*;

    pub fn serialize<S: Serializer>(
        val: &Option<Duration>,
        ser: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match val {
            Some(d) => serde_duration::serialize(d, ser),
            None => ser.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deser: D,
    ) -> std::result::Result<Option<Duration>, D::Error> {
        struct OptDurationVisitor;
        impl<'de> serde::de::Visitor<'de> for OptDurationVisitor {
            type Value = Option<Duration>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("null, integer seconds, or a duration string such as '7d'")
            }

            fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> std::result::Result<Self::Value, D2::Error> {
                serde_duration::deserialize(deserializer).map(Some)
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(Some(Duration::from_secs(v)))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                u64::try_from(v)
                    .map(|secs| Some(Duration::from_secs(secs)))
                    .map_err(|_| E::custom(format!("negative duration {v}")))
            }

            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                parse_duration_str(v).map(Some).map_err(E::custom)
            }
        }
        deser.deserialize_any(OptDurationVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn minimal_rule(name: &str, path: PathBuf) -> RuleConfig {
        RuleConfig {
            name: name.to_string(),
            path,
            enabled: true,
            recursive: false,
            max_depth: None,
            follow_symlinks: false,
            include_globs: vec![],
            exclude_globs: vec![],
            include_regex: None,
            exclude_regex: None,
            min_file_size: None,
            max_file_size: None,
            time_basis: TimeBasis::Modified,
            filename_date_regex: None,
            filename_date_format: None,
            max_files: Some(10),
            max_age: None,
            max_total_size: None,
            combine_mode: CombineMode::Any,
            eviction_order: EvictionOrder::OldestFirst,
            dry_run: false,
            backup_guard: BackupGuardConfig::default(),
            in_use: InUseConfig::default(),
            pre_delete_hook: None,
            actions: vec![ActionStep::Delete],
            move_to: None,
            move_date_subdir: None,
            move_overwrite: false,
            trash_dir: None,
            compression: CompressionFormat::Gzip,
            compression_level: None,
            compress_remove_source: true,
            compress_overwrite: false,
            copy_truncate_to: None,
            rename_template: None,
            s3: None,
        }
    }

    #[test]
    fn parse_bytes_supports_si_iec_and_fractions() {
        assert_eq!(parse_bytes("0").unwrap(), 0);
        assert_eq!(parse_bytes("512B").unwrap(), 512);
        assert_eq!(parse_bytes("4KB").unwrap(), 4_000);
        assert_eq!(parse_bytes("4KiB").unwrap(), 4_096);
        assert_eq!(parse_bytes("2 MB").unwrap(), 2_000_000);
        assert_eq!(parse_bytes("2MiB").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_bytes("3GB").unwrap(), 3_000_000_000);
        assert_eq!(parse_bytes("1.5 GiB").unwrap(), 1_610_612_736);
        assert_eq!(parse_bytes("1TB").unwrap(), 1_000_000_000_000);
        assert_eq!(parse_bytes("1TiB").unwrap(), 1024_u64.pow(4));
        assert!(parse_bytes("").is_err());
        assert!(parse_bytes("-5MB").is_err());
        assert!(parse_bytes("12XB").is_err());
        assert!(parse_bytes("1.2.3GB").is_err());
    }

    #[test]
    fn parse_duration_supports_single_compound_and_rejects_invalid() {
        assert_eq!(
            parse_duration_str("250ms").unwrap(),
            Duration::from_millis(250)
        );
        assert_eq!(parse_duration_str("45s").unwrap(), Duration::from_secs(45));
        assert_eq!(parse_duration_str("15m").unwrap(), Duration::from_secs(900));
        assert_eq!(
            parse_duration_str("2h").unwrap(),
            Duration::from_secs(7_200)
        );
        assert_eq!(
            parse_duration_str("7d").unwrap(),
            Duration::from_secs(7 * 86_400)
        );
        assert_eq!(
            parse_duration_str("2w").unwrap(),
            Duration::from_secs(14 * 86_400)
        );
        assert_eq!(
            parse_duration_str("1d 12h 30m").unwrap(),
            Duration::from_secs(86_400 + 12 * 3_600 + 30 * 60)
        );
        assert_eq!(parse_duration_str("120").unwrap(), Duration::from_secs(120));
        assert!(parse_duration_str("").is_err());
        assert!(parse_duration_str("10y").is_err());
        assert!(parse_duration_str("-5s").is_err());
        assert!(parse_duration_str("abc").is_err());
    }

    #[test]
    fn from_file_auto_detects_toml_and_yaml_and_rejects_unknown_ext() {
        let tmp = TempDir::new().unwrap();
        let scan_dir = tmp.path().join("backups");
        fs::create_dir_all(&scan_dir).unwrap();

        let toml_path = tmp.path().join("config.toml");
        let toml_body = format!(
            r#"
[daemon]
interval = "5m"
watch = true

[[rules]]
name = "pg-backups"
path = "{}"
max_files = 14
max_age = "30d"
max_total_size = "10 GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "delete"]
compression = "lz4"

[rules.backup_guard]
enabled = true
min_keep_files = 3
min_valid_backup_bytes = "1 MiB"
require_newer_within = "36h"
"#,
            scan_dir.display().to_string().replace('\\', "\\\\")
        );
        fs::write(&toml_path, toml_body).unwrap();

        let parsed_toml = Config::load_validated(&toml_path).unwrap();
        assert_eq!(parsed_toml.daemon.interval, Duration::from_secs(300));
        assert!(parsed_toml.daemon.watch);
        assert_eq!(parsed_toml.rules.len(), 1);
        assert_eq!(
            parsed_toml.rules[0].max_total_size,
            Some(10 * 1024_u64.pow(3))
        );
        assert_eq!(parsed_toml.rules[0].compression, CompressionFormat::Lz4);
        assert_eq!(parsed_toml.rules[0].backup_guard.min_keep_files, 3);

        let yaml_path = tmp.path().join("config.yml");
        let yaml_body = format!(
            r#"
daemon:
  interval: 2m
rules:
  - name: app-logs
    path: "{}"
    time_basis: filename_date
    filename_date_regex: 'app-(\d{{4}}-\d{{2}}-\d{{2}})\.log'
    filename_date_format: "%Y-%m-%d"
    max_age: 14d
    actions:
      - compress
      - trash
    compression: brotli
    trash_dir: "{}"
    pre_delete_hook:
      command: /usr/bin/true
      timeout: 10s
      on_failure: abort_rule
"#,
            scan_dir.display().to_string().replace('\\', "\\\\"),
            tmp.path()
                .join("trash")
                .display()
                .to_string()
                .replace('\\', "\\\\")
        );
        fs::write(&yaml_path, yaml_body).unwrap();

        let parsed_yaml = Config::load_validated(&yaml_path).unwrap();
        assert_eq!(parsed_yaml.rules[0].time_basis, TimeBasis::FilenameDate);
        assert_eq!(parsed_yaml.rules[0].compression, CompressionFormat::Brotli);
        assert_eq!(
            parsed_yaml.rules[0]
                .pre_delete_hook
                .as_ref()
                .unwrap()
                .on_failure,
            HookFailurePolicy::AbortRule
        );

        let json_path = tmp.path().join("config.json");
        fs::write(&json_path, "{}").unwrap();
        assert!(matches!(
            Config::from_file(&json_path),
            Err(FileCleanerError::ConfigFormat { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_rules_and_duplicate_names() {
        let empty = Config::default();
        assert!(matches!(
            empty.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let dup = Config {
            rules: vec![
                minimal_rule("dup", PathBuf::from("/tmp/a")),
                minimal_rule("dup", PathBuf::from("/tmp/b")),
            ],
            ..Config::default()
        };
        assert!(matches!(
            dup.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));
    }

    #[test]
    fn validate_rejects_invalid_globs_and_regexes() {
        let mut bad_glob = minimal_rule("bad-glob", PathBuf::from("/tmp/a"));
        bad_glob.include_globs = vec!["[unclosed".to_string()];
        let cfg = Config {
            rules: vec![bad_glob],
            ..Config::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(FileCleanerError::InvalidGlob { .. })
        ));

        let mut bad_regex = minimal_rule("bad-regex", PathBuf::from("/tmp/a"));
        bad_regex.exclude_regex = Some("(unclosed".to_string());
        let cfg = Config {
            rules: vec![bad_regex],
            ..Config::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(FileCleanerError::InvalidRegex { .. })
        ));
    }

    #[test]
    fn validate_rejects_max_files_below_min_keep_files() {
        let mut rule = minimal_rule("floor-conflict", PathBuf::from("/tmp/backups"));
        rule.max_files = Some(2);
        rule.backup_guard.enabled = true;
        rule.backup_guard.min_keep_files = 5;

        let cfg = Config {
            rules: vec![rule],
            ..Config::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));
    }

    #[test]
    fn validate_rejects_move_to_and_trash_dir_inside_recursive_path_unless_excluded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("logs");
        let archive = root.join("archive");
        let trash = root.join(".trash");
        fs::create_dir_all(&root).unwrap();

        let mut rule = minimal_rule("recursive-move", root.clone());
        rule.recursive = true;
        rule.actions = vec![ActionStep::Move];
        rule.move_to = Some(archive);

        let cfg = Config {
            rules: vec![rule.clone()],
            ..Config::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        // Excluding the destination subtree permits recursive scanning safely.
        rule.exclude_globs = vec!["archive/**".to_string()];
        let cfg_excluded = Config {
            rules: vec![rule],
            ..Config::default()
        };
        assert!(cfg_excluded.validate().is_ok());

        // Same enforcement for trash_dir inside recursive rule path.
        let mut trash_rule = minimal_rule("recursive-trash", root.clone());
        trash_rule.recursive = true;
        trash_rule.actions = vec![ActionStep::Trash];
        trash_rule.trash_dir = Some(trash);

        let cfg_trash_bad = Config {
            rules: vec![trash_rule.clone()],
            ..Config::default()
        };
        assert!(matches!(
            cfg_trash_bad.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        trash_rule.exclude_globs = vec![".trash".to_string()];
        let cfg_trash_ok = Config {
            rules: vec![trash_rule],
            ..Config::default()
        };
        assert!(cfg_trash_ok.validate().is_ok());
    }

    #[test]
    fn validate_rejects_missing_action_destinations_and_invalid_hooks_or_filename_date() {
        let mut missing_move = minimal_rule("missing-move", PathBuf::from("/tmp/a"));
        missing_move.actions = vec![ActionStep::Move];
        let cfg_move = Config {
            rules: vec![missing_move],
            ..Config::default()
        };
        assert!(matches!(
            cfg_move.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut missing_trash = minimal_rule("missing-trash", PathBuf::from("/tmp/a"));
        missing_trash.actions = vec![ActionStep::Trash];
        let cfg_trash = Config {
            rules: vec![missing_trash],
            ..Config::default()
        };
        assert!(matches!(
            cfg_trash.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut missing_fn_date = minimal_rule("missing-fn-date", PathBuf::from("/tmp/a"));
        missing_fn_date.time_basis = TimeBasis::FilenameDate;
        let cfg_fn = Config {
            rules: vec![missing_fn_date],
            ..Config::default()
        };
        assert!(matches!(
            cfg_fn.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut bad_hook = minimal_rule("bad-hook", PathBuf::from("/tmp/a"));
        bad_hook.pre_delete_hook = Some(HookConfig {
            command: "   ".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });
        let cfg_hook = Config {
            rules: vec![bad_hook],
            ..Config::default()
        };
        assert!(matches!(
            cfg_hook.validate(),
            Err(FileCleanerError::ConfigValidation(_))
        ));

        let mut zero_timeout_hook = minimal_rule("zero-timeout", PathBuf::from("/tmp/a"));
        zero_timeout_hook.pre_delete_hook = Some(HookConfig {
            command: "/bin/true".to_string(),
            args: vec![],
            timeout: Duration::ZERO,
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });
        assert!(Config {
            rules: vec![zero_timeout_hook],
            ..Config::default()
        }
        .validate()
        .is_err());

        let mut bad_fmt = minimal_rule("bad-strftime", PathBuf::from("/tmp/a"));
        bad_fmt.time_basis = TimeBasis::FilenameDate;
        bad_fmt.filename_date_regex = Some(r"(\d{4})".to_string());
        bad_fmt.filename_date_format = Some("%Q".to_string());
        assert!(Config {
            rules: vec![bad_fmt],
            ..Config::default()
        }
        .validate()
        .is_err());

        let mut empty_actions = minimal_rule("empty-actions", PathBuf::from("/tmp/a"));
        empty_actions.actions = vec![];
        assert!(Config {
            rules: vec![empty_actions],
            ..Config::default()
        }
        .validate()
        .is_err());

        let empty_name = minimal_rule("   ", PathBuf::from("/tmp/a"));
        assert!(Config {
            rules: vec![empty_name],
            ..Config::default()
        }
        .validate()
        .is_err());

        let empty_path = minimal_rule("empty-path", PathBuf::new());
        assert!(Config {
            rules: vec![empty_path],
            ..Config::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn serde_round_trip_and_numeric_variants_and_overflow_checks() {
        let mut rule = minimal_rule("roundtrip", PathBuf::from("/tmp/logs"));
        rule.min_file_size = Some(64);
        rule.max_file_size = Some(8192);
        rule.max_age = Some(Duration::from_millis(1500));
        rule.max_total_size = Some(4096);
        rule.backup_guard.min_valid_backup_bytes = 512;
        rule.backup_guard.require_newer_within = Some(Duration::from_secs(3600));
        rule.backup_guard.freeze_if_newest_older_than = Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        rule.actions = vec![
            ActionStep::RenameByDate,
            ActionStep::Compress,
            ActionStep::CopyTruncate,
            ActionStep::Delete,
        ];
        rule.copy_truncate_to = Some(PathBuf::from("/tmp/rotated"));
        rule.move_date_subdir = Some("%Y/%m".to_string());
        rule.compression = CompressionFormat::Zip;
        rule.combine_mode = CombineMode::All;
        rule.eviction_order = EvictionOrder::LargestFirst;

        let cfg = Config {
            dry_run: true,
            audit_log: Some(PathBuf::from("/tmp/audit.jsonl")),
            global_trash_dir: Some(PathBuf::from("/tmp/trash")),
            rules: vec![rule],
            ..Config::default()
        };

        let yaml_text = serde_yaml_ng::to_string(&cfg).unwrap();
        let yaml_back: Config = serde_yaml_ng::from_str(&yaml_text).unwrap();
        assert_eq!(yaml_back, cfg);

        let toml_text = toml::to_string(&cfg).unwrap();
        let toml_back: Config = toml::from_str(&toml_text).unwrap();
        assert_eq!(toml_back, cfg);
        assert_eq!(
            toml_back.effective_audit_log(),
            Some(Path::new("/tmp/audit.jsonl"))
        );

        // Numeric integer and null YAML forms exercise visitor branches.
        let numeric_yaml = r#"
daemon:
  interval: 45
  debounce: 3
rules:
  - name: numeric-rule
    path: /tmp/numeric
    max_age: null
    max_total_size: 2048
    backup_guard:
      min_valid_backup_bytes: 128
      require_newer_within: 120
"#;
        let numeric_cfg: Config = serde_yaml_ng::from_str(numeric_yaml).unwrap();
        assert_eq!(numeric_cfg.daemon.interval, Duration::from_secs(45));
        assert_eq!(numeric_cfg.rules[0].max_age, None);
        assert_eq!(numeric_cfg.rules[0].max_total_size, Some(2048));
        assert_eq!(
            numeric_cfg.rules[0].backup_guard.min_valid_backup_bytes,
            128
        );
        assert_eq!(
            numeric_cfg.rules[0].backup_guard.require_newer_within,
            Some(Duration::from_secs(120))
        );

        let negative_bytes_yaml = r#"
rules:
  - name: neg-bytes
    path: /tmp/neg
    max_total_size: -1
"#;
        assert!(serde_yaml_ng::from_str::<Config>(negative_bytes_yaml).is_err());

        let negative_duration_yaml = r#"
rules:
  - name: neg-dur
    path: /tmp/neg
    max_age: -5
"#;
        assert!(serde_yaml_ng::from_str::<Config>(negative_duration_yaml).is_err());

        assert!(parse_bytes("99999999999999999999999999999 GiB").is_err());
        assert!(parse_duration_str("9999999999999999999999999 w").is_err());
    }

    #[test]
    fn validate_and_parser_error_branches_cover_edge_cases() {
        let tmp = TempDir::new().unwrap();
        let bad_toml = tmp.path().join("bad.toml");
        fs::write(&bad_toml, "rules = [not valid").unwrap();
        assert!(matches!(
            Config::from_file(&bad_toml),
            Err(FileCleanerError::ConfigParse { .. })
        ));

        let bad_yaml = tmp.path().join("bad.yaml");
        fs::write(&bad_yaml, "rules:\n  - [broken").unwrap();
        assert!(matches!(
            Config::from_file(&bad_yaml),
            Err(FileCleanerError::ConfigParse { .. })
        ));

        // Invalid type errors exercise serde `expecting()` branches.
        assert!(serde_yaml_ng::from_str::<Config>(
            "rules:\n  - name: t\n    path: /tmp\n    max_total_size: [1, 2]\n"
        )
        .is_err());
        assert!(serde_yaml_ng::from_str::<Config>(
            "rules:\n  - name: t\n    path: /tmp\n    max_age: [1, 2]\n"
        )
        .is_err());
        assert!(serde_yaml_ng::from_str::<Config>(
            "daemon:\n  interval: [1, 2]\nrules:\n  - name: t\n    path: /tmp\n"
        )
        .is_err());
        assert!(serde_yaml_ng::from_str::<Config>(
            "rules:\n  - name: t\n    path: /tmp\n    backup_guard:\n      min_valid_backup_bytes: -1\n"
        )
        .is_err());
        assert!(serde_yaml_ng::from_str::<Config>(
            "daemon:\n  interval: -5\nrules:\n  - name: t\n    path: /tmp\n"
        )
        .is_err());

        // Empty global_trash_dir, invalid exclude_glob, include_regex, filename_date_regex,
        // move_to == recursive root, and exclude_regex exclusion.
        let mut base = minimal_rule("edge", tmp.path().join("root"));
        fs::create_dir_all(&base.path).unwrap();

        let empty_global = Config {
            global_trash_dir: Some(PathBuf::new()),
            rules: vec![base.clone()],
            ..Config::default()
        };
        assert!(empty_global.validate().is_err());

        let mut bad_ex_glob = base.clone();
        bad_ex_glob.exclude_globs = vec!["[bad".to_string()];
        assert!(matches!(
            Config {
                rules: vec![bad_ex_glob],
                ..Config::default()
            }
            .validate(),
            Err(FileCleanerError::InvalidGlob { .. })
        ));

        let mut bad_inc_re = base.clone();
        bad_inc_re.include_regex = Some("(bad".to_string());
        assert!(matches!(
            Config {
                rules: vec![bad_inc_re],
                ..Config::default()
            }
            .validate(),
            Err(FileCleanerError::InvalidRegex { .. })
        ));

        let mut bad_fn_re = base.clone();
        bad_fn_re.time_basis = TimeBasis::FilenameDate;
        bad_fn_re.filename_date_regex = Some("(bad".to_string());
        bad_fn_re.filename_date_format = Some("%Y".to_string());
        assert!(matches!(
            Config {
                rules: vec![bad_fn_re],
                ..Config::default()
            }
            .validate(),
            Err(FileCleanerError::InvalidRegex { .. })
        ));

        let mut same_dest = base.clone();
        same_dest.recursive = true;
        same_dest.actions = vec![ActionStep::Move];
        same_dest.move_to = Some(same_dest.path.clone());
        assert!(Config {
            rules: vec![same_dest],
            ..Config::default()
        }
        .validate()
        .is_err());

        // Relative path normalization + exclude_regex acceptance.
        base.recursive = true;
        base.actions = vec![ActionStep::Move];
        base.move_to = Some(base.path.join("subdir/../cold"));
        base.exclude_regex = Some("^cold(/|$)".to_string());
        assert!(Config {
            rules: vec![base],
            ..Config::default()
        }
        .validate()
        .is_ok());

        // Global trash dir inside recursive path without exclusion should fail, too.
        let mut uses_global_trash = minimal_rule("global-trash", tmp.path().join("root"));
        uses_global_trash.recursive = true;
        uses_global_trash.actions = vec![ActionStep::Trash];
        assert!(Config {
            global_trash_dir: Some(tmp.path().join("root/nested_trash")),
            rules: vec![uses_global_trash],
            ..Config::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn spec_alignment_aliases_and_new_validation_invariants() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("logs");
        let rotated = tmp.path().join("rotated");
        fs::create_dir_all(&root).unwrap();

        let alias_toml = format!(
            r#"
dry_run = true
audit_log = "/var/log/filecleaner/audit.jsonl"

[daemon]
mode = "notify"
poll_interval = "90s"
sweep_on_start = false

[[rules]]
name = "nginx-logs"
path = "{}"
min_file_size = "1 KiB"
max_file_size = "50 MiB"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = false
compress_overwrite = true
move_to = "{}"
move_date_subdir = "%Y/%m"
move_overwrite = true

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_size = "512 B"
max_newest_age = "12h"
min_newest_timestamp = "2026-01-01T00:00:00Z"
freeze_on_empty = false

[rules.pre_hook]
command = "/usr/bin/true"
run_in_dry_run = true
"#,
            root.display().to_string().replace('\\', "\\\\"),
            rotated.display().to_string().replace('\\', "\\\\")
        );

        let parsed: Config = toml::from_str(&alias_toml).unwrap();
        parsed.validate().unwrap();
        assert!(parsed.dry_run);
        assert_eq!(parsed.daemon.mode, DaemonMode::Notify);
        assert_eq!(parsed.daemon.interval, Duration::from_secs(90));
        assert!(!parsed.daemon.sweep_on_start);
        assert_eq!(parsed.rules[0].min_file_size, Some(1024));
        assert_eq!(parsed.rules[0].max_file_size, Some(50 * 1024 * 1024));
        assert!(!parsed.rules[0].compress_remove_source);
        assert!(parsed.rules[0].compress_overwrite);
        assert!(parsed.rules[0].move_overwrite);
        assert_eq!(parsed.rules[0].move_date_subdir.as_deref(), Some("%Y/%m"));
        assert_eq!(parsed.rules[0].backup_guard.min_valid_backup_bytes, 512);
        assert_eq!(
            parsed.rules[0].backup_guard.require_newer_within,
            Some(Duration::from_secs(12 * 3600))
        );
        assert!(parsed.rules[0]
            .backup_guard
            .freeze_if_newest_older_than
            .is_some());
        assert!(!parsed.rules[0].backup_guard.freeze_on_empty_valid);
        assert!(
            parsed.rules[0]
                .pre_delete_hook
                .as_ref()
                .unwrap()
                .run_in_dry_run
        );

        // min_file_size > max_file_size rejected.
        let mut size_conflict = minimal_rule("size-conflict", root.clone());
        size_conflict.min_file_size = Some(2048);
        size_conflict.max_file_size = Some(1024);
        assert!(Config {
            rules: vec![size_conflict],
            ..Config::default()
        }
        .validate()
        .is_err());

        // CopyTruncate requires copy_truncate_to or move_to and forbids unexcluded recursive nesting.
        let mut missing_ct = minimal_rule("missing-ct", root.clone());
        missing_ct.actions = vec![ActionStep::CopyTruncate];
        assert!(Config {
            rules: vec![missing_ct],
            ..Config::default()
        }
        .validate()
        .is_err());

        let mut nested_ct = minimal_rule("nested-ct", root.clone());
        nested_ct.recursive = true;
        nested_ct.actions = vec![ActionStep::CopyTruncate];
        nested_ct.copy_truncate_to = Some(root.join("inner"));
        assert!(Config {
            rules: vec![nested_ct],
            ..Config::default()
        }
        .validate()
        .is_err());

        // Terminal Delete/Trash cannot precede another step.
        let mut bad_pipeline = minimal_rule("bad-pipeline", root.clone());
        bad_pipeline.actions = vec![ActionStep::Delete, ActionStep::Compress];
        assert!(Config {
            rules: vec![bad_pipeline],
            ..Config::default()
        }
        .validate()
        .is_err());

        let mut bad_trash_pipeline = minimal_rule("bad-trash-pipeline", root.clone());
        bad_trash_pipeline.trash_dir = Some(rotated.clone());
        bad_trash_pipeline.actions = vec![ActionStep::Trash, ActionStep::Move];
        bad_trash_pipeline.move_to = Some(rotated);
        assert!(Config {
            rules: vec![bad_trash_pipeline],
            ..Config::default()
        }
        .validate()
        .is_err());

        // Invalid move_date_subdir rejected.
        let mut bad_subdir = minimal_rule("bad-subdir", root.clone());
        bad_subdir.move_date_subdir = Some("%Q".to_string());
        assert!(Config {
            rules: vec![bad_subdir],
            ..Config::default()
        }
        .validate()
        .is_err());

        // Zero daemon interval rejected.
        let mut zero_daemon = Config {
            rules: vec![minimal_rule("ok", root.clone())],
            ..Config::default()
        };
        zero_daemon.daemon.interval = Duration::ZERO;
        assert!(zero_daemon.validate().is_err());

        // Invalid filename_date_regex is rejected even when time_basis is Modified.
        let mut bad_unused_fn_regex = minimal_rule("unused-fn-re", root.clone());
        bad_unused_fn_regex.time_basis = TimeBasis::Modified;
        bad_unused_fn_regex.filename_date_regex = Some("(unclosed".to_string());
        assert!(matches!(
            Config {
                rules: vec![bad_unused_fn_regex],
                ..Config::default()
            }
            .validate(),
            Err(FileCleanerError::InvalidRegex { .. })
        ));

        // Invalid or empty rename_template is rejected eagerly by Config::validate().
        for bad_tpl in ["", "   ", "%Q"] {
            let mut bad_rename_rule = minimal_rule("bad-rename", root.clone());
            bad_rename_rule.actions = vec![ActionStep::RenameByDate];
            bad_rename_rule.rename_template = Some(bad_tpl.to_string());
            assert!(Config {
                rules: vec![bad_rename_rule],
                ..Config::default()
            }
            .validate()
            .is_err());
        }

        // Empty audit_log and full TOML/YAML serialize round-trip (covering subsecond & None serde paths).
        let mut bad_audit = Config {
            audit_log: Some(PathBuf::new()),
            rules: vec![minimal_rule("ok", root.clone())],
            ..Config::default()
        };
        assert!(bad_audit.validate().is_err());

        bad_audit.audit_log = None;
        bad_audit.daemon.debounce = Duration::from_millis(250);
        bad_audit.rules[0].max_file_size = Some(4096);
        bad_audit.rules[0].rename_template = Some("%Y-%m-%d_%H%M%S".to_string());
        let toml_ser = toml::to_string(&bad_audit).unwrap();
        let yaml_ser = serde_yaml_ng::to_string(&bad_audit).unwrap();
        let toml_back: Config = toml::from_str(&toml_ser).unwrap();
        let yaml_back: Config = serde_yaml_ng::from_str(&yaml_ser).unwrap();
        toml_back.validate().unwrap();
        yaml_back.validate().unwrap();
        assert_eq!(toml_back.daemon.debounce, Duration::from_millis(250));
        assert_eq!(yaml_back.rules[0].max_file_size, Some(4096));

        // Relative `.` / `..` lexical path resolution and regex directory exclusion inside recursive tree.
        let cwd = std::env::current_dir().unwrap();
        let lex = resolve_path_lexically(Path::new("./sub/../missing_leaf"));
        assert!(lex.starts_with(&cwd) || lex.ends_with("missing_leaf"));

        let mut regex_excl_rule = minimal_rule("regex-excl", root.clone());
        regex_excl_rule.recursive = true;
        regex_excl_rule.actions = vec![ActionStep::Move];
        regex_excl_rule.move_to = Some(root.join("archive/2026"));
        regex_excl_rule.exclude_regex = Some("^archive(/|$)".to_string());
        Config {
            rules: vec![regex_excl_rule],
            ..Config::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn s3_config_parses_toml_and_yaml_and_enforces_invariants_10_to_12() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("wal");
        fs::create_dir_all(&dir).unwrap();
        let dir_esc = dir.display().to_string().replace('\\', "\\\\");

        // Valid TOML using global [s3] fallback and rule-level override.
        let toml_path = tmp.path().join("s3.toml");
        fs::write(
            &toml_path,
            format!(
                r#"
[s3]
bucket = "global-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
force_path_style = true
verify_mode = "checksum_sha256"
multipart_part_size = "16MiB"
timeout = "45s"

[[rules]]
name = "pg-wal"
path = "{dir_esc}"
actions = ["compress", "upload_s3", "delete"]

[[rules]]
name = "pg-dumps"
path = "{dir_esc}"
actions = ["UploadS3", "move"]
move_to = "/var/archive"

[rules.s3]
bucket = "dumps-bucket"
region = "us-east-1"
endpoint = "http://127.0.0.1:9000"
allow_insecure_http = true
prefix = "postgres/dumps"
key_date_subdir = "%Y/%m/%d"
storage_class = "STANDARD_IA"
verify_mode = "readback_sha256"
access_key_env = "PG_S3_KEY"
secret_key_env = "PG_S3_SECRET"
session_token_env = "PG_S3_TOKEN"
multipart_part_size = "5MiB"
"#
            ),
        )
        .unwrap();
        let cfg = Config::load_validated(&toml_path).unwrap();
        assert_eq!(
            cfg.effective_s3(&cfg.rules[0]).unwrap().bucket,
            "global-archive"
        );
        let r1_s3 = cfg.effective_s3(&cfg.rules[1]).unwrap();
        assert_eq!(r1_s3.bucket, "dumps-bucket");
        assert_eq!(r1_s3.verify_mode, S3VerifyMode::ReadbackSha256);
        assert_eq!(r1_s3.multipart_part_size, MIN_S3_MULTIPART_PART_SIZE);

        // Valid YAML with global s3 + UploadS3 before Trash.
        let yaml_path = tmp.path().join("s3.yaml");
        fs::write(
            &yaml_path,
            format!(
                r#"
global_trash_dir: /var/tmp/trash
s3:
  bucket: yaml-archive
  region: eu-central-1
rules:
  - name: yaml-s3
    path: "{dir_esc}"
    actions:
      - upload_s3
      - trash
"#
            ),
        )
        .unwrap();
        let ycfg = Config::load_validated(&yaml_path).unwrap();
        assert_eq!(
            ycfg.effective_s3(&ycfg.rules[0]).unwrap().access_key_env,
            "AWS_ACCESS_KEY_ID"
        );

        // INV-CFG-10: UploadS3 without global or rule s3 is rejected.
        let mut missing_s3 = minimal_rule("no-s3", dir.clone());
        missing_s3.actions = vec![ActionStep::UploadS3, ActionStep::Delete];
        assert!(Config {
            rules: vec![missing_s3],
            ..Config::default()
        }
        .validate()
        .is_err());

        // INV-CFG-11 & INV-CFG-12 invalid mutations rejected on S3Config.
        let valid_s3 = S3Config {
            bucket: "ok-bucket".to_string(),
            region: "us-east-1".to_string(),
            ..S3Config::default()
        };
        let invalid_cases: Vec<S3Config> = vec![
            S3Config {
                bucket: "   ".into(),
                ..valid_s3.clone()
            },
            S3Config {
                bucket: "bad/bucket".into(),
                ..valid_s3.clone()
            },
            S3Config {
                region: "".into(),
                ..valid_s3.clone()
            },
            S3Config {
                access_key_env: "BAD=NAME".into(),
                ..valid_s3.clone()
            },
            S3Config {
                secret_key_env: "".into(),
                ..valid_s3.clone()
            },
            S3Config {
                session_token_env: Some("  ".into()),
                ..valid_s3.clone()
            },
            S3Config {
                endpoint: Some("http://minio.local:9000".into()),
                allow_insecure_http: false,
                ..valid_s3.clone()
            },
            S3Config {
                endpoint: Some("ftp://minio.local".into()),
                ..valid_s3.clone()
            },
            S3Config {
                endpoint: Some("https://".into()),
                ..valid_s3.clone()
            },
            S3Config {
                prefix: Some("a/../b".into()),
                ..valid_s3.clone()
            },
            S3Config {
                key_date_subdir: Some("%Q".into()),
                ..valid_s3.clone()
            },
            S3Config {
                key_date_subdir: Some("../%Y".into()),
                ..valid_s3.clone()
            },
            S3Config {
                storage_class: Some("STANDARD\r\nX-Inject: 1".into()),
                ..valid_s3.clone()
            },
            S3Config {
                timeout: Duration::ZERO,
                ..valid_s3.clone()
            },
            S3Config {
                multipart_part_size: MIN_S3_MULTIPART_PART_SIZE - 1,
                ..valid_s3
            },
        ];

        for (idx, bad) in invalid_cases.into_iter().enumerate() {
            let mut r = minimal_rule("bad-s3", dir.clone());
            r.actions = vec![ActionStep::UploadS3];
            r.s3 = Some(bad.clone());
            assert!(
                Config {
                    rules: vec![r],
                    ..Config::default()
                }
                .validate()
                .is_err(),
                "expected invalid rule S3Config case #{idx} to fail validation"
            );
            assert!(
                Config {
                    s3: Some(bad),
                    rules: vec![minimal_rule("ok", dir.clone())],
                    ..Config::default()
                }
                .validate()
                .is_err(),
                "expected invalid global S3Config case #{idx} to fail validation"
            );
        }
    }
}
