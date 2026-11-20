//! Integration tests for directory scanning, filtering, TimeBasis resolution, and retention planning.

use chrono::{TimeZone, Utc};
use filecleaner::{
    ActionStep, BackupGuardConfig, CombineMode, CompressionFormat, DaemonConfig, DecisionKind,
    EvictionOrder, FixedClock, InUseBatchResult, InUseConfig, RetentionEngine, RuleConfig,
    RuleOutcome, Scanner, TimeBasis,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn make_rule(name: &str, path: PathBuf) -> RuleConfig {
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

fn write_sized_with_age(
    dir: &Path,
    rel: &str,
    size: usize,
    now_secs: i64,
    age_secs: i64,
) -> PathBuf {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, vec![b'a'; size]).unwrap();
    let f = fs::OpenOptions::new().write(true).open(&full).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs((now_secs - age_secs) as u64))
        .unwrap();
    full
}

#[test]
fn retention_max_files_max_age_and_max_total_size_with_combine_any_and_all() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("logs");
    fs::create_dir_all(&root).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let now_s = now.timestamp();

    // 5 files: ages 1d, 10d, 25d, 40d, 60d; sizes 100B each.
    write_sized_with_age(&root, "f1_1d.log", 100, now_s, 86_400);
    write_sized_with_age(&root, "f2_10d.log", 100, now_s, 10 * 86_400);
    write_sized_with_age(&root, "f3_25d.log", 100, now_s, 25 * 86_400);
    write_sized_with_age(&root, "f4_40d.log", 100, now_s, 40 * 86_400);
    write_sized_with_age(&root, "f5_60d.log", 100, now_s, 60 * 86_400);

    // 1. max_files = 3 (OldestFirst) -> evicts 60d and 40d.
    let mut rule_files = make_rule("by-count", root.clone());
    rule_files.max_files = Some(3);
    let scan = Scanner::scan_rule(&rule_files, None, &DaemonConfig::default()).unwrap();
    let plan = RetentionEngine::plan(
        &rule_files,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(plan.outcome, RuleOutcome::Ok);
    let evicted: Vec<&str> = plan.evict_files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(evicted, vec!["f5_60d.log", "f4_40d.log"]);
    assert_eq!(plan.remaining_files, 3);
    assert_eq!(plan.remaining_bytes, 300);

    // 2. max_age = 30d -> evicts 60d and 40d; 25d retained.
    let mut rule_age = make_rule("by-age", root.clone());
    rule_age.max_age = Some(Duration::from_secs(30 * 86_400));
    let plan_age = RetentionEngine::plan(
        &rule_age,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    let evicted_age: Vec<&str> = plan_age
        .evict_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(evicted_age, vec!["f5_60d.log", "f4_40d.log"]);

    // 3. max_total_size = 250B -> must evict 3 oldest files to drop 500 -> 200 <= 250.
    let mut rule_size = make_rule("by-size", root.clone());
    rule_size.max_total_size = Some(250);
    let plan_size = RetentionEngine::plan(
        &rule_size,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    let evicted_size: Vec<&str> = plan_size
        .evict_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(evicted_size, vec!["f5_60d.log", "f4_40d.log", "f3_25d.log"]);
    assert_eq!(plan_size.remaining_bytes, 200);

    // 4. CombineMode::Any (max_files = 4 OR max_age = 20d) -> evicts 60d, 40d, AND 25d.
    let mut rule_any = make_rule("combine-any", root.clone());
    rule_any.max_files = Some(4);
    rule_any.max_age = Some(Duration::from_secs(20 * 86_400));
    rule_any.combine_mode = CombineMode::Any;
    let plan_any = RetentionEngine::plan(
        &rule_any,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    let evicted_any: Vec<&str> = plan_any
        .evict_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(evicted_any, vec!["f5_60d.log", "f4_40d.log", "f3_25d.log"]);

    // 5. CombineMode::All (max_files = 4 AND max_age = 20d) -> stops at 4 files even though 40d > 20d.
    let mut rule_all = rule_any.clone();
    rule_all.combine_mode = CombineMode::All;
    let plan_all = RetentionEngine::plan(
        &rule_all,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    let evicted_all: Vec<&str> = plan_all
        .evict_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(evicted_all, vec!["f5_60d.log"]);
    assert_eq!(plan_all.outcome, RuleOutcome::Ok);
    assert_eq!(plan_all.remaining_files, 4);
}

#[test]
fn retention_eviction_order_largest_first_and_tie_breaking() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sizes");
    fs::create_dir_all(&root).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let now_s = now.timestamp();

    // Two 400B files at different ages, one 100B old file, one 200B newer file.
    write_sized_with_age(&root, "big_newer.bin", 400, now_s, 100);
    write_sized_with_age(&root, "big_older.bin", 400, now_s, 200);
    write_sized_with_age(&root, "small_oldest.bin", 100, now_s, 1000);
    write_sized_with_age(&root, "medium.bin", 200, now_s, 50);

    let mut rule = make_rule("largest-first", root);
    rule.max_total_size = Some(400);
    rule.eviction_order = EvictionOrder::LargestFirst;

    let scan = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
    let plan = RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
    // Total 1100B -> target <= 400B: evict big_older (400B, oldest tie-break), then big_newer (400B) -> 300B remaining.
    let evicted: Vec<&str> = plan.evict_files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(evicted, vec!["big_older.bin", "big_newer.bin"]);
    assert_eq!(plan.remaining_bytes, 300);
    assert_eq!(plan.remaining_files, 2);
}

#[test]
fn scanner_applies_include_exclude_precedence_and_excludes_fc_tmp_and_internal_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("spool");
    fs::create_dir_all(root.join("keep_dir")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let now_s = now.timestamp();

    write_sized_with_age(&root, "app.log", 200, now_s, 1000);
    write_sized_with_age(&root, "sub/worker.log", 200, now_s, 900);
    write_sized_with_age(&root, "sub/secret.log", 200, now_s, 800);
    write_sized_with_age(&root, "keep_dir/app.log", 200, now_s, 700);
    write_sized_with_age(&root, "ignored.txt", 200, now_s, 600);
    write_sized_with_age(&root, "app.log.fc-tmp", 200, now_s, 500);
    write_sized_with_age(&root, "app.log.fc-tmp.99.1", 200, now_s, 400);
    let audit = write_sized_with_age(&root, "audit.jsonl", 200, now_s, 300);
    let pid = write_sized_with_age(&root, "daemon.pid", 200, now_s, 200);
    let state = write_sized_with_age(&root, "daemon.json", 200, now_s, 100);
    let stop = write_sized_with_age(&root, "daemon.stop", 200, now_s, 50);

    let mut rule = make_rule("filter-algebra", root.clone());
    rule.recursive = true;
    rule.include_globs = vec!["*.log".to_string()];
    rule.include_regex = Some(r"^sub/.*\.log$".to_string());
    rule.exclude_globs = vec!["keep_dir/**".to_string()];
    rule.exclude_regex = Some(r"secret\.log$".to_string());

    let daemon = DaemonConfig {
        pid_file: Some(pid),
        state_file: Some(state),
        stop_file: Some(stop),
        ..DaemonConfig::default()
    };

    let report = Scanner::scan_rule(&rule, Some(&audit), &daemon).unwrap();
    let rels: Vec<&str> = report.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(rels, vec!["sub/worker.log", "app.log"]);
}

#[test]
fn scanner_filename_date_fail_closed_never_evicts_unparseable_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("db");
    fs::create_dir_all(&root).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let now_s = now.timestamp();

    // Give unparseable files very old filesystem mtime to prove FilenameDate does not fall back to mtime.
    let bad_date = write_sized_with_age(&root, "pg_20261345_000000.sql", 500, now_s, 999_999);
    let no_match = write_sized_with_age(&root, "readme.sql", 500, now_s, 999_999);
    write_sized_with_age(&root, "pg_20261110_020000.sql", 500, now_s, 10);
    write_sized_with_age(&root, "pg_20261119_020000.sql", 500, now_s, 10);

    let mut rule = make_rule("pg-fn-date", root);
    rule.time_basis = TimeBasis::FilenameDate;
    rule.filename_date_regex = Some(r"pg_(?P<date>\d{8}_\d{6})".to_string());
    rule.filename_date_format = Some("%Y%m%d_%H%M%S".to_string());
    rule.max_files = Some(1);

    let scan = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
    assert_eq!(scan.files.len(), 2);
    assert_eq!(scan.skipped_timestamp_parse.len(), 2);
    assert!(scan.skipped_timestamp_parse.contains(&bad_date));
    assert!(scan.skipped_timestamp_parse.contains(&no_match));

    let plan = RetentionEngine::plan(&rule, &scan, &InUseBatchResult::default(), &FixedClock(now));
    let evicted: Vec<&str> = plan.evict_files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(evicted, vec!["pg_20261110_020000.sql"]);
    assert_eq!(
        plan.decisions
            .iter()
            .filter(|d| d.decision == DecisionKind::WouldEvict)
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn scanner_skips_symlinks_by_default_and_detects_loops_when_enabled() {
    use filecleaner::FileCleanerError;
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("watched");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();

    let outside_file = outside.join("precious.sql");
    fs::write(&outside_file, b"do-not-touch").unwrap();
    let normal_file = root.join("local.sql");
    fs::write(&normal_file, b"local").unwrap();

    symlink(&outside_file, root.join("link_to_file.sql")).unwrap();
    symlink(&outside, root.join("link_to_dir")).unwrap();

    let mut rule = make_rule("symlink-safe", root.clone());
    rule.recursive = true;
    rule.follow_symlinks = false;

    let safe_scan = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
    assert_eq!(safe_scan.files.len(), 1);
    assert_eq!(safe_scan.files[0].rel_path, "local.sql");

    // Enabling follow_symlinks discovers symlink targets, and directory loops return Io error.
    rule.follow_symlinks = true;
    let followed_scan = Scanner::scan_rule(&rule, None, &DaemonConfig::default()).unwrap();
    assert_eq!(followed_scan.files.len(), 3);

    symlink(&root, root.join("loop_back")).unwrap();
    let loop_res = Scanner::scan_rule(&rule, None, &DaemonConfig::default());
    assert!(matches!(loop_res, Err(FileCleanerError::Io { .. })));
}

#[test]
fn retention_inplace_actions_keep_accurate_counts_and_copy_truncate_reclaims_bytes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("inplace");
    let rotated = tmp.path().join("rotated");
    fs::create_dir_all(&root).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let now_s = now.timestamp();
    for i in 1..=5 {
        write_sized_with_age(&root, &format!("app_{i}.log"), 100, now_s, i * 600);
    }

    let scan = Scanner::scan_rule(
        &make_rule("scan-seed", root.clone()),
        None,
        &DaemonConfig::default(),
    )
    .unwrap();
    assert_eq!(scan.files.len(), 5);

    // 1. RenameByDate does not remove files or reclaim bytes -> Constrained when caps exceeded.
    let mut rename_rule = make_rule("rename-cap", root.clone());
    rename_rule.actions = vec![ActionStep::RenameByDate];
    rename_rule.max_files = Some(2);
    rename_rule.max_total_size = Some(200);
    let rename_plan = RetentionEngine::plan(
        &rename_rule,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(rename_plan.remaining_files, 5);
    assert_eq!(rename_plan.remaining_bytes, 500);
    assert_eq!(rename_plan.evict_files.len(), 5);
    assert_eq!(rename_plan.outcome, RuleOutcome::Constrained);

    // 2. Compress (both remove_source=false and remove_source=true) preserves file count & byte
    //    accounting until post-compress sweep, signaling Constrained when caps remain exceeded.
    for remove_source in [false, true] {
        let mut comp_rule = make_rule("compress-cap", root.clone());
        comp_rule.actions = vec![ActionStep::Compress];
        comp_rule.compress_remove_source = remove_source;
        comp_rule.max_files = Some(2);
        comp_rule.max_total_size = Some(200);
        let comp_plan = RetentionEngine::plan(
            &comp_rule,
            &scan,
            &InUseBatchResult::default(),
            &FixedClock(now),
        );
        assert_eq!(comp_plan.remaining_files, 5);
        assert_eq!(comp_plan.remaining_bytes, 500);
        assert_eq!(comp_plan.evict_files.len(), 5);
        assert_eq!(comp_plan.outcome, RuleOutcome::Constrained);
    }

    // 3. CopyTruncate reclaims bytes (so max_total_size=250 evicts 3 -> 200B, Ok), while
    //    remaining_files stays 5 unless min_file_size > 0 filters out truncated 0B files.
    let mut ct_size = make_rule("ct-size", root.clone());
    ct_size.actions = vec![ActionStep::CopyTruncate];
    ct_size.copy_truncate_to = Some(rotated.clone());
    ct_size.max_total_size = Some(250);
    let ct_size_plan = RetentionEngine::plan(
        &ct_size,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(ct_size_plan.evict_files.len(), 3);
    assert_eq!(ct_size_plan.remaining_bytes, 200);
    assert_eq!(ct_size_plan.remaining_files, 5);
    assert_eq!(ct_size_plan.outcome, RuleOutcome::Ok);

    let mut ct_count_unfiltered = ct_size.clone();
    ct_count_unfiltered.max_files = Some(2);
    let ct_unfiltered_plan = RetentionEngine::plan(
        &ct_count_unfiltered,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(ct_unfiltered_plan.remaining_bytes, 0);
    assert_eq!(ct_unfiltered_plan.remaining_files, 5);
    assert_eq!(ct_unfiltered_plan.outcome, RuleOutcome::Constrained);

    let mut ct_count_filtered = ct_count_unfiltered;
    ct_count_filtered.min_file_size = Some(1);
    ct_count_filtered.max_total_size = None;
    let ct_filtered_plan = RetentionEngine::plan(
        &ct_count_filtered,
        &scan,
        &InUseBatchResult::default(),
        &FixedClock(now),
    );
    assert_eq!(ct_filtered_plan.evict_files.len(), 3);
    assert_eq!(ct_filtered_plan.remaining_files, 2);
    assert_eq!(ct_filtered_plan.remaining_bytes, 200);
    assert_eq!(ct_filtered_plan.outcome, RuleOutcome::Ok);
}
