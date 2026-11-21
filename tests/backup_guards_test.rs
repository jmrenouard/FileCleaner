//! Integration tests for backup safety guardrails: 0-byte stub trap, staleness freezes,
//! protected backup floor, and RuleOutcome::Constrained resolution.

use chrono::{TimeZone, Utc};
use filecleaner::{
    ActionStep, BackupGuardConfig, CombineMode, CompressionFormat, DecisionKind, EvictionOrder,
    FixedClock, InUseBatchResult, InUseConfig, InUseReason, InUseReport, RetentionEngine,
    RuleConfig, RuleOutcome, ScanReport, ScannedFile, TimeBasis,
};
use std::path::PathBuf;
use std::time::Duration;

fn backup_rule(name: &str) -> RuleConfig {
    RuleConfig {
        name: name.to_string(),
        path: PathBuf::from("/var/backups/postgres"),
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
        backup_guard: BackupGuardConfig {
            enabled: true,
            min_keep_files: 3,
            min_valid_backup_bytes: 1_024,
            require_newer_within: Some(Duration::from_secs(36 * 3600)),
            freeze_if_newest_older_than: None,
            freeze_on_empty_valid: true,
        },
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

fn candidate(
    name: &str,
    size_bytes: u64,
    age_hours: i64,
    now: chrono::DateTime<Utc>,
) -> ScannedFile {
    let ts = now - chrono::Duration::hours(age_hours);
    ScannedFile {
        path: PathBuf::from(format!("/var/backups/postgres/{name}")),
        rel_path: name.to_string(),
        name: name.to_string(),
        size_bytes,
        resolved_ts: ts,
        modified_ts: ts,
        created_ts: None,
    }
}

#[test]
fn zero_byte_stub_trap_freezes_instead_of_counting_as_fresh_or_valid_backup() {
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let mut rule = backup_rule("pg-zero-byte-trap");
    rule.max_age = Some(Duration::from_secs(48 * 3600));

    // Newest file is a failed 0-byte dump (1h old), while newest valid backup is 40h old (> 36h!).
    let stub_new = candidate("pg_today_failed.sql", 0, 1, now);
    let valid_stale_1 = candidate("pg_d2.sql", 10_000, 40, now);
    let valid_stale_2 = candidate("pg_d4.sql", 10_000, 96, now);
    let valid_stale_3 = candidate("pg_d6.sql", 10_000, 144, now);

    let scan = ScanReport {
        files: vec![stub_new, valid_stale_1, valid_stale_2, valid_stale_3],
        ..ScanReport::default()
    };

    let plan = RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
    assert_eq!(plan.outcome, RuleOutcome::Frozen);
    assert!(plan.freeze_reason.is_some());
    assert!(plan.evict_files.is_empty());
    assert_eq!(plan.valid_backup_count, 3);
    assert!(plan
        .decisions
        .iter()
        .all(|d| d.decision == DecisionKind::FreezeBackupGuard));

    // When all files are 0-byte stubs, freeze_on_empty_valid freezes unconditionally.
    let stubs_only = ScanReport {
        files: vec![
            candidate("stub1.sql", 0, 1, now),
            candidate("stub2.sql", 512, 100, now),
        ],
        ..ScanReport::default()
    };
    let plan_empty = RetentionEngine::plan(
        &rule,
        &stubs_only,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(plan_empty.outcome, RuleOutcome::Frozen);
    assert_eq!(plan_empty.valid_backup_count, 0);
    assert!(plan_empty.evict_files.is_empty());
}

#[test]
fn valid_fresh_backup_allows_cleaning_old_stubs_while_protecting_floor() {
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let mut rule = backup_rule("pg-floor-and-stub-cleanup");
    rule.max_files = Some(3);
    rule.max_age = Some(Duration::from_secs(72 * 3600));

    let v1 = candidate("pg_01.sql", 4096, 2, now);
    let stub = candidate("pg_02_failed.sql", 0, 10, now);
    let v2 = candidate("pg_03.sql", 4096, 26, now);
    let v3 = candidate("pg_04.sql", 4096, 50, now);
    let v4_old = candidate("pg_05.sql", 4096, 120, now);

    let scan = ScanReport {
        files: vec![
            v1.clone(),
            stub.clone(),
            v2.clone(),
            v3.clone(),
            v4_old.clone(),
        ],
        ..ScanReport::default()
    };

    let plan = RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
    assert_eq!(plan.outcome, RuleOutcome::Ok);
    assert_eq!(plan.valid_backup_count, 4);
    assert_eq!(plan.protected_floor_count, 3);

    // P_floor = {v1, v2, v3}. Both v4_old (oldest) and stub (outside P_floor) can be evicted cleanly!
    let evicted_names: Vec<&str> = plan.evict_files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(evicted_names, vec!["pg_05.sql", "pg_02_failed.sql"]);
    assert_eq!(plan.remaining_files, 3);
    for protected in [&v1, &v2, &v3] {
        assert!(!plan.evict_files.contains(protected));
    }
}

#[test]
fn freeze_if_newest_older_than_absolute_cutoff_and_empty_directory() {
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let cutoff = Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap();

    let mut rule = backup_rule("cutoff-guard");
    rule.backup_guard.require_newer_within = None;
    rule.backup_guard.freeze_if_newest_older_than = Some(cutoff);
    rule.max_files = Some(3);

    // Empty scan with active backup_guard freezes cleanly.
    let empty_plan = RetentionEngine::plan(
        &rule,
        &ScanReport::default(),
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(empty_plan.outcome, RuleOutcome::Frozen);
    assert!(empty_plan.evict_files.is_empty());

    // Newest valid backup is 2026-11-18 (< cutoff 2026-11-19) -> Frozen.
    let stale_scan = ScanReport {
        files: vec![
            candidate("pg_18.sql", 2048, 48, now),
            candidate("pg_17.sql", 2048, 72, now),
        ],
        ..ScanReport::default()
    };
    let frozen_plan = RetentionEngine::plan(
        &rule,
        &stale_scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(frozen_plan.outcome, RuleOutcome::Frozen);
    assert!(frozen_plan.evict_files.is_empty());

    // Exact cutoff instant (newest == cutoff) is NOT strictly older -> allowed.
    let at_cutoff = ScannedFile {
        resolved_ts: cutoff,
        modified_ts: cutoff,
        ..candidate("pg_19_cutoff.sql", 2048, 36, now)
    };
    let ok_scan = ScanReport {
        files: vec![
            at_cutoff,
            candidate("pg_16.sql", 2048, 96, now),
            candidate("pg_15.sql", 2048, 120, now),
            candidate("pg_14.sql", 2048, 144, now),
        ],
        ..ScanReport::default()
    };
    let ok_plan = RetentionEngine::plan(
        &rule,
        &ok_scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(ok_plan.outcome, RuleOutcome::Ok);
    assert_eq!(ok_plan.evict_files.len(), 1);
    assert_eq!(ok_plan.evict_files[0].name, "pg_14.sql");
}

#[test]
fn constrained_outcome_when_min_keep_files_or_in_use_blocks_size_or_count_caps() {
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

    // Case 1: max_total_size = 5000B, but min_keep_files = 3 of 3000B each = 9000B.
    let mut rule_floor_cap = backup_rule("floor-vs-size");
    rule_floor_cap.backup_guard.require_newer_within = None;
    rule_floor_cap.backup_guard.min_keep_files = 3;
    rule_floor_cap.max_total_size = Some(5_000);

    let f1 = candidate("b1.sql", 3_000, 1, now);
    let f2 = candidate("b2.sql", 3_000, 2, now);
    let f3 = candidate("b3.sql", 3_000, 3, now);
    let f4_old = candidate("b4_old.sql", 3_000, 10, now);

    let scan = ScanReport {
        files: vec![f1, f2, f3, f4_old.clone()],
        ..ScanReport::default()
    };
    let plan = RetentionEngine::plan(
        &rule_floor_cap,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(plan.outcome, RuleOutcome::Constrained);
    assert_eq!(plan.evict_files, vec![f4_old]);
    assert_eq!(plan.protected_floor_count, 3);
    assert_eq!(plan.remaining_files, 3);
    assert_eq!(plan.remaining_bytes, 9_000);
    assert_eq!(plan.target_max_total_size, Some(5_000));

    // Case 2: max_files = 1, no backup_guard floor, 2 files marked in-use.
    let mut rule_busy_cap = backup_rule("busy-vs-count");
    rule_busy_cap.backup_guard = BackupGuardConfig::default();
    rule_busy_cap.max_files = Some(1);

    let b1 = candidate("busy1.sql", 1_000, 10, now);
    let b2 = candidate("busy2.sql", 1_000, 20, now);
    let free = candidate("free_old.sql", 1_000, 30, now);

    let mut in_use = InUseBatchResult::default();
    for busy in [&b1, &b2] {
        in_use.reports.insert(
            busy.path.clone(),
            InUseReport {
                path: busy.path.clone(),
                is_busy: true,
                reasons: vec![InUseReason::OsFlock],
            },
        );
    }

    let scan2 = ScanReport {
        files: vec![b1, b2, free.clone()],
        ..ScanReport::default()
    };
    let plan2 = RetentionEngine::plan(&rule_busy_cap, &scan2, &in_use, &FixedClock(now));
    assert_eq!(plan2.outcome, RuleOutcome::Constrained);
    assert_eq!(plan2.busy_skipped_count, 2);
    assert_eq!(plan2.evict_files, vec![free]);
    assert_eq!(plan2.remaining_files, 2);
    assert_eq!(plan2.target_max_files, Some(1));
}
