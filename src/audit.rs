//! Structured UTF-8 JSONL audit logging and CLI table/JSON summary formatters.

use crate::config::AuditConfig;
use crate::error::{FileCleanerError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Default maximum audit log size in bytes (`50 MiB`) before rotating to `<path>.1`.
pub const DEFAULT_AUDIT_ROTATE_BYTES: u64 = 50 * 1024 * 1024;

/// Event classification emitted into FileCleaner's structured JSONL audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventKind {
    /// Start of a sweep invocation.
    SweepStart,
    /// Completion of a sweep invocation.
    SweepEnd,
    /// Per-file evaluation, retention, protection, or execution decision.
    FileDecision,
    /// Backup safety guardrail staleness or empty-valid freeze trigger.
    BackupFreeze,
    /// Rule ended with unsatisfied retention caps due to safety constraints.
    RuleConstrained,
    /// Synchronous pre-delete / pre-action hook execution record.
    HookExec,
    /// Action pipeline execution record.
    ActionExec,
    /// Operational, I/O, hook, or integrity error event.
    Error,
}

/// Self-contained UTF-8 JSONL audit record aligned with `SPECIFICATIONS.md` §9.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Event timestamp in UTC (serialized as RFC 3339).
    pub ts: DateTime<Utc>,
    /// Event classification.
    pub event: AuditEventKind,
    /// Unique identifier of the sweep execution run.
    pub run_id: String,
    /// Rule identifier associated with this event, if applicable.
    pub rule: Option<String>,
    /// Target candidate file path, if applicable.
    pub path: Option<PathBuf>,
    /// Target candidate size in bytes, if applicable.
    pub size_bytes: Option<u64>,
    /// Resolved `TimeBasis` timestamp in UTC, if applicable.
    pub resolved_ts: Option<DateTime<Utc>>,
    /// Decision outcome (`keep_floor`, `keep_retention`, `skip_in_use`, `skip_hook`, `freeze`, `would_act`, `acted`, `aborted`, `error`).
    pub decision: Option<String>,
    /// Active probe reasons when skipped by in-use detection.
    pub in_use_reasons: Option<Vec<String>>,
    /// Ordered lifecycle step labels executed (`RenameByDate`, `Compress(Gzip)`, `Move`, etc.).
    #[serde(default)]
    pub actions: Vec<String>,
    /// Optional single-string action summary (`"compress(Gzip)->move"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Final output destination path produced by `RenameByDate`/`Compress`/`Move`/`Trash`/`CopyTruncate`.
    pub dest_path: Option<PathBuf>,
    /// Verified lowercase hex SHA-256 digest when cross-FS `Move`/`Trash`/`CopyTruncate` ran.
    pub sha256: Option<String>,
    /// Whether dry-run simulation mode was active.
    pub dry_run: bool,
    /// Elapsed operation duration in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Structured error or guardrail diagnostic message.
    pub error: Option<String>,
    /// Optional extensible structured metadata payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl AuditEntry {
    /// Creates a new timestamped [`AuditEntry`] with sensible defaults.
    pub fn new(event: AuditEventKind, run_id: impl Into<String>, dry_run: bool) -> Self {
        Self {
            ts: Utc::now(),
            event,
            run_id: run_id.into(),
            rule: None,
            path: None,
            size_bytes: None,
            resolved_ts: None,
            decision: None,
            in_use_reasons: None,
            actions: Vec::new(),
            action: None,
            dest_path: None,
            sha256: None,
            dry_run,
            duration_ms: 0,
            error: None,
            details: None,
        }
    }

    /// Attaches a rule name to this entry.
    pub fn with_rule(mut self, rule: impl Into<String>) -> Self {
        self.rule = Some(rule.into());
        self
    }

    /// Attaches candidate file path and byte size to this entry.
    pub fn with_candidate(mut self, path: impl Into<PathBuf>, size_bytes: u64) -> Self {
        self.path = Some(path.into());
        self.size_bytes = Some(size_bytes);
        self
    }

    /// Attaches a resolved `TimeBasis` timestamp to this entry.
    pub fn with_resolved_ts(mut self, resolved_ts: DateTime<Utc>) -> Self {
        self.resolved_ts = Some(resolved_ts);
        self
    }

    /// Attaches a decision label to this entry.
    pub fn with_decision(mut self, decision: impl Into<String>) -> Self {
        self.decision = Some(decision.into());
        self
    }

    /// Attaches active in-use probe trigger strings to this entry.
    pub fn with_in_use_reasons(mut self, reasons: Vec<String>) -> Self {
        self.in_use_reasons = Some(reasons);
        self
    }

    /// Attaches executed step labels and optional summary string to this entry.
    pub fn with_actions(mut self, actions: Vec<String>) -> Self {
        self.action = if actions.is_empty() {
            None
        } else {
            Some(actions.join("->"))
        };
        self.actions = actions;
        self
    }

    /// Attaches a destination output path to this entry.
    pub fn with_dest_path(mut self, dest_path: impl Into<PathBuf>) -> Self {
        self.dest_path = Some(dest_path.into());
        self
    }

    /// Attaches a verified SHA-256 hex digest to this entry.
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    /// Attaches an elapsed duration in milliseconds to this entry.
    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    /// Attaches an error or guardrail diagnostic message to this entry.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Attaches arbitrary structured JSON metadata to this entry.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

struct AuditSinkState {
    writer: Option<Box<dyn Write + Send>>,
    current_bytes: u64,
    max_bytes: u64,
}

/// Thread-safe synchronous UTF-8 JSONL audit writer.
pub struct AuditLogger {
    sink: Option<Mutex<AuditSinkState>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for AuditLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLogger")
            .field("enabled", &self.sink.is_some())
            .field("path", &self.path)
            .finish()
    }
}

impl AuditLogger {
    /// Creates a disabled no-op logger that accepts [`AuditLogger::log`] calls without writing.
    pub fn noop() -> Self {
        Self {
            sink: None,
            path: None,
        }
    }

    /// Opens or creates a JSONL audit log file in append mode with `50 MiB` `.1` rotation.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_max_bytes(path, DEFAULT_AUDIT_ROTATE_BYTES)
    }

    /// Opens or creates a JSONL audit log file with private permissions (`0o600` on Unix)
    /// and single-generation `.1` rotation when reaching `max_bytes`.
    pub fn open_with_max_bytes(path: impl AsRef<Path>, max_bytes: u64) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| FileCleanerError::io(parent, err))?;
            }
        }
        let file = open_private_audit_file(path, false)?;
        let current_bytes = file
            .metadata()
            .map_err(|err| FileCleanerError::io(path, err))?
            .len();

        let mut state = AuditSinkState {
            writer: Some(Box::new(BufWriter::new(file))),
            current_bytes,
            max_bytes,
        };
        if max_bytes > 0 && state.current_bytes >= max_bytes {
            rotate_audit_file(path, &mut state)?;
        }
        Ok(Self {
            sink: Some(Mutex::new(state)),
            path: Some(path.to_path_buf()),
        })
    }

    /// Initializes an [`AuditLogger`] from [`AuditConfig`] and an optional fallback top-level `audit_log` path.
    pub fn from_config(audit: &AuditConfig, fallback_path: Option<&Path>) -> Result<Self> {
        if !audit.jsonl {
            return Ok(Self::noop());
        }
        match audit.log_path.as_deref().or(fallback_path) {
            Some(path) => Self::open(path),
            None => Ok(Self::noop()),
        }
    }

    /// Wraps an arbitrary `Write + Send` sink for deterministic testing or stdout streaming.
    pub fn from_writer(writer: impl Write + Send + 'static) -> Self {
        Self {
            sink: Some(Mutex::new(AuditSinkState {
                writer: Some(Box::new(writer)),
                current_bytes: 0,
                max_bytes: 0,
            })),
            path: None,
        }
    }

    /// Returns the configured output file path if this logger writes to disk.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns `true` when this logger has an active output sink.
    pub fn is_enabled(&self) -> bool {
        self.sink.is_some()
    }

    /// Serializes `entry` as a single UTF-8 JSON line terminated by `\n` and flushes synchronously.
    pub fn log(&self, entry: &AuditEntry) -> Result<()> {
        let Some(mutex) = &self.sink else {
            return Ok(());
        };

        let mut line = serde_json::to_string(entry)
            .map_err(|err| FileCleanerError::Serialization(err.to_string()))?;
        line.push('\n');

        let mut guard = mutex.lock().map_err(|err| {
            FileCleanerError::Serialization(format!("audit logger mutex poisoned: {err}"))
        })?;
        if let Some(path) = self.path.as_deref() {
            if guard.max_bytes > 0 && guard.current_bytes >= guard.max_bytes {
                rotate_audit_file(path, &mut guard)?;
            } else if guard.writer.is_none() {
                let file = open_private_audit_file(path, false)?;
                guard.writer = Some(Box::new(BufWriter::new(file)));
            }
        }
        let writer = guard.writer.as_mut().ok_or_else(|| {
            self.wrap_io_error(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "audit writer is unavailable",
            ))
        })?;
        writer
            .write_all(line.as_bytes())
            .map_err(|err| self.wrap_io_error(err))?;
        writer.flush().map_err(|err| self.wrap_io_error(err))?;
        guard.current_bytes = guard.current_bytes.saturating_add(line.len() as u64);
        Ok(())
    }

    fn wrap_io_error(&self, err: std::io::Error) -> FileCleanerError {
        let path = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<audit-writer>"));
        FileCleanerError::io(path, err)
    }
}

fn open_private_audit_file(path: &Path, truncate: bool) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true);
    if truncate {
        opts.write(true).truncate(true);
    } else {
        opts.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts
        .open(path)
        .map_err(|err| FileCleanerError::io(path, err))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|err| FileCleanerError::io(path, err))?;
    }
    Ok(file)
}

fn rotate_audit_file(path: &Path, state: &mut AuditSinkState) -> Result<()> {
    if let Some(writer) = state.writer.as_mut() {
        writer
            .flush()
            .map_err(|err| FileCleanerError::io(path, err))?;
    }
    drop(state.writer.take());

    let mut rotated_os = path.as_os_str().to_os_string();
    rotated_os.push(".1");
    let rotated_path = PathBuf::from(rotated_os);

    fs::rename(path, &rotated_path).map_err(|err| FileCleanerError::io(&rotated_path, err))?;

    let fresh_file = open_private_audit_file(path, true)?;
    state.writer = Some(Box::new(BufWriter::new(fresh_file)));
    state.current_bytes = 0;
    Ok(())
}

/// Aggregated per-rule execution summary used for CLI tables and `--json` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSummaryRow {
    /// Rule name.
    pub rule: String,
    /// Rule outcome label (`ok`, `frozen`, `constrained`, `aborted`, `skipped`).
    pub outcome: String,
    /// Total candidate files scanned.
    pub evaluated: usize,
    /// Files executed (live) or projected (`would_act` in dry-run).
    pub acted: usize,
    /// Files skipped because in-use probes reported busy.
    pub skipped_in_use: usize,
    /// Files skipped due to pre-delete hook veto (`skip_exit_code`) or `AbortFile`.
    pub skipped_hook: usize,
    /// Valid backup files protected by `backup_guard.min_keep_files`.
    pub kept_floor: usize,
    /// Total bytes freed in live mode.
    pub bytes_freed: u64,
    /// Whether dry-run mode was active for this rule.
    pub dry_run: bool,
    /// Optional diagnostic error or freeze explanation.
    pub error: Option<String>,
}

/// Top-level sweep summary across all rules in a single run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepSummary {
    /// Unique sweep execution identifier.
    pub run_id: String,
    /// Whether global dry-run mode was active.
    pub dry_run: bool,
    /// Ordered per-rule execution rows.
    pub rules: Vec<RuleSummaryRow>,
    /// Total candidates evaluated across all rules.
    pub total_evaluated: usize,
    /// Total candidates acted or projected across all rules.
    pub total_acted: usize,
    /// Total bytes freed across all rules.
    pub total_bytes_freed: u64,
}

impl SweepSummary {
    /// Aggregates per-rule summary rows into a [`SweepSummary`].
    pub fn from_rows(run_id: impl Into<String>, dry_run: bool, rules: Vec<RuleSummaryRow>) -> Self {
        let total_evaluated = rules.iter().map(|r| r.evaluated).sum();
        let total_acted = rules.iter().map(|r| r.acted).sum();
        let total_bytes_freed = rules.iter().map(|r| r.bytes_freed).sum();
        Self {
            run_id: run_id.into(),
            dry_run,
            rules,
            total_evaluated,
            total_acted,
            total_bytes_freed,
        }
    }

    /// Serializes this sweep summary as pretty-printed JSON (`--json`).
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|err| FileCleanerError::Serialization(err.to_string()))
    }

    /// Serializes this sweep summary as compact single-line JSON.
    pub fn to_json_compact(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|err| FileCleanerError::Serialization(err.to_string()))
    }

    /// Formats an aligned human-readable summary table for terminal display.
    pub fn format_table(&self) -> String {
        let mut rule_w = "Rule".len();
        let mut outcome_w = "Outcome".len();
        for row in &self.rules {
            rule_w = rule_w.max(row.rule.len());
            outcome_w = outcome_w.max(row.outcome.len());
        }

        let mut out = format!(
            "Run: {} | Mode: {}\n",
            self.run_id,
            if self.dry_run { "DRY-RUN" } else { "LIVE" }
        );
        out.push_str(&format!(
            "{:<rule_w$}  {:<outcome_w$}  {:>9}  {:>6}  {:>5}  {:>8}  {:>5}  {:>10}  {}\n",
            "Rule", "Outcome", "Evaluated", "Acted", "Busy", "HookSkip", "Floor", "Freed", "Mode"
        ));
        let sep_len = rule_w + outcome_w + 68;
        out.push_str(&"-".repeat(sep_len));
        out.push('\n');

        for row in &self.rules {
            let mode = if row.dry_run { "dry-run" } else { "live" };
            let freed = format_bytes_human(row.bytes_freed);
            out.push_str(&format!(
                "{:<rule_w$}  {:<outcome_w$}  {:>9}  {:>6}  {:>5}  {:>8}  {:>5}  {:>10}  {}\n",
                row.rule,
                row.outcome,
                row.evaluated,
                row.acted,
                row.skipped_in_use,
                row.skipped_hook,
                row.kept_floor,
                freed,
                mode
            ));
        }

        out.push_str(&"-".repeat(sep_len));
        out.push('\n');
        out.push_str(&format!(
            "Totals: {} evaluated, {} {}, {} freed\n",
            self.total_evaluated,
            self.total_acted,
            if self.dry_run { "would-act" } else { "acted" },
            format_bytes_human(self.total_bytes_freed)
        ));
        out
    }
}

/// Formats a byte count using human-readable IEC units (`B`, `KiB`, `MiB`, `GiB`, `TiB`).
pub fn format_bytes_human(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < MIB {
        format!("{:.2} KiB", b / KIB)
    } else if b < GIB {
        format!("{:.2} MiB", b / MIB)
    } else if b < TIB {
        format!("{:.2} GiB", b / GIB)
    } else {
        format!("{:.2} TiB", b / TIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::{self, ErrorKind};
    use tempfile::TempDir;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "simulated broken pipe",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn audit_logger_noop_and_from_config_behavior() {
        let noop = AuditLogger::noop();
        assert!(!noop.is_enabled());
        assert!(noop.path().is_none());
        let entry = AuditEntry::new(AuditEventKind::SweepStart, "run-1", true);
        assert!(noop.log(&entry).is_ok());

        let disabled_cfg = AuditConfig {
            log_path: Some(PathBuf::from("/tmp/ignored.jsonl")),
            jsonl: false,
        };
        let logger = AuditLogger::from_config(&disabled_cfg, None).unwrap();
        assert!(!logger.is_enabled());
        assert!(format!("{logger:?}").contains("AuditLogger"));

        let empty_cfg = AuditConfig::default();
        let logger2 = AuditLogger::from_config(&empty_cfg, None).unwrap();
        assert!(!logger2.is_enabled());

        let tmp_cfg = TempDir::new().unwrap();
        let primary_path = tmp_cfg.path().join("primary.jsonl");
        let primary_cfg = AuditConfig {
            log_path: Some(primary_path.clone()),
            jsonl: true,
        };
        let logger3 = AuditLogger::from_config(&primary_cfg, None).unwrap();
        assert_eq!(logger3.path(), Some(primary_path.as_path()));
    }

    #[test]
    fn audit_logger_writes_valid_utf8_jsonl_and_builder_fields() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("nested/audit.jsonl");
        let cfg = AuditConfig::default();
        let logger = AuditLogger::from_config(&cfg, Some(&log_path)).unwrap();
        assert!(logger.is_enabled());
        assert_eq!(logger.path(), Some(log_path.as_path()));

        let ts = Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap();
        let entry = AuditEntry::new(AuditEventKind::FileDecision, "run-42", false)
            .with_rule("pg-backups")
            .with_candidate("/var/backups/pg_20261120.sql", 2048)
            .with_resolved_ts(ts)
            .with_decision("acted")
            .with_in_use_reasons(vec![])
            .with_actions(vec!["Compress(Gzip)".to_string(), "Move".to_string()])
            .with_dest_path("/mnt/cold/pg_20261120.sql.gz")
            .with_sha256("deadbeef")
            .with_duration_ms(17)
            .with_error("none")
            .with_details(serde_json::json!({"note": "ok"}));

        logger.log(&entry).unwrap();

        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["event"], "file_decision");
        assert_eq!(parsed["run_id"], "run-42");
        assert_eq!(parsed["rule"], "pg-backups");
        assert_eq!(parsed["size_bytes"], 2048);
        assert_eq!(parsed["decision"], "acted");
        assert_eq!(parsed["action"], "Compress(Gzip)->Move");
        assert_eq!(parsed["actions"][0], "Compress(Gzip)");
        assert_eq!(parsed["sha256"], "deadbeef");
        assert_eq!(parsed["duration_ms"], 17);
        assert_eq!(parsed["details"]["note"], "ok");
    }

    #[test]
    fn audit_logger_propagates_write_errors_and_formats_summaries() {
        let failing = AuditLogger::from_writer(FailingWriter);
        let entry = AuditEntry::new(AuditEventKind::Error, "run-err", false);
        assert!(matches!(
            failing.log(&entry),
            Err(FileCleanerError::Io { .. })
        ));

        let rows = vec![
            RuleSummaryRow {
                rule: "pg-backups".to_string(),
                outcome: "ok".to_string(),
                evaluated: 10,
                acted: 4,
                skipped_in_use: 1,
                skipped_hook: 1,
                kept_floor: 3,
                bytes_freed: 15 * 1024 * 1024,
                dry_run: false,
                error: None,
            },
            RuleSummaryRow {
                rule: "logs".to_string(),
                outcome: "constrained".to_string(),
                evaluated: 5,
                acted: 0,
                skipped_in_use: 2,
                skipped_hook: 0,
                kept_floor: 3,
                bytes_freed: 0,
                dry_run: true,
                error: Some("blocked by min_keep_files".to_string()),
            },
        ];

        let summary = SweepSummary::from_rows("run-summary", false, rows);
        assert_eq!(summary.total_evaluated, 15);
        assert_eq!(summary.total_acted, 4);
        assert_eq!(summary.total_bytes_freed, 15 * 1024 * 1024);

        let table = summary.format_table();
        assert!(table.contains("pg-backups"));
        assert!(table.contains("15.00 MiB"));
        assert!(table.contains("LIVE"));

        let pretty = summary.to_json_pretty().unwrap();
        let compact = summary.to_json_compact().unwrap();
        let roundtrip: SweepSummary = serde_json::from_str(&pretty).unwrap();
        assert_eq!(roundtrip, summary);
        assert!(compact.contains("\"run_id\":\"run-summary\""));

        assert_eq!(format_bytes_human(512), "512 B");
        assert_eq!(format_bytes_human(2048), "2.00 KiB");
        assert_eq!(format_bytes_human(2 * 1024 * 1024), "2.00 MiB");
        assert_eq!(format_bytes_human(3 * 1024 * 1024 * 1024), "3.00 GiB");
        assert_eq!(
            format_bytes_human(2 * 1024 * 1024 * 1024 * 1024),
            "2.00 TiB"
        );

        let dry_summary = SweepSummary::from_rows("run-dry", true, vec![]);
        let dry_table = dry_summary.format_table();
        assert!(dry_table.contains("DRY-RUN"));
        assert!(dry_table.contains("would-act"));

        let empty_actions_entry =
            AuditEntry::new(AuditEventKind::SweepEnd, "run-dry", true).with_actions(vec![]);
        assert!(empty_actions_entry.action.is_none());

        // Exercise AuditLogger Debug formatting, noop jsonl=false config, and enabled accessors.
        let tmp = TempDir::new().unwrap();
        let disabled = AuditLogger::from_config(
            &AuditConfig {
                log_path: Some(tmp.path().join("ignored.jsonl")),
                jsonl: false,
            },
            None,
        )
        .unwrap();
        assert!(!disabled.is_enabled());
        assert!(disabled.path().is_none());
        assert!(format!("{disabled:?}").contains("AuditLogger"));

        let summary = SweepSummary::from_rows(
            "run-summary",
            false,
            vec![RuleSummaryRow {
                rule: "logs".to_string(),
                outcome: "ok".to_string(),
                evaluated: 3,
                acted: 1,
                skipped_in_use: 1,
                skipped_hook: 0,
                kept_floor: 1,
                bytes_freed: 512,
                dry_run: false,
                error: None,
            }],
        );
        let pretty = summary.to_json_pretty().unwrap();
        let compact = summary.to_json_compact().unwrap();
        assert!(pretty.contains("\"run-summary\"") && compact.contains("\"total_acted\":1"));

        let rich_entry = AuditEntry::new(AuditEventKind::HookExec, "run-rich", true)
            .with_in_use_reasons(vec!["OsFlock".to_string()])
            .with_actions(vec![])
            .with_details(serde_json::json!({"ok": true}));
        assert!(rich_entry.action.is_none());
        assert_eq!(rich_entry.in_use_reasons.as_ref().unwrap().len(), 1);
        assert!(rich_entry.details.is_some());

        // `format_bytes_human` covers all magnitude tiers and `format_table` formats output cleanly.
        assert_eq!(format_bytes_human(512), "512 B");
        assert_eq!(format_bytes_human(2048), "2.00 KiB");
        assert_eq!(format_bytes_human(2 * 1024 * 1024), "2.00 MiB");
        assert_eq!(format_bytes_human(3 * 1024 * 1024 * 1024), "3.00 GiB");
        assert_eq!(
            format_bytes_human(2 * 1024 * 1024 * 1024 * 1024),
            "2.00 TiB"
        );
        assert!(summary.format_table().contains("Totals:"));

        // Opening an audit log beneath an existing regular file propagates `Io` error cleanly.
        let blocker = tmp.path().join("file_blocker");
        fs::write(&blocker, b"x").unwrap();
        assert!(AuditLogger::open(blocker.join("audit.jsonl")).is_err());
    }
}
