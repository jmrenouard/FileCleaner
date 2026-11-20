//! Domain error types and result alias for FileCleaner.

use std::path::PathBuf;
use thiserror::Error;

/// Unified result alias used across all FileCleaner subsystems.
pub type Result<T> = std::result::Result<T, FileCleanerError>;

/// Structured error variants emitted by configuration parsing, validation,
/// filesystem probes, lifecycle hooks, transforms, and daemon control.
#[derive(Debug, Error)]
pub enum FileCleanerError {
    /// Failed to parse a TOML or YAML configuration file.
    #[error("failed to parse configuration file '{path}': {reason}")]
    ConfigParse {
        /// Path of the configuration file that failed to parse.
        path: PathBuf,
        /// Underlying parser diagnostic message.
        reason: String,
    },

    /// Configuration file extension is neither TOML nor YAML.
    #[error("unsupported configuration format for '{path}': expected .toml, .yaml, or .yml (got '{extension}')")]
    ConfigFormat {
        /// Path with an unrecognized extension.
        path: PathBuf,
        /// Extracted extension (empty if missing).
        extension: String,
    },

    /// Configuration semantic invariant failed validation.
    #[error("configuration validation error: {0}")]
    ConfigValidation(String),

    /// Invalid glob pattern in include or exclude rules.
    #[error("invalid glob pattern '{pattern}': {source}")]
    InvalidGlob {
        /// Raw glob pattern string.
        pattern: String,
        /// Underlying `globset` compilation error.
        #[source]
        source: globset::Error,
    },

    /// Invalid regular expression in include, exclude, or filename-date rules.
    #[error("invalid regular expression '{pattern}': {source}")]
    InvalidRegex {
        /// Raw regular expression string.
        pattern: String,
        /// Underlying `regex` compilation error.
        #[source]
        source: regex::Error,
    },

    /// Invalid human-readable duration literal.
    #[error("invalid duration '{0}'")]
    InvalidDuration(String),

    /// Invalid human-readable byte size literal.
    #[error("invalid byte size '{0}'")]
    InvalidByteSize(String),

    /// Underlying filesystem or OS I/O error bound to a target path.
    #[error("I/O error on '{path}': {source}")]
    Io {
        /// Path associated with the failing I/O call.
        path: PathBuf,
        /// Underlying `std::io::Error`.
        #[source]
        source: std::io::Error,
    },

    /// Failed to serialize structured output, state, or audit records.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Pre-action or pre-delete hook failed with a non-zero non-skip status.
    #[error(
        "hook failed for rule '{rule}' running '{command}' (exit code {exit_code:?}): {stderr}"
    )]
    HookFailed {
        /// Rule name owning the hook.
        rule: String,
        /// Command executable that ran.
        command: String,
        /// Exit code if the process exited normally.
        exit_code: Option<i32>,
        /// Captured standard error output.
        stderr: String,
    },

    /// Pre-action or pre-delete hook exceeded its configured timeout.
    #[error("hook timed out after {timeout_secs}s for rule '{rule}' running '{command}'")]
    HookTimedOut {
        /// Rule name owning the hook.
        rule: String,
        /// Command executable that timed out.
        command: String,
        /// Configured timeout threshold in seconds.
        timeout_secs: u64,
    },

    /// In-use probe error when querying OS lock or descriptor state.
    #[error("in-use probe failure: {0}")]
    InUseProbe(String),

    /// Compression or archive transformation failed.
    #[error("compression failure on '{path}': {reason}")]
    Compression {
        /// Source or destination archive path.
        path: PathBuf,
        /// Underlying codec error description.
        reason: String,
    },

    /// Cryptographic SHA-256 verification failed during cross-filesystem move.
    #[error(
        "integrity verification failed on '{path}': expected SHA-256 {expected}, got {actual}"
    )]
    IntegrityMismatch {
        /// Target file path failing digest verification.
        path: PathBuf,
        /// Expected SHA-256 hex digest computed before copy.
        expected: String,
        /// Actual SHA-256 hex digest computed from destination copy.
        actual: String,
    },

    /// Backup safety guardrail blocked or detected a critical invariant violation.
    #[error("backup guard violation on rule '{rule}': {reason}")]
    BackupGuardViolation {
        /// Rule name where the violation occurred.
        rule: String,
        /// Detailed explanation of the guardrail violation.
        reason: String,
    },

    /// Daemon lifecycle, PID, signal, or filesystem watcher error.
    #[error("daemon error: {0}")]
    Daemon(String),
}

impl FileCleanerError {
    /// Constructs a path-contextualized I/O error.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn io_helper_preserves_path_and_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let err = FileCleanerError::io("/var/backups/db.sql", io_err);
        let msg = err.to_string();
        assert!(msg.contains("/var/backups/db.sql"));
        assert!(msg.contains("access denied"));
        assert!(err.source().is_some());
    }

    #[test]
    fn error_variants_display_human_readable_diagnostics() {
        let err = FileCleanerError::ConfigValidation("rules list must not be empty".to_string());
        assert_eq!(
            err.to_string(),
            "configuration validation error: rules list must not be empty"
        );

        let integrity = FileCleanerError::IntegrityMismatch {
            path: PathBuf::from("/tmp/out.gz"),
            expected: "aaa".to_string(),
            actual: "bbb".to_string(),
        };
        assert!(integrity.to_string().contains("expected SHA-256 aaa"));
        assert!(integrity.to_string().contains("got bbb"));

        let timeout = FileCleanerError::HookTimedOut {
            rule: "nightly".to_string(),
            command: "/usr/local/bin/check.sh".to_string(),
            timeout_secs: 15,
        };
        assert!(timeout.to_string().contains("15s"));
        assert!(timeout.to_string().contains("nightly"));
    }
}
