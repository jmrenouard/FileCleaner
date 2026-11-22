//! Strict 6-phase sweep orchestrator (`Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`).

use crate::actions::{ActionExecutor, WorkingTarget};
use crate::audit::{AuditEntry, AuditEventKind, AuditLogger, RuleSummaryRow, SweepSummary};
use crate::config::{ActionStep, Config, RuleConfig};
use crate::error::{FileCleanerError, Result};
use crate::hooks::{
    evaluate_hook, format_action_summary, HookCandidate, HookFailureDisposition, HookRunner,
    HookVerdict, ProcessHookRunner,
};
use crate::in_use::{InUseBatchResult, InUseDetector, LockProbe, SweepLockProbe};
use crate::policy::{DecisionKind, RetentionEngine, RuleOutcome, SweepPlan};
use crate::s3::{HttpS3Uploader, S3Uploader};
use crate::scanner::{
    system_time_to_utc, Clock, CompiledRuleMatchers, ScanReport, Scanner, SystemClock,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Clean completion exit code (`run`, `plan`, `validate`, `init`, `daemon`, or safe `check-in-use`).
pub const EXIT_OK: i32 = 0;

/// Operational, I/O, integrity, timestamp, or non-skipped hook failure exit code.
pub const EXIT_OPERATIONAL_ERROR: i32 = 1;

/// CLI usage error or configuration / `INV-CFG-*` validation failure exit code.
pub const EXIT_CONFIG_OR_USAGE_ERROR: i32 = 2;

/// Strict safety gate trigger exit code (`--fail-on-freeze` or `--strict-constraints`).
pub const EXIT_STRICT_SAFETY_GATE: i32 = 3;

/// Daemon lifecycle error exit code (already running PID, stopped/stale status, stop timeout).
pub const EXIT_DAEMON_ERROR: i32 = 4;

/// Exit code emitted by `check-in-use` when any evaluated path is busy / in-use.
pub const EXIT_IN_USE_BUSY: i32 = 10;

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Execution options controlling a single sweep run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Optional subset of rule names to run (empty runs all rules in configuration order).
    pub rule_filter: Vec<String>,
    /// Global dry-run override (`--dry-run` or `plan` subcommand).
    pub dry_run: bool,
    /// Optional CLI audit log override taking precedence over configuration paths.
    pub audit_log_override: Option<PathBuf>,
    /// Elevate any triggered `BackupGuard` freeze to exit code `3` (`--fail-on-freeze`).
    pub fail_on_freeze: bool,
    /// Elevate any `RuleOutcome::Constrained` rule to exit code `3` (`--strict-constraints`).
    pub strict_constraints: bool,
    /// Optional cooperative shutdown flag checked between rules and individual candidate files.
    pub shutdown: Option<Arc<AtomicBool>>,
}

/// Detailed execution report for a single rule within a sweep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleExecutionReport {
    /// Rule name.
    pub rule_name: String,
    /// Whether the rule was enabled.
    pub enabled: bool,
    /// Whether effective dry-run mode applied to this rule.
    pub dry_run: bool,
    /// Computed deterministic retention and safety plan.
    pub plan: SweepPlan,
    /// Aggregated summary row for CLI tables and `--json` reports.
    pub summary: RuleSummaryRow,
    /// Number of candidate files whose action pipeline succeeded (or simulated in dry-run).
    pub acted_files: usize,
    /// Number of candidate files skipped cleanly by pre-delete hook veto (`skip_exit_code`).
    pub skipped_hook: usize,
    /// Number of candidate files where a hook failed under `WarnAndContinue`.
    pub hook_warnings: usize,
    /// Recorded operational or hook error diagnostics for this rule.
    pub errors: Vec<String>,
    /// Whether a hook `AbortRule` terminated remaining actions for this rule.
    pub aborted_by_hook: bool,
    /// Whether a cooperative shutdown interrupted remaining candidates in this rule.
    pub interrupted: bool,
}

/// Complete outcome of executing [`SweepEngine::run_config`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepReport {
    /// Unique sweep execution identifier (e.g., `"run-42180-1"`).
    pub run_id: String,
    /// Global effective dry-run status (`config.dry_run || opts.dry_run`).
    pub dry_run: bool,
    /// UTC timestamp when the sweep started.
    pub started_at: DateTime<Utc>,
    /// UTC timestamp when the sweep finished.
    pub finished_at: DateTime<Utc>,
    /// Elapsed wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Deterministic CLI exit code (`0`, `1`, or `3`).
    pub exit_code: i32,
    /// Whether any scan, action, or non-tolerated hook error occurred.
    pub has_operational_error: bool,
    /// Rule names that froze due to `BackupGuard` staleness or empty-valid checks.
    pub active_freezes: Vec<String>,
    /// Rule names ending in `RuleOutcome::Constrained`.
    pub constrained_rules: Vec<String>,
    /// Whether cooperative shutdown stopped processing before all rules/files completed.
    pub interrupted: bool,
    /// Aggregated table/JSON summary across all evaluated rules.
    pub summary: SweepSummary,
    /// Per-rule execution details including full `SweepPlan` decisions.
    pub rule_reports: Vec<RuleExecutionReport>,
}

impl SweepReport {
    /// Serializes this sweep report as pretty-printed JSON (`--json`).
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|err| FileCleanerError::Serialization(err.to_string()))
    }

    /// Serializes this sweep report as compact single-line JSON.
    pub fn to_json_compact(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|err| FileCleanerError::Serialization(err.to_string()))
    }

    /// Formats a human-readable CLI summary table with freeze, constraint, and error notes.
    pub fn format_table(&self) -> String {
        let mut out = self.summary.format_table();
        if !self.active_freezes.is_empty() {
            out.push_str(&format!(
                "Active freezes: {}\n",
                self.active_freezes.join(", ")
            ));
        }
        if !self.constrained_rules.is_empty() {
            out.push_str(&format!(
                "Constrained rules: {}\n",
                self.constrained_rules.join(", ")
            ));
        }
        for row in &self.summary.rules {
            if let Some(err) = &row.error {
                out.push_str(&format!("  [{}] {}: {}\n", row.outcome, row.rule, err));
            }
        }
        if self.interrupted {
            out.push_str("Status: interrupted gracefully by shutdown signal\n");
        }
        out
    }
}

/// Deterministic 6-phase sweep orchestrator.
#[derive(Debug, Default, Clone, Copy)]
pub struct SweepEngine;

impl SweepEngine {
    /// Runs [`SweepEngine::run_config`] using production OS clock, single-pass lock probes, and process hooks.
    pub fn run_live(config: &Config, opts: &RunOptions) -> Result<SweepReport> {
        let lock_probe = SweepLockProbe::default();
        Self::run_config(config, opts, &SystemClock, &lock_probe, &ProcessHookRunner)
    }

    /// Executes the strict 6-phase pipeline (`Scan -> InUseProbe -> BackupGuard ->
    /// RetentionEval -> PreHook -> Execute`) across all selected rules in `config`.
    pub fn run_config(
        config: &Config,
        opts: &RunOptions,
        clock: &dyn Clock,
        lock_probe: &dyn LockProbe,
        hook_runner: &dyn HookRunner,
    ) -> Result<SweepReport> {
        config.validate()?;
        let compiled = Scanner::compile_config(config)?;
        Self::run_config_precompiled_with_s3(
            config,
            &compiled,
            opts,
            clock,
            lock_probe,
            hook_runner,
            None,
        )
    }

    /// Executes the strict 6-phase pipeline with an injectable [`S3Uploader`] for deterministic testing.
    pub fn run_config_with_s3(
        config: &Config,
        opts: &RunOptions,
        clock: &dyn Clock,
        lock_probe: &dyn LockProbe,
        hook_runner: &dyn HookRunner,
        s3_uploader: &dyn S3Uploader,
    ) -> Result<SweepReport> {
        config.validate()?;
        let compiled = Scanner::compile_config(config)?;
        Self::run_config_precompiled_with_s3(
            config,
            &compiled,
            opts,
            clock,
            lock_probe,
            hook_runner,
            Some(s3_uploader),
        )
    }

    /// Executes the strict 6-phase pipeline using precompiled rule globs and regex matchers.
    pub(crate) fn run_config_precompiled(
        config: &Config,
        compiled: &HashMap<String, CompiledRuleMatchers>,
        opts: &RunOptions,
        clock: &dyn Clock,
        lock_probe: &dyn LockProbe,
        hook_runner: &dyn HookRunner,
    ) -> Result<SweepReport> {
        Self::run_config_precompiled_with_s3(
            config,
            compiled,
            opts,
            clock,
            lock_probe,
            hook_runner,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_config_precompiled_with_s3(
        config: &Config,
        compiled: &HashMap<String, CompiledRuleMatchers>,
        opts: &RunOptions,
        clock: &dyn Clock,
        lock_probe: &dyn LockProbe,
        hook_runner: &dyn HookRunner,
        s3_uploader: Option<&dyn S3Uploader>,
    ) -> Result<SweepReport> {
        let selected_rules = select_rules(&config.rules, &opts.rule_filter)?;
        let global_dry_run = config.dry_run || opts.dry_run;
        let run_id = next_run_id();
        let started_at = clock.now();
        let sweep_timer = Instant::now();

        let needs_live_s3 = s3_uploader.is_none()
            && !global_dry_run
            && selected_rules
                .iter()
                .any(|r| r.enabled && !r.dry_run && r.actions.contains(&ActionStep::UploadS3));
        let owned_s3 = needs_live_s3.then(HttpS3Uploader::new);
        let effective_s3_uploader: Option<&dyn S3Uploader> =
            s3_uploader.or_else(|| owned_s3.as_ref().map(|u| u as &dyn S3Uploader));

        let effective_audit_path = opts
            .audit_log_override
            .as_deref()
            .or_else(|| config.effective_audit_log());
        let logger = open_audit_logger(config, opts)?;

        logger.log(
            &AuditEntry::new(AuditEventKind::SweepStart, &run_id, global_dry_run).with_details(
                serde_json::json!({
                    "rules_selected": selected_rules.len(),
                    "fail_on_freeze": opts.fail_on_freeze,
                    "strict_constraints": opts.strict_constraints,
                }),
            ),
        )?;

        let mut has_operational_error = false;
        let mut active_freezes = Vec::new();
        let mut constrained_rules = Vec::new();
        let mut interrupted = false;
        let mut summary_rows = Vec::with_capacity(selected_rules.len());
        let mut rule_reports = Vec::with_capacity(selected_rules.len());

        for rule in selected_rules {
            if shutdown_requested(&opts.shutdown) {
                interrupted = true;
                break;
            }

            let rule_dry_run = global_dry_run || rule.dry_run;
            if !rule.enabled {
                let empty_plan = empty_sweep_plan(rule, RuleOutcome::Ok, None);
                let row = RuleSummaryRow {
                    rule: rule.name.clone(),
                    outcome: "skipped".to_string(),
                    evaluated: 0,
                    acted: 0,
                    skipped_in_use: 0,
                    skipped_hook: 0,
                    kept_floor: 0,
                    bytes_freed: 0,
                    dry_run: rule_dry_run,
                    error: None,
                };
                summary_rows.push(row.clone());
                rule_reports.push(RuleExecutionReport {
                    rule_name: rule.name.clone(),
                    enabled: false,
                    dry_run: rule_dry_run,
                    plan: empty_plan,
                    summary: row,
                    acted_files: 0,
                    skipped_hook: 0,
                    hook_warnings: 0,
                    errors: Vec::new(),
                    aborted_by_hook: false,
                    interrupted: false,
                });
                continue;
            }

            let rule_report = execute_single_rule(
                config,
                rule,
                compiled.get(&rule.name),
                rule_dry_run,
                effective_audit_path,
                &run_id,
                &logger,
                clock,
                lock_probe,
                hook_runner,
                effective_s3_uploader,
                &opts.shutdown,
            )?;

            if !rule_report.errors.is_empty() || rule_report.aborted_by_hook {
                has_operational_error = true;
            }
            if rule_report.plan.outcome == RuleOutcome::Frozen {
                active_freezes.push(rule.name.clone());
            } else if rule_report.plan.outcome == RuleOutcome::Constrained {
                constrained_rules.push(rule.name.clone());
            }
            if rule_report.interrupted {
                interrupted = true;
            }

            summary_rows.push(rule_report.summary.clone());
            rule_reports.push(rule_report);

            if interrupted {
                break;
            }
        }

        let summary = SweepSummary::from_rows(&run_id, global_dry_run, summary_rows);
        let duration_ms = elapsed_millis(sweep_timer);
        let finished_at = clock.now();
        let exit_code = compute_exit_code(
            has_operational_error,
            !active_freezes.is_empty(),
            !constrained_rules.is_empty(),
            opts.fail_on_freeze,
            opts.strict_constraints,
        );

        logger.log(
            &AuditEntry::new(AuditEventKind::SweepEnd, &run_id, global_dry_run)
                .with_duration_ms(duration_ms)
                .with_details(serde_json::json!({
                    "exit_code": exit_code,
                    "total_evaluated": summary.total_evaluated,
                    "total_acted": summary.total_acted,
                    "total_bytes_freed": summary.total_bytes_freed,
                    "active_freezes": active_freezes,
                    "constrained_rules": constrained_rules,
                    "interrupted": interrupted,
                })),
        )?;

        Ok(SweepReport {
            run_id,
            dry_run: global_dry_run,
            started_at,
            finished_at,
            duration_ms,
            exit_code,
            has_operational_error,
            active_freezes,
            constrained_rules,
            interrupted,
            summary,
            rule_reports,
        })
    }
}

/// Computes deterministic CLI exit code (`0`, `1`, or `3`) from sweep outcome flags.
pub fn compute_exit_code(
    has_operational_error: bool,
    has_freeze: bool,
    has_constrained: bool,
    fail_on_freeze: bool,
    strict_constraints: bool,
) -> i32 {
    if has_operational_error {
        return EXIT_OPERATIONAL_ERROR;
    }
    if (fail_on_freeze && has_freeze) || (strict_constraints && has_constrained) {
        return EXIT_STRICT_SAFETY_GATE;
    }
    EXIT_OK
}

#[allow(clippy::too_many_arguments)]
fn execute_single_rule(
    config: &Config,
    rule: &RuleConfig,
    compiled: Option<&CompiledRuleMatchers>,
    rule_dry_run: bool,
    effective_audit_path: Option<&std::path::Path>,
    run_id: &str,
    logger: &AuditLogger,
    clock: &dyn Clock,
    lock_probe: &dyn LockProbe,
    hook_runner: &dyn HookRunner,
    s3_uploader: Option<&dyn S3Uploader>,
    shutdown: &Option<Arc<AtomicBool>>,
) -> Result<RuleExecutionReport> {
    let rule_timer = Instant::now();

    // Phase 1: Scan directory, apply filter algebra, and resolve timestamps fail-closed.
    let scan =
        match Scanner::scan_rule_compiled(rule, compiled, effective_audit_path, &config.daemon) {
            Ok(report) => report,
            Err(err) => {
                let err_msg = err.to_string();
                logger.log(
                    &AuditEntry::new(AuditEventKind::Error, run_id, rule_dry_run)
                        .with_rule(&rule.name)
                        .with_decision("error")
                        .with_duration_ms(elapsed_millis(rule_timer))
                        .with_error(&err_msg),
                )?;
                let row = RuleSummaryRow {
                    rule: rule.name.clone(),
                    outcome: "aborted".to_string(),
                    evaluated: 0,
                    acted: 0,
                    skipped_in_use: 0,
                    skipped_hook: 0,
                    kept_floor: 0,
                    bytes_freed: 0,
                    dry_run: rule_dry_run,
                    error: Some(err_msg.clone()),
                };
                return Ok(RuleExecutionReport {
                    rule_name: rule.name.clone(),
                    enabled: true,
                    dry_run: rule_dry_run,
                    plan: empty_sweep_plan(rule, RuleOutcome::Ok, None),
                    summary: row,
                    acted_files: 0,
                    skipped_hook: 0,
                    hook_warnings: 0,
                    errors: vec![err_msg],
                    aborted_by_hook: false,
                    interrupted: false,
                });
            }
        };

    for skipped_path in &scan.skipped_timestamp_parse {
        logger.log(
            &AuditEntry::new(AuditEventKind::FileDecision, run_id, rule_dry_run)
                .with_rule(&rule.name)
                .with_candidate(skipped_path, 0)
                .with_decision("skip_ts_parse")
                .with_error("filename_date failed to match or parse"),
        )?;
    }

    // Phase 2: 6-layer multi-probe active-writer detection (skip OS locks on P_floor, read-only in dry-run).
    let floor_paths = crate::policy::compute_protected_floor_paths(rule, &scan.files);
    let in_use_batch = InUseDetector::check_batch_scoped(
        &scan.files,
        &rule.in_use,
        clock,
        lock_probe,
        &floor_paths,
        rule_dry_run,
    );

    // Phases 3 & 4: Backup safety guards and deterministic retention plan.
    let plan = RetentionEngine::plan(rule, &scan, &in_use_batch, clock);

    emit_non_evict_decision_audits(&plan, &in_use_batch, run_id, rule_dry_run, logger)?;

    if plan.outcome == RuleOutcome::Frozen {
        let freeze_msg = plan
            .freeze_reason
            .clone()
            .unwrap_or_else(|| "backup_guard freeze triggered".to_string());
        logger.log(
            &AuditEntry::new(AuditEventKind::BackupFreeze, run_id, rule_dry_run)
                .with_rule(&rule.name)
                .with_decision("freeze")
                .with_duration_ms(elapsed_millis(rule_timer))
                .with_error(&freeze_msg)
                .with_details(serde_json::json!({
                    "total_candidates": plan.total_candidates,
                    "valid_backup_count": plan.valid_backup_count,
                    "protected_floor_count": plan.protected_floor_count,
                })),
        )?;
        let row = RuleSummaryRow {
            rule: rule.name.clone(),
            outcome: "frozen".to_string(),
            evaluated: plan.total_candidates,
            acted: 0,
            skipped_in_use: plan.busy_skipped_count,
            skipped_hook: 0,
            kept_floor: plan.protected_floor_count,
            bytes_freed: 0,
            dry_run: rule_dry_run,
            error: Some(freeze_msg),
        };
        return Ok(RuleExecutionReport {
            rule_name: rule.name.clone(),
            enabled: true,
            dry_run: rule_dry_run,
            plan,
            summary: row,
            acted_files: 0,
            skipped_hook: 0,
            hook_warnings: 0,
            errors: Vec::new(),
            aborted_by_hook: false,
            interrupted: false,
        });
    }

    // Phases 5 & 6: Synchronous pre-delete hooks and ordered lifecycle actions.
    let mut acted_files = 0usize;
    let mut skipped_hook = 0usize;
    let mut hook_warnings = 0usize;
    let mut bytes_freed = 0u64;
    let mut errors = Vec::new();
    let mut aborted_by_hook = false;
    let mut rule_interrupted = false;

    let action_summary = format_action_summary(&rule.actions, rule.compression);

    for candidate in &plan.evict_files {
        if shutdown_requested(shutdown) {
            rule_interrupted = true;
            break;
        }

        let file_timer = Instant::now();

        // Phase 5: PreHook gate.
        if let Some(hook) = &rule.pre_delete_hook {
            let hc = HookCandidate {
                path: candidate.path.clone(),
                name: candidate.name.clone(),
                size_bytes: candidate.size_bytes,
                mtime: candidate.modified_ts,
                rule_name: rule.name.clone(),
                action_summary: action_summary.clone(),
            };

            match evaluate_hook(hook_runner, &hc, hook, rule_dry_run) {
                Ok(HookVerdict::Proceed) => {
                    logger.log(
                        &AuditEntry::new(AuditEventKind::HookExec, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("proceed")
                            .with_duration_ms(elapsed_millis(file_timer)),
                    )?;
                }
                Ok(HookVerdict::SimulatedDryRun) => {
                    logger.log(
                        &AuditEntry::new(AuditEventKind::HookExec, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("simulated_dry_run"),
                    )?;
                }
                Ok(HookVerdict::SkippedByCode(code)) => {
                    skipped_hook += 1;
                    logger.log(
                        &AuditEntry::new(AuditEventKind::HookExec, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("skip_hook")
                            .with_duration_ms(elapsed_millis(file_timer))
                            .with_details(serde_json::json!({ "exit_code": code })),
                    )?;
                    logger.log(
                        &AuditEntry::new(AuditEventKind::FileDecision, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("skip_hook")
                            .with_duration_ms(elapsed_millis(file_timer)),
                    )?;
                    continue;
                }
                Ok(HookVerdict::WarnAndContinue { reason }) => {
                    hook_warnings += 1;
                    logger.log(
                        &AuditEntry::new(AuditEventKind::HookExec, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("warn_and_continue")
                            .with_duration_ms(elapsed_millis(file_timer))
                            .with_error(&reason),
                    )?;
                }
                Err((hook_err, disposition)) => {
                    let err_str = hook_err.to_string();
                    errors.push(err_str.clone());
                    logger.log(
                        &AuditEntry::new(AuditEventKind::HookExec, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("aborted")
                            .with_duration_ms(elapsed_millis(file_timer))
                            .with_error(&err_str),
                    )?;
                    logger.log(
                        &AuditEntry::new(AuditEventKind::Error, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("aborted")
                            .with_error(&err_str),
                    )?;
                    match disposition {
                        HookFailureDisposition::AbortFile => continue,
                        HookFailureDisposition::AbortRule => {
                            aborted_by_hook = true;
                            break;
                        }
                    }
                }
            }
        }

        // Phase 6: Execute ordered lifecycle action pipeline (or dry-run simulation).
        // Re-verify candidate is still non-symlink immediately prior to mutation when follow_symlinks=false.
        if !rule_dry_run && !rule.follow_symlinks {
            if let Ok(sym) = fs::symlink_metadata(&candidate.path) {
                if sym.file_type().is_symlink() {
                    let err_str = format!(
                        "refusing symlink target '{}' when follow_symlinks=false",
                        candidate.path.display()
                    );
                    errors.push(err_str.clone());
                    logger.log(
                        &AuditEntry::new(AuditEventKind::Error, run_id, rule_dry_run)
                            .with_rule(&rule.name)
                            .with_candidate(&candidate.path, candidate.size_bytes)
                            .with_resolved_ts(candidate.resolved_ts)
                            .with_decision("error")
                            .with_duration_ms(elapsed_millis(file_timer))
                            .with_error(&err_str),
                    )?;
                    continue;
                }
            }
        }

        let atime = fs::metadata(&candidate.path)
            .ok()
            .and_then(|meta| meta.accessed().ok())
            .and_then(|st| system_time_to_utc(st, &candidate.path).ok());
        let target = WorkingTarget {
            path: candidate.path.clone(),
            size_bytes: candidate.size_bytes,
            mtime: candidate.modified_ts,
            atime,
        };

        match ActionExecutor::execute_pipeline_with_s3(
            target,
            rule,
            config.global_trash_dir.as_deref(),
            config.s3.as_ref(),
            s3_uploader,
            rule_dry_run,
        ) {
            Ok(action_report) => {
                acted_files += 1;
                bytes_freed = bytes_freed.saturating_add(action_report.bytes_freed);
                let elapsed_ms = elapsed_millis(file_timer);
                let decision_label = if action_report.dry_run {
                    "would_act"
                } else {
                    "acted"
                };
                let s3_details = if let Some(rcpt) = &action_report.s3_receipt {
                    Some(serde_json::json!({
                        "s3_uri": rcpt.uri,
                        "etag": rcpt.etag,
                        "verify_mode": rcpt.verify_mode,
                        "parts_uploaded": rcpt.parts_uploaded,
                        "verified_size": rcpt.verified_size,
                    }))
                } else {
                    action_report
                        .remote_uri
                        .as_ref()
                        .map(|uri| serde_json::json!({ "s3_uri": uri, "dry_run": true }))
                };

                let mut exec_entry =
                    AuditEntry::new(AuditEventKind::ActionExec, run_id, action_report.dry_run)
                        .with_rule(&rule.name)
                        .with_candidate(&candidate.path, candidate.size_bytes)
                        .with_resolved_ts(candidate.resolved_ts)
                        .with_decision(decision_label)
                        .with_actions(action_report.steps_executed.clone())
                        .with_duration_ms(elapsed_ms);
                if let Some(dest) = &action_report.final_path {
                    exec_entry = exec_entry.with_dest_path(dest);
                }
                if let Some(sha) = &action_report.sha256 {
                    exec_entry = exec_entry.with_sha256(sha);
                }
                if let Some(details) = s3_details.clone() {
                    exec_entry = exec_entry.with_details(details);
                }
                logger.log(&exec_entry)?;

                let mut decision_entry =
                    AuditEntry::new(AuditEventKind::FileDecision, run_id, action_report.dry_run)
                        .with_rule(&rule.name)
                        .with_candidate(&candidate.path, candidate.size_bytes)
                        .with_resolved_ts(candidate.resolved_ts)
                        .with_decision(decision_label)
                        .with_actions(action_report.steps_executed)
                        .with_duration_ms(elapsed_ms);
                if let Some(dest) = &action_report.final_path {
                    decision_entry = decision_entry.with_dest_path(dest);
                }
                if let Some(sha) = &action_report.sha256 {
                    decision_entry = decision_entry.with_sha256(sha);
                }
                if let Some(details) = s3_details {
                    decision_entry = decision_entry.with_details(details);
                }
                logger.log(&decision_entry)?;
            }
            Err(act_err) => {
                let err_str = act_err.to_string();
                errors.push(err_str.clone());
                let mut err_entry = AuditEntry::new(AuditEventKind::Error, run_id, rule_dry_run)
                    .with_rule(&rule.name)
                    .with_candidate(&candidate.path, candidate.size_bytes)
                    .with_resolved_ts(candidate.resolved_ts)
                    .with_decision("error")
                    .with_duration_ms(elapsed_millis(file_timer))
                    .with_error(&err_str);
                if let FileCleanerError::S3Upload { uri, .. } = &act_err {
                    err_entry = err_entry.with_details(serde_json::json!({ "s3_uri": uri }));
                }
                logger.log(&err_entry)?;
            }
        }
    }

    let mut diagnostic_note: Option<String> = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };

    if plan.outcome == RuleOutcome::Constrained {
        let constraint_msg = format_constrained_message(&plan);
        logger.log(
            &AuditEntry::new(AuditEventKind::RuleConstrained, run_id, rule_dry_run)
                .with_rule(&rule.name)
                .with_decision("keep_floor")
                .with_duration_ms(elapsed_millis(rule_timer))
                .with_error(&constraint_msg)
                .with_details(serde_json::json!({
                    "remaining_files": plan.remaining_files,
                    "target_max_files": plan.target_max_files,
                    "remaining_bytes": plan.remaining_bytes,
                    "target_max_total_size": plan.target_max_total_size,
                    "protected_floor_count": plan.protected_floor_count,
                    "busy_skipped_count": plan.busy_skipped_count,
                })),
        )?;
        if diagnostic_note.is_none() {
            diagnostic_note = Some(constraint_msg);
        }
    }

    let outcome_label = if aborted_by_hook {
        "aborted"
    } else if !errors.is_empty() {
        "error"
    } else if plan.outcome == RuleOutcome::Constrained {
        "constrained"
    } else {
        "ok"
    };

    let row = RuleSummaryRow {
        rule: rule.name.clone(),
        outcome: outcome_label.to_string(),
        evaluated: plan.total_candidates,
        acted: acted_files,
        skipped_in_use: plan.busy_skipped_count,
        skipped_hook,
        kept_floor: plan.protected_floor_count,
        bytes_freed,
        dry_run: rule_dry_run,
        error: diagnostic_note,
    };

    Ok(RuleExecutionReport {
        rule_name: rule.name.clone(),
        enabled: true,
        dry_run: rule_dry_run,
        plan,
        summary: row,
        acted_files,
        skipped_hook,
        hook_warnings,
        errors,
        aborted_by_hook,
        interrupted: rule_interrupted,
    })
}

fn emit_non_evict_decision_audits(
    plan: &SweepPlan,
    in_use: &InUseBatchResult,
    run_id: &str,
    dry_run: bool,
    logger: &AuditLogger,
) -> Result<()> {
    for decision in &plan.decisions {
        match decision.decision {
            DecisionKind::KeepFloor => {
                logger.log(
                    &AuditEntry::new(AuditEventKind::FileDecision, run_id, dry_run)
                        .with_rule(&plan.rule_name)
                        .with_candidate(&decision.file.path, decision.file.size_bytes)
                        .with_resolved_ts(decision.file.resolved_ts)
                        .with_decision("keep_floor"),
                )?;
            }
            DecisionKind::KeepRetention => {
                logger.log(
                    &AuditEntry::new(AuditEventKind::FileDecision, run_id, dry_run)
                        .with_rule(&plan.rule_name)
                        .with_candidate(&decision.file.path, decision.file.size_bytes)
                        .with_resolved_ts(decision.file.resolved_ts)
                        .with_decision("keep_retention"),
                )?;
            }
            DecisionKind::SkipInUse => {
                let reasons = in_use
                    .reports
                    .get(&decision.file.path)
                    .map(|r| r.reasons.iter().map(ToString::to_string).collect())
                    .unwrap_or_default();
                logger.log(
                    &AuditEntry::new(AuditEventKind::FileDecision, run_id, dry_run)
                        .with_rule(&plan.rule_name)
                        .with_candidate(&decision.file.path, decision.file.size_bytes)
                        .with_resolved_ts(decision.file.resolved_ts)
                        .with_decision("skip_in_use")
                        .with_in_use_reasons(reasons),
                )?;
            }
            DecisionKind::FreezeBackupGuard => {
                logger.log(
                    &AuditEntry::new(AuditEventKind::FileDecision, run_id, dry_run)
                        .with_rule(&plan.rule_name)
                        .with_candidate(&decision.file.path, decision.file.size_bytes)
                        .with_resolved_ts(decision.file.resolved_ts)
                        .with_decision("freeze")
                        .with_error(&decision.reason),
                )?;
            }
            DecisionKind::WouldEvict => {}
        }
    }
    Ok(())
}

fn select_rules<'a>(rules: &'a [RuleConfig], filter: &[String]) -> Result<Vec<&'a RuleConfig>> {
    if filter.is_empty() {
        return Ok(rules.iter().collect());
    }

    let known: HashSet<&str> = rules.iter().map(|r| r.name.as_str()).collect();
    for requested in filter {
        if !known.contains(requested.as_str()) {
            return Err(FileCleanerError::ConfigValidation(format!(
                "unknown rule '{requested}' in --rule filter"
            )));
        }
    }

    let allow: HashSet<&str> = filter.iter().map(String::as_str).collect();
    Ok(rules
        .iter()
        .filter(|r| allow.contains(r.name.as_str()))
        .collect())
}

fn open_audit_logger(config: &Config, opts: &RunOptions) -> Result<AuditLogger> {
    if let Some(override_path) = &opts.audit_log_override {
        return AuditLogger::open(override_path);
    }
    AuditLogger::from_config(&config.audit, config.audit_log.as_deref())
}

fn empty_sweep_plan(
    rule: &RuleConfig,
    outcome: RuleOutcome,
    freeze_reason: Option<String>,
) -> SweepPlan {
    SweepPlan {
        rule_name: rule.name.clone(),
        outcome,
        freeze_reason,
        total_candidates: 0,
        valid_backup_count: 0,
        protected_floor_count: 0,
        busy_skipped_count: 0,
        evict_files: Vec::new(),
        decisions: Vec::new(),
        remaining_files: 0,
        remaining_bytes: 0,
        target_max_files: rule.max_files,
        target_max_total_size: rule.max_total_size,
    }
}

fn format_constrained_message(plan: &SweepPlan) -> String {
    let mut parts = Vec::new();
    if let Some(max_f) = plan.target_max_files {
        if plan.remaining_files > max_f {
            parts.push(format!(
                "max_files exceeded ({} > {})",
                plan.remaining_files, max_f
            ));
        }
    }
    if let Some(max_b) = plan.target_max_total_size {
        if plan.remaining_bytes > max_b {
            parts.push(format!(
                "max_total_size exceeded ({} > {})",
                plan.remaining_bytes, max_b
            ));
        }
    }
    format!(
        "{}: protected by {} floor backup(s) and {} in-use skip(s)",
        parts.join(", "),
        plan.protected_floor_count,
        plan.busy_skipped_count
    )
}

fn next_run_id() -> String {
    let seq = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("run-{}-{seq}", std::process::id())
}

fn elapsed_millis(timer: Instant) -> u64 {
    u64::try_from(timer.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn shutdown_requested(flag: &Option<Arc<AtomicBool>>) -> bool {
    flag.as_ref().is_some_and(|f| f.load(Ordering::Acquire))
}

// Re-export ScanReport/RetentionEngine usage guarantees so unused warnings never occur.
#[allow(dead_code)]
fn _type_anchor(_: &ScanReport, _: &RetentionEngine) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ActionStep, AuditConfig, BackupGuardConfig, CombineMode, CompressionFormat, DaemonConfig,
        EvictionOrder, HookConfig, HookFailurePolicy, InUseConfig, TimeBasis,
    };
    use crate::in_use::{InUseReason, OsLockProbe, ProcFdProbeOutcome};
    use crate::scanner::FixedClock;
    use chrono::TimeZone;
    use std::collections::{BTreeMap, HashMap};
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    fn set_mtime_unix(path: &std::path::Path, secs: i64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(secs as u64))
            .unwrap();
    }

    fn base_rule(name: &str, path: PathBuf) -> RuleConfig {
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
            max_files: None,
            max_age: None,
            max_total_size: None,
            combine_mode: CombineMode::Any,
            eviction_order: EvictionOrder::OldestFirst,
            dry_run: false,
            backup_guard: BackupGuardConfig::default(),
            in_use: InUseConfig {
                check_os_locks: false,
                check_proc_fd: false,
                ..InUseConfig::default()
            },
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

    struct ScriptedHookRunner {
        verdicts: HashMap<String, std::result::Result<HookVerdict, FileCleanerError>>,
    }

    impl HookRunner for ScriptedHookRunner {
        fn run(
            &self,
            candidate: &HookCandidate,
            _hook: &HookConfig,
            dry_run: bool,
        ) -> Result<HookVerdict> {
            if dry_run {
                return Ok(HookVerdict::SimulatedDryRun);
            }
            match self.verdicts.get(&candidate.name) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(err)) => Err(FileCleanerError::HookFailed {
                    rule: candidate.rule_name.clone(),
                    command: "mock-hook".to_string(),
                    exit_code: Some(9),
                    stderr: err.to_string(),
                }),
                None => Ok(HookVerdict::Proceed),
            }
        }
    }

    struct MapProbe {
        locked: HashSet<PathBuf>,
    }

    impl LockProbe for MapProbe {
        fn check_os_lock(&self, path: &Path) -> std::result::Result<Option<InUseReason>, String> {
            if self.locked.contains(path) {
                Ok(Some(InUseReason::OsFlock))
            } else {
                Ok(None)
            }
        }

        fn check_proc_fd(&self, _path: &Path) -> ProcFdProbeOutcome {
            ProcFdProbeOutcome::default()
        }
    }

    #[test]
    fn engine_runs_all_6_phases_with_dry_run_parity_and_audit_events() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("backups");
        let dest_dir = tmp.path().join("archive");
        let audit_path = tmp.path().join("audit.jsonl");
        fs::create_dir_all(&watch_dir).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        let f_new = watch_dir.join("pg_1_new.sql");
        let f_busy = watch_dir.join("pg_2_busy.sql");
        let f_vetoed = watch_dir.join("pg_3_vetoed.sql");
        let f_evict = watch_dir.join("pg_4_evict.sql");

        for (p, age) in [
            (&f_new, 100),
            (&f_busy, 200),
            (&f_vetoed, 300),
            (&f_evict, 400),
        ] {
            fs::write(p, b"valid-backup-payload").unwrap();
            set_mtime_unix(p, now.timestamp() - age);
        }

        let mut locked = HashSet::new();
        locked.insert(f_busy.clone());
        let probe = MapProbe { locked };

        let mut rule = base_rule("pg-backups", watch_dir.clone());
        rule.max_files = Some(1);
        rule.in_use.check_os_locks = true;
        rule.backup_guard.enabled = true;
        rule.backup_guard.min_keep_files = 1;
        rule.backup_guard.min_valid_backup_bytes = 5;
        rule.actions = vec![ActionStep::Compress, ActionStep::Move];
        rule.move_to = Some(dest_dir.clone());
        rule.pre_delete_hook = Some(HookConfig {
            command: "/bin/true".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });

        let mut disabled_rule = base_rule("disabled", tmp.path().join("nonexistent_dir"));
        disabled_rule.enabled = false;

        let cfg = Config {
            dry_run: false,
            audit_log: None,
            daemon: DaemonConfig::default(),
            audit: AuditConfig {
                log_path: Some(audit_path.clone()),
                jsonl: true,
            },
            global_trash_dir: None,
            s3: None,
            rules: vec![rule, disabled_rule],
        };

        let mut verdicts = HashMap::new();
        verdicts.insert(
            "pg_3_vetoed.sql".to_string(),
            Ok(HookVerdict::SkippedByCode(42)),
        );
        let runner = ScriptedHookRunner { verdicts };

        // 1. Dry-run sweep produces exact plan with zero mutations.
        let dry_opts = RunOptions {
            dry_run: true,
            strict_constraints: true,
            ..RunOptions::default()
        };
        let dry_report =
            SweepEngine::run_config(&cfg, &dry_opts, &FixedClock(now), &probe, &runner).unwrap();
        assert!(dry_report.dry_run);
        assert_eq!(dry_report.exit_code, EXIT_STRICT_SAFETY_GATE);
        assert_eq!(dry_report.constrained_rules, vec!["pg-backups"]);
        assert!(f_evict.exists());
        assert!(!dest_dir.exists());

        // 2. Live sweep executes identical plan and writes full JSONL audit trail.
        let live_opts = RunOptions {
            dry_run: false,
            strict_constraints: false,
            ..RunOptions::default()
        };
        let live_report =
            SweepEngine::run_config(&cfg, &live_opts, &FixedClock(now), &probe, &runner).unwrap();
        assert_eq!(live_report.exit_code, EXIT_OK);
        assert_eq!(
            live_report.rule_reports[0].plan.evict_files,
            dry_report.rule_reports[0].plan.evict_files
        );
        assert_eq!(live_report.rule_reports[0].acted_files, 1);
        assert_eq!(live_report.rule_reports[0].skipped_hook, 1);
        assert!(!f_evict.exists());
        assert!(f_vetoed.exists());
        assert!(f_busy.exists());
        assert!(f_new.exists());
        assert!(dest_dir.join("pg_4_evict.sql.gz").exists());
        assert_eq!(live_report.rule_reports[1].summary.outcome, "skipped");

        let table = live_report.format_table();
        assert!(table.contains("pg-backups"));
        assert!(table.contains("Constrained rules: pg-backups"));
        let json = live_report.to_json_pretty().unwrap();
        assert!(json.contains("\"run_id\""));
        assert!(live_report
            .to_json_compact()
            .unwrap()
            .contains("pg-backups"));

        let audit_text = fs::read_to_string(&audit_path).unwrap();
        assert!(audit_text.contains("\"sweep_start\""));
        assert!(audit_text.contains("\"sweep_end\""));
        assert!(audit_text.contains("\"hook_exec\""));
        assert!(audit_text.contains("\"action_exec\""));
        assert!(audit_text.contains("\"rule_constrained\""));
    }

    #[test]
    fn engine_handles_freeze_hook_policies_scan_errors_and_cooperative_shutdown() {
        let tmp = TempDir::new().unwrap();
        let stale_dir = tmp.path().join("stale");
        let abort_dir = tmp.path().join("abort");
        fs::create_dir_all(&stale_dir).unwrap();
        fs::create_dir_all(&abort_dir).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

        // Stale rule triggers freeze.
        let mut frozen_rule = base_rule("frozen-rule", stale_dir.clone());
        frozen_rule.backup_guard.enabled = true;
        frozen_rule.backup_guard.min_valid_backup_bytes = 100;
        let stub = stale_dir.join("stub.sql");
        fs::write(&stub, b"tiny").unwrap();

        // Hook abort rule + warn rule + scan error rule.
        let f1 = abort_dir.join("a.log");
        let f2 = abort_dir.join("b.log");
        fs::write(&f1, b"1").unwrap();
        fs::write(&f2, b"2").unwrap();
        set_mtime_unix(&f1, now.timestamp() - 200);
        set_mtime_unix(&f2, now.timestamp() - 100);

        let mut hook_abort_rule = base_rule("hook-abort", abort_dir.clone());
        hook_abort_rule.pre_delete_hook = Some(HookConfig {
            command: "/bin/false".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::AbortRule,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });

        let missing_dir_rule = base_rule("missing-dir", tmp.path().join("does_not_exist"));

        let cfg = Config {
            rules: vec![frozen_rule.clone(), hook_abort_rule, missing_dir_rule],
            ..Config::default()
        };

        let mut verdicts = HashMap::new();
        verdicts.insert(
            "a.log".to_string(),
            Err(FileCleanerError::Daemon("boom".to_string())),
        );
        let runner = ScriptedHookRunner { verdicts };

        let report = SweepEngine::run_config(
            &cfg,
            &RunOptions::default(),
            &FixedClock(now),
            &OsLockProbe,
            &runner,
        )
        .unwrap();

        assert_eq!(report.exit_code, EXIT_OPERATIONAL_ERROR);
        assert_eq!(report.active_freezes, vec!["frozen-rule"]);
        assert!(report.rule_reports[1].aborted_by_hook);
        assert_eq!(report.rule_reports[1].summary.outcome, "aborted");
        assert_eq!(report.rule_reports[2].summary.outcome, "aborted");
        assert!(f1.exists());
        assert!(f2.exists());

        // Strict freeze without operational errors exits 3.
        let freeze_only_cfg = Config {
            rules: vec![frozen_rule],
            ..Config::default()
        };
        let freeze_report = SweepEngine::run_config(
            &freeze_only_cfg,
            &RunOptions {
                fail_on_freeze: true,
                ..RunOptions::default()
            },
            &FixedClock(now),
            &OsLockProbe,
            &runner,
        )
        .unwrap();
        assert_eq!(freeze_report.exit_code, EXIT_STRICT_SAFETY_GATE);

        // Unknown --rule filter rejected with ConfigValidation.
        let bad_filter = SweepEngine::run_config(
            &freeze_only_cfg,
            &RunOptions {
                rule_filter: vec!["no-such-rule".to_string()],
                ..RunOptions::default()
            },
            &FixedClock(now),
            &OsLockProbe,
            &runner,
        );
        assert!(matches!(
            bad_filter,
            Err(FileCleanerError::ConfigValidation(_))
        ));

        // Pre-set shutdown stops before running rules cleanly.
        let shutdown = Arc::new(AtomicBool::new(true));
        let stopped_report = SweepEngine::run_config(
            &freeze_only_cfg,
            &RunOptions {
                shutdown: Some(shutdown),
                ..RunOptions::default()
            },
            &FixedClock(now),
            &OsLockProbe,
            &runner,
        )
        .unwrap();
        assert!(stopped_report.interrupted);
        assert!(stopped_report.rule_reports.is_empty());
        assert!(stopped_report
            .format_table()
            .contains("interrupted gracefully"));
    }

    #[test]
    fn engine_covers_filename_date_skip_warn_abort_file_action_err_and_max_files_constraint() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("fn_date_dir");
        let dest_dir = tmp.path().join("dest_conflict");
        fs::create_dir_all(&watch_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

        // 1. FilenameDate rule with unparseable file + warn-and-continue + AbortFile + Move conflict error.
        let unparseable = watch_dir.join("notes.sql");
        let f_keep = watch_dir.join("pg_20261120_020000.sql");
        let f_abort = watch_dir.join("pg_20261118_020000.sql");
        let f_conflict = watch_dir.join("pg_20261117_020000.sql");
        let f_warn_ok = watch_dir.join("pg_20261116_020000.sql");

        for p in [&unparseable, &f_keep, &f_abort, &f_conflict, &f_warn_ok] {
            fs::write(p, b"backup-payload-bytes").unwrap();
        }
        // Pre-create destination collision for f_conflict so ActionExecutor returns AlreadyExists.
        fs::write(dest_dir.join("pg_20261117_020000.sql"), b"existing").unwrap();

        let mut fn_rule = base_rule("fn-rule", watch_dir.clone());
        fn_rule.time_basis = TimeBasis::FilenameDate;
        fn_rule.filename_date_regex = Some(r"pg_(\d{8}_\d{6})".to_string());
        fn_rule.filename_date_format = Some("%Y%m%d_%H%M%S".to_string());
        fn_rule.max_files = Some(1);
        fn_rule.actions = vec![ActionStep::Move];
        fn_rule.move_to = Some(dest_dir.clone());
        fn_rule.pre_delete_hook = Some(HookConfig {
            command: "/bin/true".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });

        // 2. Second rule constrained strictly by max_files < min_keep_files? Wait: config.validate
        // enforces max_files >= min_keep_files, so simulate constrained via 1 floor + 1 in-use > max_files(1).
        let constrained_dir = tmp.path().join("constrained_files");
        fs::create_dir_all(&constrained_dir).unwrap();
        let c_floor = constrained_dir.join("c1.sql");
        let c_busy = constrained_dir.join("c2.sql");
        fs::write(&c_floor, b"floor-backup").unwrap();
        fs::write(&c_busy, b"busy-backup").unwrap();
        set_mtime_unix(&c_floor, now.timestamp() - 10);
        set_mtime_unix(&c_busy, now.timestamp() - 100);

        let mut locked = HashSet::new();
        locked.insert(c_busy.clone());
        let probe = MapProbe { locked };

        let mut c_rule = base_rule("c-files", constrained_dir);
        c_rule.max_files = Some(1);
        c_rule.in_use.check_os_locks = true;
        c_rule.backup_guard.enabled = true;
        c_rule.backup_guard.min_keep_files = 1;

        let audit_path = tmp.path().join("audit_override.jsonl");
        let cfg = Config {
            rules: vec![fn_rule, c_rule],
            ..Config::default()
        };

        let mut verdicts = HashMap::new();
        verdicts.insert(
            "pg_20261118_020000.sql".to_string(),
            Err(FileCleanerError::Daemon("abort single file".to_string())),
        );
        verdicts.insert(
            "pg_20261116_020000.sql".to_string(),
            Ok(HookVerdict::WarnAndContinue {
                reason: "tolerated warning".to_string(),
            }),
        );
        let runner = ScriptedHookRunner { verdicts };

        let report = SweepEngine::run_config(
            &cfg,
            &RunOptions {
                audit_log_override: Some(audit_path.clone()),
                ..RunOptions::default()
            },
            &FixedClock(now),
            &probe,
            &runner,
        )
        .unwrap();

        assert_eq!(report.exit_code, EXIT_OPERATIONAL_ERROR);
        assert_eq!(report.rule_reports[0].summary.outcome, "error");
        assert_eq!(report.rule_reports[0].hook_warnings, 1);
        assert_eq!(report.rule_reports[0].acted_files, 1);
        assert!(unparseable.exists());
        assert!(f_abort.exists());
        assert!(f_conflict.exists());
        assert!(!f_warn_ok.exists());
        assert!(dest_dir.join("pg_20261116_020000.sql").exists());

        assert_eq!(report.rule_reports[1].summary.outcome, "constrained");
        assert!(report.rule_reports[1]
            .summary
            .error
            .as_deref()
            .unwrap()
            .contains("max_files exceeded"));

        let audit_log = fs::read_to_string(&audit_path).unwrap();
        assert!(audit_log.contains("skip_ts_parse"));
        assert!(audit_log.contains("warn_and_continue"));
    }

    #[cfg(unix)]
    #[test]
    fn engine_rejects_post_scan_symlink_swap_before_execute_pipeline() {
        struct SymlinkSwapHook {
            victim: PathBuf,
        }

        impl HookRunner for SymlinkSwapHook {
            fn run(
                &self,
                candidate: &HookCandidate,
                _hook: &HookConfig,
                _dry_run: bool,
            ) -> Result<HookVerdict> {
                fs::remove_file(&candidate.path).unwrap();
                std::os::unix::fs::symlink(&self.victim, &candidate.path).unwrap();
                Ok(HookVerdict::Proceed)
            }
        }

        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("watch");
        fs::create_dir_all(&watch_dir).unwrap();
        let victim = tmp.path().join("victim_secret.txt");
        fs::write(&victim, b"TOP_SECRET").unwrap();

        let target = watch_dir.join("evict.log");
        fs::write(&target, b"old-log").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        set_mtime_unix(&target, now.timestamp() - 3600);

        let mut rule = base_rule("swap-guard", watch_dir);
        rule.max_files = Some(0);
        rule.pre_delete_hook = Some(HookConfig {
            command: "/bin/true".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: None,
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        });

        let audit_path = tmp.path().join("swap_audit.jsonl");
        let cfg = Config {
            audit: AuditConfig {
                log_path: Some(audit_path.clone()),
                jsonl: true,
            },
            rules: vec![rule],
            ..Config::default()
        };

        let report = SweepEngine::run_config(
            &cfg,
            &RunOptions::default(),
            &FixedClock(now),
            &OsLockProbe,
            &SymlinkSwapHook {
                victim: victim.clone(),
            },
        )
        .unwrap();

        assert_eq!(report.exit_code, EXIT_OPERATIONAL_ERROR);
        assert!(report.has_operational_error);
        assert_eq!(report.rule_reports[0].acted_files, 0);
        assert!(report.rule_reports[0].errors[0].contains("refusing symlink target"));
        assert_eq!(fs::read(&victim).unwrap(), b"TOP_SECRET");
        assert!(fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::read_to_string(&audit_path)
            .unwrap()
            .contains("\"error\""));
    }
}
