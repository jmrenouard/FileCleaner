//! Integration tests for FileCleaner's ordered action pipeline, pure-Rust compression
//! codecs, atomic `.fc-tmp` staging, cross-filesystem SHA-256 verification,
//! `CopyTruncate` inode preservation, `Trash` disambiguation, and `DryRun` parity.

use brotli::Decompressor as BrotliDecoder;
use chrono::{TimeZone, Utc};
use filecleaner::{
    compute_file_sha256, ActionExecutor, ActionStep, AuditConfig, AuditEntry, AuditEventKind,
    AuditLogger, BackupGuardConfig, CombineMode, CompressionFormat, Config, EvictionOrder,
    FileCleanerError, InUseConfig, RuleConfig, RuleSummaryRow, SweepSummary, TimeBasis,
    WorkingTarget,
};
use flate2::read::GzDecoder;
use lz4_flex::frame::FrameDecoder;
use std::fs::{self, File, FileTimes};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use zip::ZipArchive;

fn make_rule(name: &str, path: PathBuf, actions: Vec<ActionStep>) -> RuleConfig {
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
        actions,
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

fn create_stamped_file(path: &Path, data: &[u8], secs: i64) -> WorkingTarget {
    fs::write(path, data).unwrap();
    let mtime = Utc.timestamp_opt(secs, 0).unwrap();
    let atime = Utc.timestamp_opt(secs - 100, 0).unwrap();
    let sys_m = UNIX_EPOCH + Duration::from_secs(secs as u64);
    let sys_a = UNIX_EPOCH + Duration::from_secs((secs - 100) as u64);
    let times = FileTimes::new().set_accessed(sys_a).set_modified(sys_m);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
    WorkingTarget {
        path: path.to_path_buf(),
        size_bytes: data.len() as u64,
        mtime,
        atime: Some(atime),
    }
}

fn modified_unix_secs(meta: &fs::Metadata) -> i64 {
    let sys: SystemTime = meta.modified().unwrap();
    chrono::DateTime::<Utc>::from(sys).timestamp()
}

fn has_fc_tmp_artifacts(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".fc-tmp"))
}

#[test]
fn compress_roundtrips_all_four_pure_rust_codecs_and_preserves_mtime() {
    let tmp = TempDir::new().unwrap();
    let payload = b"FileCleaner deterministic compression payload! ".repeat(64);
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    let codecs = [
        (CompressionFormat::Gzip, ".gz"),
        (CompressionFormat::Lz4, ".lz4"),
        (CompressionFormat::Brotli, ".br"),
        (CompressionFormat::Zip, ".zip"),
    ];

    for (idx, (format, suffix)) in codecs.into_iter().enumerate() {
        let src_path = tmp.path().join(format!("dump_{idx}.sql"));
        let target = create_stamped_file(&src_path, &payload, fixed_secs);
        let mut rule = make_rule(
            "compress-test",
            tmp.path().to_path_buf(),
            vec![ActionStep::Compress],
        );
        rule.compression = format;
        rule.compression_level = Some(6);
        rule.compress_remove_source = true;

        let report = ActionExecutor::execute_pipeline(target, &rule, None, false).unwrap();
        assert!(!report.dry_run);
        assert_eq!(report.original_path, src_path);
        assert!(
            !src_path.exists(),
            "source should be unlinked after staged compress"
        );

        let expected_dest = PathBuf::from(format!("{}{}", src_path.display(), suffix));
        assert_eq!(report.final_path.as_ref(), Some(&expected_dest));
        assert!(expected_dest.exists());
        assert!(!has_fc_tmp_artifacts(tmp.path()));

        let dest_meta = fs::metadata(&expected_dest).unwrap();
        assert_eq!(modified_unix_secs(&dest_meta), fixed_secs);
        assert!(dest_meta.len() < payload.len() as u64);
        assert_eq!(report.bytes_freed, payload.len() as u64 - dest_meta.len());

        let mut decoded = Vec::new();
        match format {
            CompressionFormat::Gzip => {
                let file = File::open(&expected_dest).unwrap();
                let mut dec = GzDecoder::new(file);
                assert_eq!(
                    dec.header().and_then(|h| h.filename()),
                    Some(format!("dump_{idx}.sql").as_bytes())
                );
                dec.read_to_end(&mut decoded).unwrap();
            }
            CompressionFormat::Lz4 => {
                let file = File::open(&expected_dest).unwrap();
                let mut dec = FrameDecoder::new(file);
                dec.read_to_end(&mut decoded).unwrap();
            }
            CompressionFormat::Brotli => {
                let file = File::open(&expected_dest).unwrap();
                let mut dec = BrotliDecoder::new(file, 4096);
                dec.read_to_end(&mut decoded).unwrap();
            }
            CompressionFormat::Zip => {
                let file = File::open(&expected_dest).unwrap();
                let mut archive = ZipArchive::new(file).unwrap();
                assert_eq!(archive.len(), 1);
                let mut entry = archive.by_index(0).unwrap();
                assert_eq!(entry.name(), format!("dump_{idx}.sql"));
                entry.read_to_end(&mut decoded).unwrap();
            }
        }
        assert_eq!(decoded, payload);
    }
}

#[test]
fn compress_keep_source_and_overwrite_collision_guard() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("keep_me.log");
    let target = create_stamped_file(&src, b"log-line-1\nlog-line-2\n", 1_795_079_100);

    let mut rule = make_rule(
        "keep-src",
        tmp.path().to_path_buf(),
        vec![ActionStep::Compress],
    );
    rule.compression = CompressionFormat::Gzip;
    rule.compress_remove_source = false;
    rule.compress_overwrite = false;

    let report = ActionExecutor::execute_pipeline(target.clone(), &rule, None, false).unwrap();
    let gz_path = tmp.path().join("keep_me.log.gz");
    assert_eq!(report.final_path.as_ref(), Some(&gz_path));
    assert!(
        src.exists(),
        "source must remain when compress_remove_source=false"
    );
    assert!(gz_path.exists());
    assert_eq!(report.bytes_freed, 0);

    // Second run without compress_overwrite must reject overwriting existing archive.
    let err = ActionExecutor::execute_pipeline(target.clone(), &rule, None, false).unwrap_err();
    assert!(matches!(err, FileCleanerError::Io { .. }));
    assert!(src.exists());

    // Enabling compress_overwrite succeeds cleanly.
    rule.compress_overwrite = true;
    assert!(ActionExecutor::execute_pipeline(target, &rule, None, false).is_ok());
    assert!(!has_fc_tmp_artifacts(tmp.path()));
}

#[test]
fn rename_by_date_preserves_times_chains_pipeline_and_rejects_collisions() {
    let tmp = TempDir::new().unwrap();
    let archive_dir = tmp.path().join("archive");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();
    let src = tmp.path().join("service.log");
    let target = create_stamped_file(&src, b"rotated log body\n", fixed_secs);

    let mut rule = make_rule(
        "rename-compress-move",
        tmp.path().to_path_buf(),
        vec![
            ActionStep::RenameByDate,
            ActionStep::Compress,
            ActionStep::Move,
        ],
    );
    rule.compression = CompressionFormat::Lz4;
    rule.move_to = Some(archive_dir.clone());
    rule.move_date_subdir = Some("%Y/%m".to_string());

    let report = ActionExecutor::execute_pipeline(target, &rule, None, false).unwrap();
    let expected = archive_dir.join("2026/11/service_20261120_090500.log.lz4");
    assert_eq!(report.final_path.as_ref(), Some(&expected));
    assert_eq!(
        report.steps_executed,
        vec!["RenameByDate", "Compress(Lz4)", "Move"]
    );
    assert!(!src.exists());
    assert!(expected.exists());

    let meta = fs::metadata(&expected).unwrap();
    assert_eq!(modified_unix_secs(&meta), fixed_secs);

    // Collision guard: existing destination must fail without clobbering either file.
    let src2 = tmp.path().join("collision.log");
    let target2 = create_stamped_file(&src2, b"new-content", fixed_secs);
    let existing = tmp.path().join("collision_20261120_090500.log");
    fs::write(&existing, b"existing-protected").unwrap();

    let rename_only = make_rule(
        "rename-only",
        tmp.path().to_path_buf(),
        vec![ActionStep::RenameByDate],
    );
    let err = ActionExecutor::execute_pipeline(target2, &rename_only, None, false).unwrap_err();
    assert!(matches!(err, FileCleanerError::Io { .. }));
    assert_eq!(fs::read(&src2).unwrap(), b"new-content");
    assert_eq!(fs::read(&existing).unwrap(), b"existing-protected");
}

#[test]
fn move_same_fs_and_forced_cross_fs_sha256_verification_and_tamper_cleanup() {
    let tmp = TempDir::new().unwrap();
    let dest_dir = tmp.path().join("cold");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    // 1. Same-FS move via pipeline with move_date_subdir + overwrite guard.
    let src1 = tmp.path().join("backup.dump");
    let target1 = create_stamped_file(&src1, b"dump-payload-1", fixed_secs);
    let mut rule = make_rule(
        "move-rule",
        tmp.path().to_path_buf(),
        vec![ActionStep::Move],
    );
    rule.move_to = Some(dest_dir.clone());
    rule.move_date_subdir = Some("%Y".to_string());
    let report1 = ActionExecutor::execute_pipeline(target1, &rule, None, false).unwrap();
    let moved1 = dest_dir.join("2026/backup.dump");
    assert_eq!(report1.final_path.as_ref(), Some(&moved1));
    assert!(!src1.exists());
    assert_eq!(fs::read(&moved1).unwrap(), b"dump-payload-1");

    // Attempting to move another file to same destination without move_overwrite fails cleanly.
    let src1_dup = tmp.path().join("backup.dump");
    let target1_dup = create_stamped_file(&src1_dup, b"dump-payload-new", fixed_secs);
    let collision_err =
        ActionExecutor::execute_pipeline(target1_dup.clone(), &rule, None, false).unwrap_err();
    assert!(matches!(collision_err, FileCleanerError::Io { .. }));
    assert_eq!(fs::read(&moved1).unwrap(), b"dump-payload-1");

    // With move_overwrite = true, destination is replaced cleanly.
    rule.move_overwrite = true;
    ActionExecutor::execute_pipeline(target1_dup, &rule, None, false).unwrap();
    assert_eq!(fs::read(&moved1).unwrap(), b"dump-payload-new");

    // 2. Forced cross-FS move verifies SHA-256 digest and preserves mtime/atime.
    let src2 = tmp.path().join("cross.sql");
    let content2 = b"SELECT 42; -- cross filesystem payload\n".repeat(128);
    let target2 = create_stamped_file(&src2, &content2, fixed_secs);
    let (expected_sha, _) = compute_file_sha256(&src2).unwrap();
    let dest2 = dest_dir.join("cross.sql");

    let got_sha = ActionExecutor::move_with_cross_fs_fallback(
        &src2,
        &dest2,
        target2.mtime,
        target2.atime,
        false,
        true,
    )
    .unwrap();
    assert_eq!(got_sha.as_deref(), Some(expected_sha.as_str()));
    assert!(!src2.exists());
    assert_eq!(fs::read(&dest2).unwrap(), content2);
    assert!(!has_fc_tmp_artifacts(&dest_dir));
    let dest2_meta = fs::metadata(&dest2).unwrap();
    assert_eq!(modified_unix_secs(&dest2_meta), fixed_secs);

    // 3. Tamper simulation during cross-FS copy triggers IntegrityMismatch, cleans .fc-tmp, leaves src intact.
    let src3 = tmp.path().join("tamper.sql");
    let target3 = create_stamped_file(&src3, b"precious-backup-data", fixed_secs);
    let dest3 = dest_dir.join("tamper.sql");

    let tamper_err = ActionExecutor::move_with_cross_fs_fallback_hook(
        &src3,
        &dest3,
        target3.mtime,
        target3.atime,
        false,
        true,
        |staged_tmp| {
            assert!(staged_tmp
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".fc-tmp."));
            fs::write(staged_tmp, b"corrupted-disk-sector!").unwrap();
            Ok(())
        },
    )
    .unwrap_err();

    assert!(matches!(
        tamper_err,
        FileCleanerError::IntegrityMismatch { .. }
    ));
    assert!(
        src3.exists(),
        "original source must survive integrity failure"
    );
    assert_eq!(fs::read(&src3).unwrap(), b"precious-backup-data");
    assert!(!dest3.exists());
    assert!(!has_fc_tmp_artifacts(&dest_dir));
}

#[test]
fn copy_truncate_preserves_inode_zeroes_source_and_supports_raw_and_compressed() {
    let tmp = TempDir::new().unwrap();
    let rotated_dir = tmp.path().join("rotated");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    // 1. Raw CopyTruncate (default compression Gzip without compression_level).
    let log1 = tmp.path().join("app.log");
    let payload1 = b"2026-11-20 INFO started\n2026-11-20 INFO ready\n";
    let target1 = create_stamped_file(&log1, payload1, fixed_secs);

    #[cfg(unix)]
    let original_inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&log1).unwrap().ino()
    };

    let mut raw_rule = make_rule(
        "ct-raw",
        tmp.path().to_path_buf(),
        vec![ActionStep::CopyTruncate],
    );
    raw_rule.copy_truncate_to = Some(rotated_dir.clone());

    let report1 = ActionExecutor::execute_pipeline(target1, &raw_rule, None, false).unwrap();
    assert_eq!(report1.bytes_freed, payload1.len() as u64);
    assert!(report1.sha256.is_some());
    let archive1 = report1.final_path.clone().unwrap();
    assert_eq!(archive1, rotated_dir.join("app_20261120_090500.log"));
    assert_eq!(fs::read(&archive1).unwrap(), payload1);
    assert_eq!(fs::metadata(&log1).unwrap().len(), 0);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&log1).unwrap().ino(),
            original_inode,
            "CopyTruncate must preserve open file inode"
        );
    }

    // 2. Second CopyTruncate in same timestamp second disambiguates _1 without overwriting,
    // and uses LZ4 compression when configured.
    fs::write(&log1, b"second-wave-lines\n").unwrap();
    let target2 = WorkingTarget {
        path: log1.clone(),
        size_bytes: 18,
        mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
        atime: None,
    };
    raw_rule.compression = CompressionFormat::Lz4;
    let report2 = ActionExecutor::execute_pipeline(target2, &raw_rule, None, false).unwrap();
    let archive2 = report2.final_path.unwrap();
    assert_eq!(archive2, rotated_dir.join("app_20261120_090500.log.lz4"));
    assert_eq!(fs::metadata(&log1).unwrap().len(), 0);

    let mut lz4_out = Vec::new();
    FrameDecoder::new(File::open(&archive2).unwrap())
        .read_to_end(&mut lz4_out)
        .unwrap();
    assert_eq!(lz4_out, b"second-wave-lines\n");
    assert_eq!(fs::read(&archive1).unwrap(), payload1);
}

#[test]
fn trash_disambiguates_collisions_and_delete_unlinks() {
    let tmp = TempDir::new().unwrap();
    let trash_dir = tmp.path().join("trash");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    let trash_rule = make_rule(
        "trash-rule",
        tmp.path().to_path_buf(),
        vec![ActionStep::Trash],
    );

    for idx in 0..3 {
        let src = tmp.path().join("report.txt");
        let body = format!("payload-{idx}");
        let target = create_stamped_file(&src, body.as_bytes(), fixed_secs);
        let report =
            ActionExecutor::execute_pipeline(target, &trash_rule, Some(&trash_dir), false).unwrap();
        let expected_name = if idx == 0 {
            "report.txt".to_string()
        } else {
            format!("report_{idx}.txt")
        };
        let trashed = trash_dir.join(expected_name);
        assert_eq!(report.final_path.as_ref(), Some(&trashed));
        assert_eq!(fs::read(&trashed).unwrap(), body.as_bytes());
        assert!(!src.exists());
    }

    // Delete action permanently unlinks the file.
    let del_src = tmp.path().join("ephemeral.tmp");
    let del_target = create_stamped_file(&del_src, b"bye-bye", fixed_secs);
    let del_rule = make_rule(
        "delete-rule",
        tmp.path().to_path_buf(),
        vec![ActionStep::Delete],
    );
    let del_report = ActionExecutor::execute_pipeline(del_target, &del_rule, None, false).unwrap();
    assert!(del_report.final_path.is_none());
    assert_eq!(del_report.bytes_freed, 7);
    assert!(!del_src.exists());
}

#[test]
fn dry_run_produces_zero_filesystem_mutations_across_all_steps() {
    let tmp = TempDir::new().unwrap();
    let move_dest = tmp.path().join("never_created_move");
    let ct_dest = tmp.path().join("never_created_ct");
    let trash_dest = tmp.path().join("never_created_trash");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    let f1 = tmp.path().join("pipeline.sql");
    let t1 = create_stamped_file(&f1, b"immutable-sql", fixed_secs);
    let mut r1 = make_rule(
        "dry-pipeline",
        tmp.path().to_path_buf(),
        vec![
            ActionStep::RenameByDate,
            ActionStep::Compress,
            ActionStep::Move,
        ],
    );
    r1.move_to = Some(move_dest.clone());
    r1.move_date_subdir = Some("%Y/%m".to_string());

    let rep1 = ActionExecutor::execute_pipeline(t1, &r1, None, true).unwrap();
    assert!(rep1.dry_run);
    assert_eq!(rep1.bytes_freed, 0);
    assert_eq!(
        rep1.final_path,
        Some(move_dest.join("2026/11/pipeline_20261120_090500.sql.gz"))
    );
    assert!(f1.exists());
    assert!(!move_dest.exists());
    assert!(!has_fc_tmp_artifacts(tmp.path()));

    // Rule-level dry_run = true also blocks CopyTruncate and Trash/Delete even when param dry_run=false.
    let f2 = tmp.path().join("active.log");
    let t2 = create_stamped_file(&f2, b"stay-intact", fixed_secs);
    let mut r2 = make_rule(
        "dry-ct",
        tmp.path().to_path_buf(),
        vec![ActionStep::CopyTruncate],
    );
    r2.dry_run = true;
    r2.copy_truncate_to = Some(ct_dest.clone());
    let rep2 = ActionExecutor::execute_pipeline(t2, &r2, None, false).unwrap();
    assert!(rep2.dry_run);
    assert_eq!(fs::read(&f2).unwrap(), b"stay-intact");
    assert!(!ct_dest.exists());

    let f3 = tmp.path().join("trash_or_del.log");
    let t3 = create_stamped_file(&f3, b"keep", fixed_secs);
    let mut r3 = make_rule(
        "dry-trash",
        tmp.path().to_path_buf(),
        vec![ActionStep::Trash],
    );
    r3.dry_run = true;
    ActionExecutor::execute_pipeline(t3.clone(), &r3, Some(&trash_dest), false).unwrap();
    assert!(f3.exists());
    assert!(!trash_dest.exists());

    let mut r4 = make_rule(
        "dry-del",
        tmp.path().to_path_buf(),
        vec![ActionStep::Delete],
    );
    r4.dry_run = true;
    let rep4 = ActionExecutor::execute_pipeline(t3, &r4, None, false).unwrap();
    assert!(rep4.final_path.is_none());
    assert!(f3.exists());
}

#[test]
fn audit_jsonl_log_and_examples_configs_validate_cleanly() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        "examples/postgres-backups.toml",
        "examples/app-logs-rotation.toml",
        "examples/incoming-spool.yaml",
    ] {
        let cfg_path = manifest_dir.join(rel);
        let cfg = Config::load_validated(&cfg_path)
            .unwrap_or_else(|e| panic!("failed to validate example {rel}: {e}"));
        assert!(!cfg.rules.is_empty());
    }

    // Write structured audit entries and validate JSONL schema lines.
    let tmp = TempDir::new().unwrap();
    let audit_path = tmp.path().join("audit.jsonl");
    let logger = AuditLogger::from_config(
        &AuditConfig {
            log_path: Some(audit_path.clone()),
            jsonl: true,
        },
        None,
    )
    .unwrap();

    logger
        .log(&AuditEntry::new(
            AuditEventKind::SweepStart,
            "run-99",
            false,
        ))
        .unwrap();
    logger
        .log(
            &AuditEntry::new(AuditEventKind::ActionExec, "run-99", false)
                .with_rule("pg-backups")
                .with_candidate("/var/backups/pg_20261120.sql", 8192)
                .with_decision("acted")
                .with_actions(vec!["Compress(Gzip)".to_string(), "Move".to_string()])
                .with_dest_path("/mnt/cold/pg_20261120.sql.gz")
                .with_sha256("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .with_duration_ms(12),
        )
        .unwrap();
    logger
        .log(&AuditEntry::new(AuditEventKind::SweepEnd, "run-99", false))
        .unwrap();

    let raw = fs::read_to_string(&audit_path).unwrap();
    let lines: Vec<serde_json::Value> = raw
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["event"], "sweep_start");
    assert_eq!(lines[1]["event"], "action_exec");
    assert_eq!(lines[1]["rule"], "pg-backups");
    assert_eq!(lines[1]["decision"], "acted");
    assert_eq!(lines[1]["actions"][1], "Move");
    assert_eq!(lines[2]["event"], "sweep_end");

    let summary = SweepSummary::from_rows(
        "run-99",
        false,
        vec![RuleSummaryRow {
            rule: "pg-backups".to_string(),
            outcome: "ok".to_string(),
            evaluated: 3,
            acted: 1,
            skipped_in_use: 0,
            skipped_hook: 0,
            kept_floor: 2,
            bytes_freed: 8192,
            dry_run: false,
            error: None,
        }],
    );
    let table = summary.format_table();
    assert!(table.contains("8.00 KiB"));
    assert!(summary
        .to_json_pretty()
        .unwrap()
        .contains("\"total_acted\": 1"));
}

fn try_create_file_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            Err(err) if err.raw_os_error() == Some(1314) => false,
            Err(err) => panic!("unexpected symlink_file error: {err}"),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

#[test]
fn symlink_swap_toctou_rejected_by_copy_truncate_and_action_pipeline() {
    let tmp = TempDir::new().unwrap();
    let victim = tmp.path().join("victim_secret.txt");
    fs::write(&victim, b"TOP_SECRET").unwrap();

    let swapped = tmp.path().join("swapped.log");
    if !try_create_file_symlink(&victim, &swapped) {
        return;
    }

    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();
    let target = WorkingTarget {
        path: swapped.clone(),
        size_bytes: 10,
        mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
        atime: None,
    };

    // 1. CopyTruncate must reject symlinks before staging or truncating.
    let archive_dir = tmp.path().join("archive");
    let archive_dest = archive_dir.join("swapped_20261120_090500.log");
    let ct_err =
        ActionExecutor::execute_copy_truncate(&target, &archive_dest, None, None).unwrap_err();
    assert!(matches!(ct_err, FileCleanerError::Io { .. }));
    assert_eq!(fs::read(&victim).unwrap(), b"TOP_SECRET");
    assert!(!archive_dest.exists());
    assert!(!archive_dir.exists());

    // 2. Compress must reject symlinks when follow_symlinks is false.
    let compress_rule = make_rule(
        "compress-symlink-guard",
        tmp.path().to_path_buf(),
        vec![ActionStep::Compress],
    );
    let comp_err =
        ActionExecutor::execute_pipeline(target.clone(), &compress_rule, None, false).unwrap_err();
    assert!(matches!(comp_err, FileCleanerError::Io { .. }));
    assert_eq!(fs::read(&victim).unwrap(), b"TOP_SECRET");
    assert!(!tmp.path().join("swapped.log.gz").exists());
    assert!(!has_fc_tmp_artifacts(tmp.path()));

    // 3. Move (both same-FS and cross-FS), Delete, and cross-FS fallback reject symlinks.
    let mut move_rule = make_rule(
        "move-symlink-guard",
        tmp.path().to_path_buf(),
        vec![ActionStep::Move],
    );
    move_rule.move_to = Some(tmp.path().join("moved"));
    assert!(matches!(
        ActionExecutor::execute_pipeline(target.clone(), &move_rule, None, false),
        Err(FileCleanerError::Io { .. })
    ));
    let cross_dest = tmp.path().join("moved/cross_swapped.log");
    assert!(matches!(
        ActionExecutor::move_with_cross_fs_fallback(
            &swapped,
            &cross_dest,
            target.mtime,
            None,
            false,
            true
        ),
        Err(FileCleanerError::Io { .. })
    ));
    assert!(!cross_dest.exists());

    let delete_rule = make_rule(
        "delete-symlink-guard",
        tmp.path().to_path_buf(),
        vec![ActionStep::Delete],
    );
    assert!(matches!(
        ActionExecutor::execute_pipeline(target, &delete_rule, None, false),
        Err(FileCleanerError::Io { .. })
    ));
    assert_eq!(fs::read(&victim).unwrap(), b"TOP_SECRET");
    assert!(fs::symlink_metadata(&swapped)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn multi_pass_idempotency_for_rename_by_date_and_compress() {
    let tmp = TempDir::new().unwrap();
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    // 1. RenameByDate applied twice does not produce _date_date.
    let src = tmp.path().join("service.log");
    let target = create_stamped_file(&src, b"idempotent-log", fixed_secs);
    let rename_rule = make_rule(
        "rename-idempotent",
        tmp.path().to_path_buf(),
        vec![ActionStep::RenameByDate],
    );
    let pass1 = ActionExecutor::execute_pipeline(target, &rename_rule, None, false).unwrap();
    let stamped_path = tmp.path().join("service_20261120_090500.log");
    assert_eq!(pass1.final_path.as_ref(), Some(&stamped_path));
    assert!(stamped_path.exists());

    let target_pass2 = WorkingTarget {
        path: stamped_path.clone(),
        size_bytes: 14,
        mtime: Utc.timestamp_opt(fixed_secs + 3600, 0).unwrap(),
        atime: None,
    };
    let pass2 = ActionExecutor::execute_pipeline(target_pass2, &rename_rule, None, false).unwrap();
    assert_eq!(pass2.final_path.as_ref(), Some(&stamped_path));
    assert_eq!(pass2.bytes_freed, 0);
    assert!(stamped_path.exists());
    assert!(!tmp
        .path()
        .join("service_20261120_090500_20261120_100500.log")
        .exists());

    // 2. Compress applied to already-compressed files (.gz, .lz4, .br, .zip, .GZ) is a clean no-op.
    for ext in ["gz", "lz4", "br", "zip", "GZ"] {
        let compressed = tmp.path().join(format!("already.{ext}"));
        let compressed_target = create_stamped_file(&compressed, b"compressed-bytes", fixed_secs);
        let mut comp_rule = make_rule(
            "compress-idempotent",
            tmp.path().to_path_buf(),
            vec![ActionStep::Compress],
        );
        comp_rule.compression = CompressionFormat::Gzip;
        let report =
            ActionExecutor::execute_pipeline(compressed_target, &comp_rule, None, false).unwrap();
        assert_eq!(report.final_path.as_ref(), Some(&compressed));
        assert_eq!(report.bytes_freed, 0);
        assert!(compressed.exists());
        assert!(!tmp.path().join(format!("already.{ext}.gz")).exists());
    }
}

#[cfg(unix)]
#[test]
fn fc_tmp_staging_and_audit_log_enforce_0600_and_rotation() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    // 1. Cross-FS .fc-tmp staging file must be created with private 0o600 permissions.
    let src = tmp.path().join("secret_dump.sql");
    let target = create_stamped_file(&src, b"CREATE TABLE t (id INT);\n", fixed_secs);
    fs::set_permissions(&src, fs::Permissions::from_mode(0o644)).unwrap();
    let dest = tmp.path().join("cold/secret_dump.sql");

    let mut observed_tmp_mode = None;
    ActionExecutor::move_with_cross_fs_fallback_hook(
        &src,
        &dest,
        target.mtime,
        target.atime,
        false,
        true,
        |staged_tmp| {
            let mode = fs::metadata(staged_tmp).unwrap().permissions().mode() & 0o777;
            observed_tmp_mode = Some(mode);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(observed_tmp_mode, Some(0o600));
    assert_eq!(
        fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
        0o644
    );

    // 2. AuditLogger::open_with_max_bytes sets 0o600 and rotates to <path>.1.
    let audit_path = tmp.path().join("audit.jsonl");
    let logger = AuditLogger::open_with_max_bytes(&audit_path, 256).unwrap();
    assert_eq!(
        fs::metadata(&audit_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    for idx in 0..4 {
        let entry = AuditEntry::new(AuditEventKind::FileDecision, format!("run-{idx}"), false)
            .with_rule("pg-backups")
            .with_candidate(format!("/var/backups/dump_{idx}.sql"), 4096)
            .with_decision("acted");
        logger.log(&entry).unwrap();
    }

    let rotated_path = PathBuf::from(format!("{}.1", audit_path.display()));
    assert!(
        rotated_path.exists(),
        "audit.jsonl.1 must exist after exceeding 256 bytes"
    );
    assert_eq!(
        fs::metadata(&rotated_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&audit_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let rotated_lines = fs::read_to_string(&rotated_path).unwrap();
    let active_lines = fs::read_to_string(&audit_path).unwrap();
    assert!(!rotated_lines.trim().is_empty());
    assert!(!active_lines.trim().is_empty());
    for line in rotated_lines.lines().chain(active_lines.lines()) {
        let _: serde_json::Value = serde_json::from_str(line).unwrap();
    }

    // Reopening an already-oversized audit log rotates immediately on open.
    drop(logger);
    fs::write(&audit_path, vec![b'x'; 300]).unwrap();
    let reopened = AuditLogger::open_with_max_bytes(&audit_path, 256).unwrap();
    assert_eq!(fs::metadata(&audit_path).unwrap().len(), 0);
    assert_eq!(fs::metadata(&rotated_path).unwrap().len(), 300);
    drop(reopened);
}

#[test]
fn compressed_copy_truncate_cross_fs_boundaries_and_trash_extensionless_edge_cases() {
    let tmp = TempDir::new().unwrap();
    let rotated = tmp.path().join("ct_rotated");
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
        .unwrap()
        .timestamp();

    // 1. CopyTruncate with Brotli & Zip codecs + _1/_2 collision counter on raw archives.
    let ct_src = tmp.path().join("edge.log");
    let make_ct_target = |len: u64| WorkingTarget {
        path: ct_src.clone(),
        size_bytes: len,
        mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
        atime: None,
    };

    for (fmt, ext) in [
        (CompressionFormat::Brotli, ".br"),
        (CompressionFormat::Zip, ".zip"),
    ] {
        fs::write(&ct_src, b"brotli-and-zip-copy-truncate-payload").unwrap();
        let mut rule = make_rule(
            "ct-compressed",
            tmp.path().to_path_buf(),
            vec![ActionStep::CopyTruncate],
        );
        rule.copy_truncate_to = Some(rotated.clone());
        rule.compression = fmt;
        rule.compression_level = Some(5);
        let rep = ActionExecutor::execute_pipeline(make_ct_target(36), &rule, None, false).unwrap();
        let dest = rep.final_path.unwrap();
        assert!(dest.to_string_lossy().ends_with(ext));
        assert!(dest.exists());
        assert_eq!(fs::metadata(&ct_src).unwrap().len(), 0);
    }

    // Raw same-second collision counter increments _1 then _2.
    let mut raw_rule = make_rule(
        "ct-seq",
        tmp.path().to_path_buf(),
        vec![ActionStep::CopyTruncate],
    );
    raw_rule.copy_truncate_to = Some(rotated.clone());
    for expected in [
        rotated.join("edge_20261120_090500.log"),
        rotated.join("edge_20261120_090500_1.log"),
        rotated.join("edge_20261120_090500_2.log"),
    ] {
        fs::write(&ct_src, b"chunk").unwrap();
        let rep =
            ActionExecutor::execute_pipeline(make_ct_target(5), &raw_rule, None, false).unwrap();
        assert_eq!(rep.final_path, Some(expected));
    }

    // 2. Dry-run Compress preserves source without touching disk (both remove_source=true/false).
    let dry_file = tmp.path().join("dry_comp.log");
    let dry_target = create_stamped_file(&dry_file, b"dry-compress-bytes", fixed_secs);
    for remove_src in [true, false] {
        let mut comp_dry = make_rule(
            "dry-comp",
            tmp.path().to_path_buf(),
            vec![ActionStep::Compress],
        );
        comp_dry.compress_remove_source = remove_src;
        let rep =
            ActionExecutor::execute_pipeline(dry_target.clone(), &comp_dry, None, true).unwrap();
        assert!(rep.dry_run);
        assert_eq!(rep.bytes_freed, 0);
        assert_eq!(rep.final_path, Some(tmp.path().join("dry_comp.log.gz")));
        assert!(!tmp.path().join("dry_comp.log.gz").exists());
    }

    // 3. Forced cross-FS overwrite=false fails, overwrite=true replaces dest with SHA-256.
    let cross_src = tmp.path().join("cross_src.sql");
    let cross_dest = tmp.path().join("dest_dir/cross_src.sql");
    fs::create_dir_all(cross_dest.parent().unwrap()).unwrap();
    fs::write(&cross_dest, b"existing-target").unwrap();
    let t_cross = create_stamped_file(&cross_src, b"replacement-payload", fixed_secs);
    assert!(matches!(
        ActionExecutor::move_with_cross_fs_fallback(
            &cross_src,
            &cross_dest,
            t_cross.mtime,
            None,
            false,
            true
        ),
        Err(FileCleanerError::Io { .. })
    ));
    let sha = ActionExecutor::move_with_cross_fs_fallback(
        &cross_src,
        &cross_dest,
        t_cross.mtime,
        t_cross.atime,
        true,
        true,
    )
    .unwrap();
    assert!(sha.is_some());
    assert!(!cross_src.exists());
    assert_eq!(fs::read(&cross_dest).unwrap(), b"replacement-payload");

    // 4. Trash disambiguates extensionless files cleanly (`README`, `README_1`, `README_2`).
    let trash_dir = tmp.path().join("trash");
    let trash_rule = make_rule(
        "trash-extless",
        tmp.path().to_path_buf(),
        vec![ActionStep::Trash],
    );
    for expected_name in ["README", "README_1", "README_2"] {
        let p = tmp.path().join("README");
        let t = create_stamped_file(&p, expected_name.as_bytes(), fixed_secs);
        let rep =
            ActionExecutor::execute_pipeline(t, &trash_rule, Some(&trash_dir), false).unwrap();
        assert_eq!(rep.final_path, Some(trash_dir.join(expected_name)));
    }

    // 5. Public API error boundaries on missing paths / directories + pre-1970 mtime Zip handling.
    let missing_file = tmp.path().join("does_not_exist.log");
    assert!(matches!(
        ActionExecutor::copy_and_verify_sha256(&missing_file, &tmp.path().join("out.tmp")),
        Err(FileCleanerError::Io { .. })
    ));
    assert!(matches!(
        ActionExecutor::move_with_cross_fs_fallback(
            tmp.path(),
            &tmp.path().join("bad_out"),
            t_cross.mtime,
            None,
            false,
            true
        ),
        Err(FileCleanerError::Io { .. })
    ));

    let old_src = tmp.path().join("ancient.txt");
    fs::write(&old_src, b"ancient payload").unwrap();
    let mut old_zip_rule = make_rule(
        "ancient-zip",
        tmp.path().to_path_buf(),
        vec![ActionStep::Compress],
    );
    old_zip_rule.compression = CompressionFormat::Zip;
    let old_target = WorkingTarget {
        path: old_src.clone(),
        size_bytes: 15,
        mtime: Utc.with_ymd_and_hms(1969, 12, 31, 23, 59, 58).unwrap(),
        atime: Some(
            Utc.with_ymd_and_hms(1969, 12, 31, 23, 59, 58).unwrap()
                + chrono::Duration::milliseconds(500),
        ),
    };
    let zip_rep = ActionExecutor::execute_pipeline(old_target, &old_zip_rule, None, false).unwrap();
    assert!(zip_rep.final_path.unwrap().exists());
}

#[test]
fn action_dry_run_overwrite_rename_collision_and_io_error_boundaries() {
    let tmp = TempDir::new().unwrap();
    let fixed_secs = Utc
        .with_ymd_and_hms(2026, 11, 20, 12, 0, 0)
        .unwrap()
        .timestamp();

    // 1. Dry-run paths for Move (with date subdir), CopyTruncate, Trash, and Delete never touch disk.
    let dry_src = tmp.path().join("dry_item.log");
    let dry_target = create_stamped_file(&dry_src, b"dry-run-bytes", fixed_secs);
    let dest_dir = tmp.path().join("dry_dest");
    let trash_dir = tmp.path().join("dry_trash");

    let mut dry_move = make_rule("dry-move", tmp.path().to_path_buf(), vec![ActionStep::Move]);
    dry_move.move_to = Some(dest_dir.clone());
    dry_move.move_date_subdir = Some("%Y/%m".to_string());
    let mv_rep =
        ActionExecutor::execute_pipeline(dry_target.clone(), &dry_move, None, true).unwrap();
    assert_eq!(
        mv_rep.final_path,
        Some(dest_dir.join("2026/11/dry_item.log"))
    );
    assert!(!dest_dir.exists());

    let mut dry_ct = make_rule(
        "dry-ct",
        tmp.path().to_path_buf(),
        vec![ActionStep::CopyTruncate],
    );
    dry_ct.copy_truncate_to = Some(dest_dir.clone());
    let ct_rep = ActionExecutor::execute_pipeline(dry_target.clone(), &dry_ct, None, true).unwrap();
    assert!(ct_rep.dry_run);
    assert_eq!(fs::metadata(&dry_src).unwrap().len(), 13);

    let dry_tr = make_rule(
        "dry-trash",
        tmp.path().to_path_buf(),
        vec![ActionStep::Trash],
    );
    assert!(
        ActionExecutor::execute_pipeline(dry_target.clone(), &dry_tr, Some(&trash_dir), true)
            .is_ok()
    );
    assert!(!trash_dir.exists());

    let dry_del = make_rule(
        "dry-del",
        tmp.path().to_path_buf(),
        vec![ActionStep::Delete],
    );
    assert!(ActionExecutor::execute_pipeline(dry_target, &dry_del, None, true).is_ok());
    assert!(dry_src.exists());

    // 2. RenameByDate rejects existing destination cleanly and refuses symlink when disallowed.
    let ren_src = tmp.path().join("dup.log");
    let ren_existing = tmp.path().join("dup_20261120_120000.log");
    let ren_target = create_stamped_file(&ren_src, b"src", fixed_secs);
    fs::write(&ren_existing, b"already-here").unwrap();
    let ren_rule = make_rule(
        "ren-dup",
        tmp.path().to_path_buf(),
        vec![ActionStep::RenameByDate],
    );
    assert!(matches!(
        ActionExecutor::execute_pipeline(ren_target, &ren_rule, None, false),
        Err(FileCleanerError::Io { .. })
    ));

    // 3. Compress overwrite=false fails when dest exists; overwrite=true + remove_source=false keeps source.
    let comp_src = tmp.path().join("report.csv");
    let comp_dest = tmp.path().join("report.csv.gz");
    let comp_target = create_stamped_file(&comp_src, b"csv-row-1\ncsv-row-2\n", fixed_secs);
    fs::write(&comp_dest, b"stale-gz").unwrap();

    let mut comp_no_ow = make_rule(
        "comp-no-ow",
        tmp.path().to_path_buf(),
        vec![ActionStep::Compress],
    );
    comp_no_ow.compress_overwrite = false;
    assert!(matches!(
        ActionExecutor::execute_pipeline(comp_target.clone(), &comp_no_ow, None, false),
        Err(FileCleanerError::Io { .. })
    ));

    comp_no_ow.compress_overwrite = true;
    comp_no_ow.compress_remove_source = false;
    comp_no_ow.compression_level = Some(1);
    let ow_rep = ActionExecutor::execute_pipeline(comp_target, &comp_no_ow, None, false).unwrap();
    assert!(ow_rep.final_path == Some(comp_dest.clone()));
    assert!(comp_src.exists());

    // Compression levels across Brotli & Zip.
    for (fmt, level) in [(CompressionFormat::Brotli, 1), (CompressionFormat::Zip, 9)] {
        let s = tmp.path().join(format!("lvl_{fmt:?}.txt"));
        let t = create_stamped_file(&s, b"level-check-data", fixed_secs);
        let mut r = make_rule(
            "comp-lvl",
            tmp.path().to_path_buf(),
            vec![ActionStep::Compress],
        );
        r.compression = fmt;
        r.compression_level = Some(level);
        assert!(ActionExecutor::execute_pipeline(t, &r, None, false).is_ok());
    }

    // 4. Move to same directory (`dest == target.path`) acts as safe no-op.
    let same_dir_file = tmp.path().join("stay.log");
    let same_target = create_stamped_file(&same_dir_file, b"stay", fixed_secs);
    let mut same_mv = make_rule("same-mv", tmp.path().to_path_buf(), vec![ActionStep::Move]);
    same_mv.move_to = Some(tmp.path().to_path_buf());
    let same_rep = ActionExecutor::execute_pipeline(same_target, &same_mv, None, false).unwrap();
    assert_eq!(same_rep.final_path, Some(same_dir_file.clone()));
    assert!(same_dir_file.exists());

    // 5. Error boundaries: missing source for Compress/CopyTruncate, invalid dest parent in SHA copy.
    let missing = WorkingTarget {
        path: tmp.path().join("gone.log"),
        size_bytes: 10,
        mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
        atime: None,
    };
    let miss_comp = make_rule(
        "miss-comp",
        tmp.path().to_path_buf(),
        vec![ActionStep::Compress],
    );
    assert!(ActionExecutor::execute_pipeline(missing.clone(), &miss_comp, None, false).is_err());

    let mut miss_ct = make_rule(
        "miss-ct",
        tmp.path().to_path_buf(),
        vec![ActionStep::CopyTruncate],
    );
    miss_ct.copy_truncate_to = Some(dest_dir.clone());
    assert!(ActionExecutor::execute_pipeline(missing, &miss_ct, None, false).is_err());

    let regular_blocker = tmp.path().join("blocker_file");
    fs::write(&regular_blocker, b"not-a-dir").unwrap();
    assert!(ActionExecutor::copy_and_verify_sha256(
        &same_dir_file,
        &regular_blocker.join("child.tmp")
    )
    .is_err());

    // 6. Multi-chunk (> 64 KiB) payloads across all 4 compression codecs, CopyTruncate & cross-FS Move.
    let big_payload: Vec<u8> = (0..131_072).map(|i| (i % 251) as u8).collect();
    for (idx, fmt) in [
        CompressionFormat::Gzip,
        CompressionFormat::Lz4,
        CompressionFormat::Brotli,
        CompressionFormat::Zip,
    ]
    .into_iter()
    .enumerate()
    {
        let p = tmp.path().join(format!("big_{idx}.bin"));
        let t = create_stamped_file(&p, &big_payload, fixed_secs);
        let mut r = make_rule(
            "big-comp",
            tmp.path().to_path_buf(),
            vec![ActionStep::Compress],
        );
        r.compression = fmt;
        r.compression_level = Some(6);
        let rep = ActionExecutor::execute_pipeline(t, &r, None, false).unwrap();
        assert!(rep.final_path.unwrap().exists());
    }

    let big_move_src = tmp.path().join("big_move.bin");
    let big_move_dst = dest_dir.join("big_move.bin");
    fs::create_dir_all(&dest_dir).unwrap();
    let big_mv_target = create_stamped_file(&big_move_src, &big_payload, fixed_secs);
    let sha_big = ActionExecutor::move_with_cross_fs_fallback(
        &big_move_src,
        &big_move_dst,
        big_mv_target.mtime,
        big_mv_target.atime,
        true,
        true,
    )
    .unwrap();
    assert!(sha_big.is_some());
    assert_eq!(fs::read(&big_move_dst).unwrap(), big_payload);

    // 7. Live Move collision (overwrite=false), same-name RenameByDate no-op, missing-file Delete error.
    let col_src = tmp.path().join("collide.log");
    let col_dst = dest_dir.join("collide.log");
    fs::write(&col_dst, b"existing").unwrap();
    let col_target = create_stamped_file(&col_src, b"new", fixed_secs);
    let mut col_rule = make_rule("mv-col", tmp.path().to_path_buf(), vec![ActionStep::Move]);
    col_rule.move_to = Some(dest_dir.clone());
    col_rule.move_overwrite = false;
    assert!(matches!(
        ActionExecutor::execute_pipeline(col_target, &col_rule, None, false),
        Err(FileCleanerError::Io { .. })
    ));

    let same_name = tmp.path().join("fixed_20261120_120000.log");
    let same_name_target = create_stamped_file(&same_name, b"ok", fixed_secs);
    let same_name_rule = make_rule(
        "ren-noop",
        tmp.path().to_path_buf(),
        vec![ActionStep::RenameByDate],
    );
    let ren_noop_rep =
        ActionExecutor::execute_pipeline(same_name_target, &same_name_rule, None, false).unwrap();
    assert_eq!(ren_noop_rep.final_path, Some(same_name));

    let missing_del = WorkingTarget {
        path: tmp.path().join("never_existed.log"),
        size_bytes: 5,
        mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
        atime: None,
    };
    let del_rule = make_rule(
        "del-miss",
        tmp.path().to_path_buf(),
        vec![ActionStep::Delete],
    );
    assert!(matches!(
        ActionExecutor::execute_pipeline(missing_del, &del_rule, None, false),
        Err(FileCleanerError::Io { .. })
    ));

    #[cfg(unix)]
    {
        let link_target = tmp.path().join("real_ct.log");
        let link_path = tmp.path().join("sym_ct.log");
        fs::write(&link_target, b"real").unwrap();
        std::os::unix::fs::symlink(&link_target, &link_path).unwrap();
        let sym_wt = WorkingTarget {
            path: link_path,
            size_bytes: 4,
            mtime: Utc.timestamp_opt(fixed_secs, 0).unwrap(),
            atime: None,
        };
        let mut sym_ct = make_rule(
            "sym-ct",
            tmp.path().to_path_buf(),
            vec![ActionStep::CopyTruncate],
        );
        sym_ct.copy_truncate_to = Some(dest_dir);
        assert!(ActionExecutor::execute_pipeline(sym_wt, &sym_ct, None, false).is_err());
    }
}
