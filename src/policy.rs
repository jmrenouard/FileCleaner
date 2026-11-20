//! Formal retention and backup-safety planning engine (Phases 3 & 4).

use crate::config::{CombineMode, EvictionOrder, RuleConfig};
use crate::in_use::InUseBatchResult;
use crate::scanner::{Clock, ScanReport, ScannedFile};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Overall status of a rule sweep evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOutcome {
    /// All retention constraints were satisfied without blocking safety invariants.
    Ok,
    /// Staleness dead-man's-switch or empty-valid-backup guard froze all evictions (`E = empty`).
    Frozen,
    /// `max_files` or `max_total_size` remained exceeded after evicting everything outside `P_floor U S_busy`.
    Constrained,
}

/// Per-file classification emitted inside [`SweepPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Protected by `backup_guard.min_keep_files` (`P_floor`).
    KeepFloor,
    /// Retained because retention thresholds (`max_files`, `max_age`, `max_total_size`) allow keeping it.
    KeepRetention,
    /// Skipped because an active-file probe or fail-closed error placed it in `S_busy`.
    SkipInUse,
    /// Frozen because `backup_guard` detected missing or stale valid backups.
    FreezeBackupGuard,
    /// Selected for eviction and ordered execution by the action pipeline.
    WouldEvict,
}

/// Decision explanation bound to an individual scanned candidate file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDecision {
    /// Candidate file metadata.
    pub file: ScannedFile,
    /// Retention/safety verdict for this candidate.
    pub decision: DecisionKind,
    /// Human-readable explanation of the decision.
    pub reason: String,
}

/// Deterministic execution plan computed identically for `plan`, `--dry-run`, and live `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepPlan {
    /// Name of the evaluated rule.
    pub rule_name: String,
    /// Final rule status (`Ok`, `Frozen`, or `Constrained`).
    pub outcome: RuleOutcome,
    /// Explanation when `outcome == RuleOutcome::Frozen`.
    pub freeze_reason: Option<String>,
    /// Total candidates discovered in `F`.
    pub total_candidates: usize,
    /// Count of candidates meeting `size_bytes >= min_valid_backup_bytes` (`|F_valid|`).
    pub valid_backup_count: usize,
    /// Count of valid backups unconditionally protected by `min_keep_files` (`|P_floor|`).
    pub protected_floor_count: usize,
    /// Count of non-floor candidates skipped because they were in `S_busy`.
    pub busy_skipped_count: usize,
    /// Evicted candidates ordered in configured [`EvictionOrder`] traversal sequence.
    pub evict_files: Vec<ScannedFile>,
    /// Per-candidate decision breakdown ordered newest-first matching `scan.files`.
    pub decisions: Vec<CandidateDecision>,
    /// Projected file count remaining after `evict_files` execute.
    pub remaining_files: usize,
    /// Projected total byte size remaining after `evict_files` execute.
    pub remaining_bytes: u64,
    /// Configured `rule.max_files` limit for diagnostics.
    pub target_max_files: Option<usize>,
    /// Configured `rule.max_total_size` limit for diagnostics.
    pub target_max_total_size: Option<u64>,
}

/// Stateless planner implementing the mathematical model of `SPECIFICATIONS.md` §4.
#[derive(Debug, Clone, Copy, Default)]
pub struct RetentionEngine;

impl RetentionEngine {
    /// Evaluates backup guards, `S_busy` exclusions, and retention thresholds to build a [`SweepPlan`].
    pub fn plan(
        rule: &RuleConfig,
        scan: &ScanReport,
        in_use: &InUseBatchResult,
        clock: &dyn Clock,
    ) -> SweepPlan {
        let mut ordered = scan.files.clone();
        ordered.sort_by(|a, b| {
            b.resolved_ts
                .cmp(&a.resolved_ts)
                .then_with(|| a.rel_path.cmp(&b.rel_path))
        });

        let total_candidates = ordered.len();
        let total_bytes: u64 = ordered.iter().map(|f| f.size_bytes).sum();

        if !rule.enabled {
            let decisions = ordered
                .into_iter()
                .map(|file| CandidateDecision {
                    file,
                    decision: DecisionKind::KeepRetention,
                    reason: "rule is disabled".to_string(),
                })
                .collect();
            return SweepPlan {
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::Ok,
                freeze_reason: None,
                total_candidates,
                valid_backup_count: 0,
                protected_floor_count: 0,
                busy_skipped_count: 0,
                evict_files: Vec::new(),
                decisions,
                remaining_files: total_candidates,
                remaining_bytes: total_bytes,
                target_max_files: rule.max_files,
                target_max_total_size: rule.max_total_size,
            };
        }

        let b_min = rule.backup_guard.min_valid_backup_bytes;
        let valid_files: Vec<&ScannedFile> =
            ordered.iter().filter(|f| f.size_bytes >= b_min).collect();
        let valid_backup_count = valid_files.len();

        let n_min = valid_backup_count.min(rule.backup_guard.min_keep_files);
        let floor_paths: HashSet<PathBuf> = valid_files
            .iter()
            .take(n_min)
            .map(|f| f.path.clone())
            .collect();

        let busy_paths: HashSet<PathBuf> = ordered
            .iter()
            .filter(|f| in_use.is_busy(&f.path))
            .map(|f| f.path.clone())
            .collect();

        let now = clock.now();
        if let Some(freeze_reason) = evaluate_backup_freeze(rule, &valid_files, now) {
            let busy_skipped_count = busy_paths.len();
            let decisions = ordered
                .into_iter()
                .map(|file| CandidateDecision {
                    file,
                    decision: DecisionKind::FreezeBackupGuard,
                    reason: freeze_reason.clone(),
                })
                .collect();
            return SweepPlan {
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::Frozen,
                freeze_reason: Some(freeze_reason),
                total_candidates,
                valid_backup_count,
                protected_floor_count: n_min,
                busy_skipped_count,
                evict_files: Vec::new(),
                decisions,
                remaining_files: total_candidates,
                remaining_bytes: total_bytes,
                target_max_files: rule.max_files,
                target_max_total_size: rule.max_total_size,
            };
        }

        let mut evictable: Vec<&ScannedFile> = Vec::new();
        let mut protected_count = 0usize;
        let mut protected_bytes = 0u64;
        let mut busy_skipped_count = 0usize;

        for file in &ordered {
            if floor_paths.contains(&file.path) {
                protected_count += 1;
                protected_bytes = protected_bytes.saturating_add(file.size_bytes);
            } else if busy_paths.contains(&file.path) {
                busy_skipped_count += 1;
                protected_count += 1;
                protected_bytes = protected_bytes.saturating_add(file.size_bytes);
            } else {
                evictable.push(file);
            }
        }

        sort_eviction_candidates(&mut evictable, rule.eviction_order);

        let mut remaining_files = total_candidates;
        let mut remaining_bytes = total_bytes;
        let mut evict_files: Vec<ScannedFile> = Vec::new();
        let mut evictable_decisions: HashMap<PathBuf, (DecisionKind, String)> =
            HashMap::with_capacity(evictable.len());

        for file in evictable {
            let (should_evict, reason) =
                evaluate_eviction_predicates(rule, file, remaining_files, remaining_bytes, now);

            if should_evict {
                remaining_files = remaining_files.saturating_sub(1);
                remaining_bytes = remaining_bytes.saturating_sub(file.size_bytes);
                evict_files.push(file.clone());
                evictable_decisions.insert(file.path.clone(), (DecisionKind::WouldEvict, reason));
            } else {
                evictable_decisions
                    .insert(file.path.clone(), (DecisionKind::KeepRetention, reason));
            }
        }

        let decisions = ordered
            .into_iter()
            .map(|file| {
                if floor_paths.contains(&file.path) {
                    CandidateDecision {
                        file,
                        decision: DecisionKind::KeepFloor,
                        reason: format!(
                            "protected in newest {n_min} valid backup(s) (min_keep_files={}, min_valid_backup_bytes={b_min})",
                            rule.backup_guard.min_keep_files
                        ),
                    }
                } else if busy_paths.contains(&file.path) {
                    let detail = format_busy_reasons(in_use, &file.path);
                    CandidateDecision {
                        file,
                        decision: DecisionKind::SkipInUse,
                        reason: detail,
                    }
                } else if let Some((decision, reason)) = evictable_decisions.remove(&file.path) {
                    CandidateDecision {
                        file,
                        decision,
                        reason,
                    }
                } else {
                    CandidateDecision {
                        file,
                        decision: DecisionKind::KeepRetention,
                        reason: "retained by default".to_string(),
                    }
                }
            })
            .collect();

        // Constrained when safety guards (P_floor U S_busy) keep count or size caps unsatisfied.
        let constrained_by_files = rule
            .max_files
            .is_some_and(|max_f| remaining_files > max_f && protected_count > max_f);
        let constrained_by_bytes = rule
            .max_total_size
            .is_some_and(|max_b| remaining_bytes > max_b && protected_bytes > max_b);
        let outcome = if constrained_by_files || constrained_by_bytes {
            RuleOutcome::Constrained
        } else {
            RuleOutcome::Ok
        };

        SweepPlan {
            rule_name: rule.name.clone(),
            outcome,
            freeze_reason: None,
            total_candidates,
            valid_backup_count,
            protected_floor_count: n_min,
            busy_skipped_count,
            evict_files,
            decisions,
            remaining_files,
            remaining_bytes,
            target_max_files: rule.max_files,
            target_max_total_size: rule.max_total_size,
        }
    }
}

fn evaluate_backup_freeze(
    rule: &RuleConfig,
    valid_files: &[&ScannedFile],
    now: DateTime<chrono::Utc>,
) -> Option<String> {
    let guard = &rule.backup_guard;
    let guard_active = guard.enabled
        || guard.min_keep_files > 0
        || guard.min_valid_backup_bytes > 0
        || guard.require_newer_within.is_some()
        || guard.freeze_if_newest_older_than.is_some();

    if !guard_active {
        return None;
    }

    let Some(newest_valid) = valid_files.first() else {
        if guard.freeze_on_empty_valid {
            return Some(format!(
                "backup_guard freeze: zero valid backups meet min_valid_backup_bytes={}",
                guard.min_valid_backup_bytes
            ));
        }
        if let Some(fresh_window) = guard.require_newer_within {
            return Some(format!(
                "backup_guard freeze: no valid backup available within require_newer_within={}s",
                fresh_window.as_secs()
            ));
        }
        if let Some(cutoff) = guard.freeze_if_newest_older_than {
            return Some(format!(
                "backup_guard freeze: no valid backup newer than cutoff {}",
                cutoff.to_rfc3339()
            ));
        }
        return None;
    };

    if let Some(fresh_window) = guard.require_newer_within {
        let age = now.signed_duration_since(newest_valid.resolved_ts);
        let exceeds = match chrono::Duration::from_std(fresh_window) {
            Ok(limit) => age > limit,
            Err(_) => false,
        };
        if exceeds {
            return Some(format!(
                "backup_guard freeze: newest valid backup '{}' at {} is {}s old (> require_newer_within {}s)",
                newest_valid.rel_path,
                newest_valid.resolved_ts.to_rfc3339(),
                age.num_seconds(),
                fresh_window.as_secs()
            ));
        }
    }

    if let Some(cutoff) = guard.freeze_if_newest_older_than {
        if newest_valid.resolved_ts < cutoff {
            return Some(format!(
                "backup_guard freeze: newest valid backup '{}' at {} predates freeze_if_newest_older_than {}",
                newest_valid.rel_path,
                newest_valid.resolved_ts.to_rfc3339(),
                cutoff.to_rfc3339()
            ));
        }
    }

    None
}

fn sort_eviction_candidates(candidates: &mut [&ScannedFile], order: EvictionOrder) {
    match order {
        EvictionOrder::OldestFirst => {
            candidates.sort_by(|a, b| {
                a.resolved_ts
                    .cmp(&b.resolved_ts)
                    .then_with(|| b.size_bytes.cmp(&a.size_bytes))
                    .then_with(|| a.rel_path.cmp(&b.rel_path))
            });
        }
        EvictionOrder::LargestFirst => {
            candidates.sort_by(|a, b| {
                b.size_bytes
                    .cmp(&a.size_bytes)
                    .then_with(|| a.resolved_ts.cmp(&b.resolved_ts))
                    .then_with(|| a.rel_path.cmp(&b.rel_path))
            });
        }
    }
}

fn evaluate_eviction_predicates(
    rule: &RuleConfig,
    file: &ScannedFile,
    remaining_files: usize,
    remaining_bytes: u64,
    now: DateTime<chrono::Utc>,
) -> (bool, String) {
    let mut matched: Vec<String> = Vec::new();
    let mut unmet: Vec<String> = Vec::new();
    let mut results: Vec<bool> = Vec::new();

    if let Some(max_files) = rule.max_files {
        let exceeded = remaining_files > max_files;
        results.push(exceeded);
        if exceeded {
            matched.push(format!("count {remaining_files} > max_files {max_files}"));
        } else {
            unmet.push(format!("count {remaining_files} <= max_files {max_files}"));
        }
    }

    if let Some(max_age) = rule.max_age {
        let age = now.signed_duration_since(file.resolved_ts);
        let exceeded = match chrono::Duration::from_std(max_age) {
            Ok(limit) => age > limit,
            Err(_) => false,
        };
        results.push(exceeded);
        if exceeded {
            matched.push(format!(
                "age {}s > max_age {}s",
                age.num_seconds(),
                max_age.as_secs()
            ));
        } else {
            unmet.push(format!(
                "age {}s <= max_age {}s",
                age.num_seconds(),
                max_age.as_secs()
            ));
        }
    }

    if let Some(max_total_size) = rule.max_total_size {
        let exceeded = remaining_bytes > max_total_size;
        results.push(exceeded);
        if exceeded {
            matched.push(format!(
                "total_bytes {remaining_bytes} > max_total_size {max_total_size}"
            ));
        } else {
            unmet.push(format!(
                "total_bytes {remaining_bytes} <= max_total_size {max_total_size}"
            ));
        }
    }

    if results.is_empty() {
        return if rule.actions.is_empty() {
            (
                false,
                "no retention threshold or action configured".to_string(),
            )
        } else {
            (
                true,
                "unconditional action pipeline with no retention caps".to_string(),
            )
        };
    }

    let should_evict = match rule.combine_mode {
        CombineMode::Any => results.iter().any(|&flag| flag),
        CombineMode::All => results.iter().all(|&flag| flag),
    };

    if should_evict {
        let prefix = match rule.combine_mode {
            CombineMode::Any => "combine=any triggered:",
            CombineMode::All => "combine=all satisfied:",
        };
        (true, format!("{prefix} {}", matched.join("; ")))
    } else {
        let reason = if unmet.is_empty() {
            "within retention limits".to_string()
        } else {
            format!("retained ({})", unmet.join("; "))
        };
        (false, reason)
    }
}

fn format_busy_reasons(in_use: &InUseBatchResult, path: &std::path::Path) -> String {
    match in_use.reports.get(path) {
        Some(report) if !report.reasons.is_empty() => {
            let parts: Vec<String> = report.reasons.iter().map(ToString::to_string).collect();
            format!("skipped in-use: {}", parts.join(", "))
        }
        _ => "skipped in-use".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ActionStep, BackupGuardConfig, CompressionFormat, InUseConfig, TimeBasis};
    use crate::in_use::{InUseReason, InUseReport};
    use crate::scanner::FixedClock;
    use chrono::TimeZone;
    use std::time::Duration;

    fn file(
        rel_path: &str,
        size_bytes: u64,
        age_secs: i64,
        now: DateTime<chrono::Utc>,
    ) -> ScannedFile {
        let ts = now - chrono::Duration::seconds(age_secs);
        let p = PathBuf::from(format!("/backups/{rel_path}"));
        ScannedFile {
            path: p,
            rel_path: rel_path.to_string(),
            name: rel_path.rsplit('/').next().unwrap_or(rel_path).to_string(),
            size_bytes,
            resolved_ts: ts,
            modified_ts: ts,
            created_ts: None,
        }
    }

    fn base_rule(name: &str) -> RuleConfig {
        RuleConfig {
            name: name.to_string(),
            path: PathBuf::from("/backups"),
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
        }
    }

    #[test]
    fn disabled_rule_evicts_nothing_and_returns_ok() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .unwrap();
        let mut rule = base_rule("disabled");
        rule.enabled = false;
        rule.max_files = Some(0);
        let scan = ScanReport {
            files: vec![file("a.sql", 100, 1000, now)],
            ..ScanReport::default()
        };
        let plan =
            RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
        assert_eq!(plan.outcome, RuleOutcome::Ok);
        assert!(plan.evict_files.is_empty());
        assert_eq!(plan.remaining_files, 1);
    }

    #[test]
    fn unconditional_actions_without_limits_evicts_non_protected_candidates() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .unwrap();
        let rule = base_rule("spool");
        let f1 = file("new.dat", 50, 10, now);
        let f2 = file("old.dat", 50, 100, now);
        let mut in_use = InUseBatchResult::default();
        in_use.reports.insert(
            f1.path.clone(),
            InUseReport {
                path: f1.path.clone(),
                is_busy: true,
                reasons: vec![InUseReason::OsFlock],
            },
        );
        let scan = ScanReport {
            files: vec![f1, f2.clone()],
            ..ScanReport::default()
        };
        let plan = RetentionEngine::plan(&rule, &scan, &in_use, &FixedClock(now));
        assert_eq!(plan.outcome, RuleOutcome::Ok);
        assert_eq!(plan.busy_skipped_count, 1);
        assert_eq!(plan.evict_files, vec![f2]);
        assert_eq!(plan.remaining_files, 1);
    }

    #[test]
    fn freeze_on_empty_valid_can_opt_out_when_no_freshness_threshold_set() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .unwrap();
        let mut rule = base_rule("stubs-only");
        rule.max_age = Some(Duration::from_secs(60));
        rule.backup_guard.enabled = true;
        rule.backup_guard.min_valid_backup_bytes = 100;
        rule.backup_guard.freeze_on_empty_valid = false;

        let stub = file("stub.sql", 0, 3600, now);
        let scan = ScanReport {
            files: vec![stub.clone()],
            ..ScanReport::default()
        };
        let plan =
            RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
        assert_eq!(plan.outcome, RuleOutcome::Ok);
        assert_eq!(plan.evict_files, vec![stub]);
    }

    #[test]
    fn empty_valid_freezes_when_freshness_or_cutoff_set_even_if_freeze_on_empty_false() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .unwrap();
        let mut rule_fresh = base_rule("empty-fresh");
        rule_fresh.backup_guard.enabled = true;
        rule_fresh.backup_guard.freeze_on_empty_valid = false;
        rule_fresh.backup_guard.require_newer_within = Some(Duration::from_secs(3600));

        let plan_fresh = RetentionEngine::plan(
            &rule_fresh,
            &ScanReport::default(),
            &InUseBatchResult::default(),
            &FixedClock(now),
        );
        assert_eq!(plan_fresh.outcome, RuleOutcome::Frozen);

        let mut rule_cutoff = base_rule("empty-cutoff");
        rule_cutoff.backup_guard.enabled = true;
        rule_cutoff.backup_guard.freeze_on_empty_valid = false;
        rule_cutoff.backup_guard.freeze_if_newest_older_than = Some(now);

        let plan_cutoff = RetentionEngine::plan(
            &rule_cutoff,
            &ScanReport::default(),
            &InUseBatchResult::default(),
            &FixedClock(now),
        );
        assert_eq!(plan_cutoff.outcome, RuleOutcome::Frozen);
    }

    #[test]
    fn empty_actions_and_busy_without_reason_details_and_huge_durations_handled_cleanly() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
            .unwrap();

        let mut rule_no_actions = base_rule("noop");
        rule_no_actions.actions = vec![];
        let f1 = file("a.sql", 100, 100, now);
        let f2 = file("b.sql", 100, 200, now);
        let mut in_use = InUseBatchResult::default();
        in_use.reports.insert(
            f2.path.clone(),
            InUseReport {
                path: f2.path.clone(),
                is_busy: true,
                reasons: vec![],
            },
        );
        let scan = ScanReport {
            files: vec![f1, f2],
            ..ScanReport::default()
        };
        let plan = RetentionEngine::plan(&rule_no_actions, &scan, &in_use, &FixedClock(now));
        assert_eq!(plan.outcome, RuleOutcome::Ok);
        assert!(plan.evict_files.is_empty());
        assert!(plan
            .decisions
            .iter()
            .any(|d| d.decision == DecisionKind::SkipInUse && d.reason == "skipped in-use"));

        // Huge max_age / require_newer_within exceeding chrono::Duration::MAX do not panic.
        let mut rule_huge = base_rule("huge-dur");
        rule_huge.max_age = Some(Duration::from_secs(u64::MAX));
        rule_huge.backup_guard.require_newer_within = Some(Duration::from_secs(u64::MAX));
        let plan_huge = RetentionEngine::plan(
            &rule_huge,
            &scan,
            &InUseBatchResult::default(),
            &FixedClock(now),
        );
        assert_eq!(plan_huge.outcome, RuleOutcome::Ok);
        assert!(plan_huge.evict_files.is_empty());
    }
}
