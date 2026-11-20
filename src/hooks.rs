//! Pre-action / pre-delete synchronous lifecycle hook execution.
//!
//! Hooks are invoked directly via `std::process::Command::new` without shell
//! interpolation (`sh -c` or `cmd /C` are strictly prohibited), preventing
//! command injection from untrusted filenames.

use crate::config::{ActionStep, CompressionFormat, HookConfig, HookFailurePolicy};
use crate::error::{FileCleanerError, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Poll interval used while waiting for synchronous hook completion or timeout.
const HOOK_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Metadata describing an eviction candidate passed to a pre-delete hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCandidate {
    /// Full path of the candidate file.
    pub path: PathBuf,
    /// Candidate basename.
    pub name: String,
    /// Candidate file size in bytes.
    pub size_bytes: u64,
    /// Candidate modification timestamp in UTC.
    pub mtime: DateTime<Utc>,
    /// Name of the rule evaluating this candidate.
    pub rule_name: String,
    /// Human-readable pipeline summary (e.g., `"compress(Gzip)->move"` or `"delete"`).
    pub action_summary: String,
}

/// Outcome returned by [`HookRunner::run`] after evaluating a candidate hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookVerdict {
    /// Hook exited with status `0`; proceed with the action pipeline.
    Proceed,
    /// Hook exited with the configured `skip_exit_code` (default `42`); skip this file cleanly.
    SkippedByCode(i32),
    /// Dry-run mode is active and `hook.run_in_dry_run == false`; hook was not spawned.
    SimulatedDryRun,
    /// Hook failed or timed out under `HookFailurePolicy::WarnAndContinue`; proceed with warning.
    WarnAndContinue {
        /// Diagnostic description of the tolerated hook failure or timeout.
        reason: String,
    },
}

/// Rule-level vs file-level error disposition derived from [`HookFailurePolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFailureDisposition {
    /// Skip only the current candidate file and continue processing the remaining rule items.
    AbortFile,
    /// Immediately halt processing for the remainder of the active rule.
    AbortRule,
}

/// Abstraction for executing pre-action / pre-delete lifecycle hooks.
pub trait HookRunner: Send + Sync {
    /// Evaluates `hook` against `candidate`, honoring `dry_run` semantics and `hook.on_failure`.
    fn run(
        &self,
        candidate: &HookCandidate,
        hook: &HookConfig,
        dry_run: bool,
    ) -> Result<HookVerdict>;
}

/// Production [`HookRunner`] spawning external executables directly via `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessHookRunner;

impl ProcessHookRunner {
    /// Creates a new [`ProcessHookRunner`].
    pub fn new() -> Self {
        Self
    }
}

impl HookRunner for ProcessHookRunner {
    fn run(
        &self,
        candidate: &HookCandidate,
        hook: &HookConfig,
        dry_run: bool,
    ) -> Result<HookVerdict> {
        if dry_run && !hook.run_in_dry_run {
            return Ok(HookVerdict::SimulatedDryRun);
        }

        if hook.command.trim().is_empty() {
            let err = FileCleanerError::HookFailed {
                rule: candidate.rule_name.clone(),
                command: hook.command.clone(),
                exit_code: None,
                stderr: "hook command must not be empty".to_string(),
            };
            return apply_failure_policy(hook.on_failure, err);
        }

        let expanded_args = expand_hook_args(&hook.args, candidate);
        let mtime_iso = format_mtime_iso(candidate.mtime);
        let size_str = candidate.size_bytes.to_string();
        let dry_run_str = if dry_run { "true" } else { "false" };
        let parent_dir = candidate_parent_dir(&candidate.path);

        let mut cmd = Command::new(&hook.command);
        cmd.args(&expanded_args)
            .envs(&hook.env)
            .env("FILECLEANER_PATH", candidate.path.as_os_str())
            .env("FILECLEANER_NAME", &candidate.name)
            .env("FILECLEANER_DIR", parent_dir)
            .env("FILECLEANER_SIZE", &size_str)
            .env("FILECLEANER_MTIME_ISO", &mtime_iso)
            .env("FILECLEANER_RULE", &candidate.rule_name)
            .env("FILECLEANER_ACTION", &candidate.action_summary)
            .env("FILECLEANER_DRY_RUN", dry_run_str)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = &hook.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = match spawn_with_etxtbsy_retry(&mut cmd) {
            Ok(child) => child,
            Err(io_err) => {
                let err = FileCleanerError::HookFailed {
                    rule: candidate.rule_name.clone(),
                    command: hook.command.clone(),
                    exit_code: None,
                    stderr: format!("failed to spawn hook process: {io_err}"),
                };
                return apply_failure_policy(hook.on_failure, err);
            }
        };

        // Drain stdout and stderr concurrently so large child outputs never deadlock pipe buffers.
        let stdout_reader = child.stdout.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });
        let stderr_reader = child.stderr.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });

        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(handle) = stdout_reader {
                        let _ = handle.join();
                    }
                    let stderr_bytes = stderr_reader
                        .and_then(|handle| handle.join().ok())
                        .unwrap_or_default();
                    let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

                    if let Some(code) = status.code() {
                        if code == 0 {
                            return Ok(HookVerdict::Proceed);
                        }
                        if hook.skip_exit_code == Some(code) {
                            return Ok(HookVerdict::SkippedByCode(code));
                        }
                    }

                    let err = FileCleanerError::HookFailed {
                        rule: candidate.rule_name.clone(),
                        command: hook.command.clone(),
                        exit_code: status.code(),
                        stderr: stderr_text,
                    };
                    return apply_failure_policy(hook.on_failure, err);
                }
                Ok(None) => {
                    let elapsed = start.elapsed();
                    if elapsed >= hook.timeout {
                        // Force-kill and immediately wait to guarantee zombie reaping.
                        let _ = child.kill();
                        let _ = child.wait();
                        let err = FileCleanerError::HookTimedOut {
                            rule: candidate.rule_name.clone(),
                            command: hook.command.clone(),
                            timeout_secs: hook.timeout.as_secs(),
                        };
                        return apply_failure_policy(hook.on_failure, err);
                    }
                    let remaining = hook.timeout.saturating_sub(elapsed);
                    thread::sleep(remaining.min(HOOK_POLL_INTERVAL));
                }
                Err(wait_err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let err = FileCleanerError::HookFailed {
                        rule: candidate.rule_name.clone(),
                        command: hook.command.clone(),
                        exit_code: None,
                        stderr: format!("failed polling hook status: {wait_err}"),
                    };
                    return apply_failure_policy(hook.on_failure, err);
                }
            }
        }
    }
}

/// Maps a [`HookFailurePolicy`] to its corresponding error disposition, returning `None`
/// for [`HookFailurePolicy::WarnAndContinue`].
pub fn hook_error_disposition(policy: HookFailurePolicy) -> Option<HookFailureDisposition> {
    match policy {
        HookFailurePolicy::AbortFile => Some(HookFailureDisposition::AbortFile),
        HookFailurePolicy::AbortRule => Some(HookFailureDisposition::AbortRule),
        HookFailurePolicy::WarnAndContinue => None,
    }
}

/// Executes `runner.run(...)` and classifies any returned error as [`HookFailureDisposition::AbortFile`]
/// or [`HookFailureDisposition::AbortRule`] based on `hook.on_failure`.
///
/// If a custom [`HookRunner`] implementation returns `Err(_)` while `hook.on_failure` is
/// [`HookFailurePolicy::WarnAndContinue`], this helper normalizes it to [`HookVerdict::WarnAndContinue`].
pub fn evaluate_hook(
    runner: &dyn HookRunner,
    candidate: &HookCandidate,
    hook: &HookConfig,
    dry_run: bool,
) -> std::result::Result<HookVerdict, (FileCleanerError, HookFailureDisposition)> {
    match runner.run(candidate, hook, dry_run) {
        Ok(verdict) => Ok(verdict),
        Err(err) => match hook_error_disposition(hook.on_failure) {
            Some(disposition) => Err((err, disposition)),
            None => Ok(HookVerdict::WarnAndContinue {
                reason: err.to_string(),
            }),
        },
    }
}

/// Expands hook argument templates (`{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`)
/// using a single-pass scanner so substituted values are never re-scanned as placeholders.
pub fn expand_hook_args(args: &[String], candidate: &HookCandidate) -> Vec<String> {
    let path_str = candidate.path.to_string_lossy();
    let size_str = candidate.size_bytes.to_string();
    let mtime_iso = format_mtime_iso(candidate.mtime);

    args.iter()
        .map(|arg| {
            expand_single_arg(
                arg,
                path_str.as_ref(),
                &candidate.name,
                &size_str,
                &mtime_iso,
                &candidate.rule_name,
            )
        })
        .collect()
}

/// Formats a rule's ordered action pipeline for `FILECLEANER_ACTION`
/// (e.g., `"compress(Gzip)->move"` or `"delete"`).
pub fn format_action_summary(actions: &[ActionStep], compression: CompressionFormat) -> String {
    if actions.is_empty() {
        return "none".to_string();
    }
    actions
        .iter()
        .map(|step| match step {
            ActionStep::RenameByDate => "rename_by_date".to_string(),
            ActionStep::Compress => format!("compress({compression:?})"),
            ActionStep::Move => "move".to_string(),
            ActionStep::CopyTruncate => "copy_truncate".to_string(),
            ActionStep::Trash => "trash".to_string(),
            ActionStep::Delete => "delete".to_string(),
        })
        .collect::<Vec<_>>()
        .join("->")
}

fn spawn_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    const MAX_ATTEMPTS: usize = 6;
    for attempt in 0..MAX_ATTEMPTS {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(err) if is_etxtbsy(&err) && attempt + 1 < MAX_ATTEMPTS => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err),
        }
    }
    cmd.spawn()
}

fn is_etxtbsy(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::ETXTBSY)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

fn apply_failure_policy(policy: HookFailurePolicy, err: FileCleanerError) -> Result<HookVerdict> {
    match policy {
        HookFailurePolicy::WarnAndContinue => Ok(HookVerdict::WarnAndContinue {
            reason: err.to_string(),
        }),
        HookFailurePolicy::AbortFile | HookFailurePolicy::AbortRule => Err(err),
    }
}

fn format_mtime_iso(mtime: DateTime<Utc>) -> String {
    mtime.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn candidate_parent_dir(path: &std::path::Path) -> &OsStr {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => OsStr::new("."),
        Some(parent) => parent.as_os_str(),
        None => OsStr::new(""),
    }
}

fn expand_single_arg(
    template: &str,
    path: &str,
    name: &str,
    size: &str,
    mtime_iso: &str,
    rule: &str,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open_idx) = rest.find('{') {
        out.push_str(&rest[..open_idx]);
        let after_open = &rest[open_idx..];
        if let Some(after) = after_open.strip_prefix("{path}") {
            out.push_str(path);
            rest = after;
        } else if let Some(after) = after_open.strip_prefix("{name}") {
            out.push_str(name);
            rest = after;
        } else if let Some(after) = after_open.strip_prefix("{size}") {
            out.push_str(size);
            rest = after;
        } else if let Some(after) = after_open.strip_prefix("{mtime_iso}") {
            out.push_str(mtime_iso);
            rest = after;
        } else if let Some(after) = after_open.strip_prefix("{rule}") {
            out.push_str(rule);
            rest = after;
        } else {
            out.push('{');
            rest = &after_open[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::BTreeMap;

    struct AlwaysFailMock;

    impl HookRunner for AlwaysFailMock {
        fn run(
            &self,
            candidate: &HookCandidate,
            hook: &HookConfig,
            _dry_run: bool,
        ) -> Result<HookVerdict> {
            Err(FileCleanerError::HookFailed {
                rule: candidate.rule_name.clone(),
                command: hook.command.clone(),
                exit_code: Some(99),
                stderr: "mock failure".to_string(),
            })
        }
    }

    fn sample_candidate() -> HookCandidate {
        HookCandidate {
            path: PathBuf::from("/var/backups/db_{name};$(id).sql"),
            name: "db_{name};$(id).sql".to_string(),
            size_bytes: 4096,
            mtime: Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap(),
            rule_name: "pg-backups".to_string(),
            action_summary: "compress(Gzip)->move".to_string(),
        }
    }

    #[test]
    fn expand_hook_args_is_single_pass_and_preserves_metacharacters() {
        let candidate = sample_candidate();
        let args = vec![
            "--file={path}".to_string(),
            "{name}".to_string(),
            "{size}".to_string(),
            "{mtime_iso}".to_string(),
            "{rule}".to_string(),
            "{unknown}".to_string(),
        ];
        let expanded = expand_hook_args(&args, &candidate);
        assert_eq!(expanded[0], "--file=/var/backups/db_{name};$(id).sql");
        assert_eq!(expanded[1], "db_{name};$(id).sql");
        assert_eq!(expanded[2], "4096");
        assert_eq!(expanded[3], "2026-11-20T09:05:00Z");
        assert_eq!(expanded[4], "pg-backups");
        assert_eq!(expanded[5], "{unknown}");
    }

    #[test]
    fn action_summary_and_dispositions_match_contract() {
        assert_eq!(format_action_summary(&[], CompressionFormat::Gzip), "none");
        assert_eq!(
            format_action_summary(
                &[
                    ActionStep::RenameByDate,
                    ActionStep::Compress,
                    ActionStep::Move,
                ],
                CompressionFormat::Lz4
            ),
            "rename_by_date->compress(Lz4)->move"
        );
        assert_eq!(
            format_action_summary(&[ActionStep::CopyTruncate], CompressionFormat::Brotli),
            "copy_truncate"
        );
        assert_eq!(
            format_action_summary(&[ActionStep::Trash], CompressionFormat::Zip),
            "trash"
        );
        assert_eq!(
            format_action_summary(&[ActionStep::Delete], CompressionFormat::Gzip),
            "delete"
        );

        assert_eq!(
            hook_error_disposition(HookFailurePolicy::AbortFile),
            Some(HookFailureDisposition::AbortFile)
        );
        assert_eq!(
            hook_error_disposition(HookFailurePolicy::AbortRule),
            Some(HookFailureDisposition::AbortRule)
        );
        assert_eq!(
            hook_error_disposition(HookFailurePolicy::WarnAndContinue),
            None
        );
    }

    #[test]
    fn evaluate_hook_normalizes_mock_failure_for_warn_and_continue() {
        let candidate = sample_candidate();
        let mut hook = HookConfig {
            command: "/bin/true".to_string(),
            args: vec![],
            timeout: Duration::from_secs(5),
            on_failure: HookFailurePolicy::WarnAndContinue,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        };

        let verdict = evaluate_hook(&AlwaysFailMock, &candidate, &hook, false).unwrap();
        assert!(matches!(verdict, HookVerdict::WarnAndContinue { .. }));

        hook.on_failure = HookFailurePolicy::AbortRule;
        let err = evaluate_hook(&AlwaysFailMock, &candidate, &hook, false).unwrap_err();
        assert_eq!(err.1, HookFailureDisposition::AbortRule);
    }

    #[test]
    fn empty_command_and_relative_parent_dir_behave_safely() {
        let candidate = HookCandidate {
            path: PathBuf::from("single.log"),
            name: "single.log".to_string(),
            size_bytes: 10,
            mtime: Utc.with_ymd_and_hms(2026, 11, 20, 9, 5, 0).unwrap(),
            rule_name: "logs".to_string(),
            action_summary: "delete".to_string(),
        };
        assert_eq!(candidate_parent_dir(&candidate.path), OsStr::new("."));
        assert_eq!(candidate_parent_dir(Path::new("")), OsStr::new(""));

        let mut hook = HookConfig {
            command: "   ".to_string(),
            args: vec![],
            timeout: Duration::from_secs(1),
            on_failure: HookFailurePolicy::AbortFile,
            skip_exit_code: Some(42),
            run_in_dry_run: false,
            working_dir: None,
            env: BTreeMap::new(),
        };
        let runner = ProcessHookRunner::new();
        assert!(matches!(
            runner.run(&candidate, &hook, false),
            Err(FileCleanerError::HookFailed { .. })
        ));

        hook.on_failure = HookFailurePolicy::WarnAndContinue;
        assert!(matches!(
            runner.run(&candidate, &hook, false).unwrap(),
            HookVerdict::WarnAndContinue { .. }
        ));
    }

    use std::path::Path;
}
