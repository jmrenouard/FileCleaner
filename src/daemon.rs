//! Foreground daemon scheduler (`Interval`, `Notify`, `Hybrid`), PID/state
//! management, cooperative stop-file & signal shutdown, and OS service templates.

use crate::config::{Config, DaemonConfig, DaemonMode};
use crate::engine::{RunOptions, SweepEngine, SweepReport};
use crate::error::{FileCleanerError, Result};
use crate::hooks::{HookRunner, ProcessHookRunner};
use crate::in_use::{LockProbe, OsLockProbe};
use crate::scanner::{Clock, SystemClock};
use chrono::{DateTime, Utc};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

const DAEMON_STATE_VERSION: &str = "1.0.0";
const POLL_SLICE: Duration = Duration::from_millis(50);
const STOP_POLL_SLICE: Duration = Duration::from_millis(25);

static DAEMON_TMP_SEQ: AtomicU64 = AtomicU64::new(1);
static SIGNAL_HANDLER_INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
static SIGNAL_TARGET_FLAGS: OnceLock<Mutex<Vec<Weak<AtomicBool>>>> = OnceLock::new();

/// Lifecycle status reported inside the live daemon JSON state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    /// Daemon has claimed PID/state files and is initializing watchers.
    Starting,
    /// Daemon is idle waiting for the next interval tick or filesystem event.
    Idle,
    /// Daemon is actively executing a 6-phase sweep.
    Sweeping,
    /// Daemon received shutdown signal and is draining active work.
    Stopping,
    /// Daemon shut down cleanly and released its PID file.
    Stopped,
}

/// Live daemon telemetry persisted atomically to `daemon.state_file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    /// Schema/binary version string (`"1.0.0"`).
    pub version: String,
    /// Process identifier of the daemon instance.
    pub pid: u32,
    /// Current lifecycle status.
    pub status: DaemonStatus,
    /// Effective scheduling mode (`interval`, `notify`, or `hybrid`).
    pub watch_mode: DaemonMode,
    /// UTC timestamp when the daemon process started.
    pub started_at: DateTime<Utc>,
    /// UTC timestamp when the most recent sweep finished.
    pub last_sweep_at: Option<DateTime<Utc>>,
    /// Scheduled UTC timestamp for the next periodic tick (`None` in `notify` or `stopped`).
    pub next_sweep_at: Option<DateTime<Utc>>,
    /// Total number of completed sweep cycles.
    pub sweeps_completed: u64,
    /// Cumulative files evaluated across all completed sweeps.
    pub total_files_evaluated: u64,
    /// Cumulative files acted upon (or simulated in dry-run) across all sweeps.
    pub total_files_processed: u64,
    /// Cumulative bytes reclaimed in live sweeps.
    pub total_bytes_reclaimed: u64,
    /// Rule names currently frozen by `BackupGuard` in the latest sweep.
    pub active_freezes: Vec<String>,
    /// Rule names currently constrained by safety guards in the latest sweep.
    pub constrained_rules: Vec<String>,
    /// Most recent sweep or runtime diagnostic error, if any.
    pub last_error: Option<String>,
}

/// Runtime options controlling [`DaemonRunner`].
#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    /// Override `config.daemon.mode` (`--mode interval|notify|hybrid`).
    pub mode_override: Option<DaemonMode>,
    /// Terminate deterministically after completing `N` sweeps (`--max-sweeps`).
    pub max_sweeps: Option<u64>,
    /// Force global dry-run simulation across all sweeps (`--dry-run`).
    pub dry_run: bool,
    /// Override audit log destination (`--audit-log`).
    pub audit_log_override: Option<PathBuf>,
    /// Record freeze conditions (`--fail-on-freeze`).
    pub fail_on_freeze: bool,
    /// Record constraint conflicts (`--strict-constraints`).
    pub strict_constraints: bool,
    /// Optional external cooperative shutdown flag (useful for deterministic testing).
    pub shutdown_signal: Option<Arc<AtomicBool>>,
    /// Install process-wide `SIGINT`/`SIGTERM` handler via `ctrlc`.
    pub install_signal_handler: bool,
}

/// Result returned by [`daemon_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatusReport {
    /// `true` if a live daemon process was verified.
    pub running: bool,
    /// Parsed PID from `pid_file` or `state_file`, if available.
    pub pid: Option<u32>,
    /// Whether `pid_file` exists on disk.
    pub pid_file_present: bool,
    /// `true` if `pid_file` exists on disk for a dead/non-existent process.
    pub stale_pid: bool,
    /// Parsed live or last-recorded [`DaemonState`] from `state_file`, if present.
    pub state: Option<DaemonState>,
}

/// Result returned by [`daemon_stop`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStopReport {
    /// `true` when the target daemon is confirmed stopped by the end of the call.
    pub stopped: bool,
    /// `true` if a running daemon was detected before stopping.
    pub was_running: bool,
    /// Target PID if known from `pid_file`.
    pub pid: Option<u32>,
    /// `true` if an OS termination signal (`SIGTERM` on Unix) was sent.
    pub signal_sent: bool,
    /// `true` if `stop_file` was written.
    pub stop_file_written: bool,
}

/// Supported OS service definition targets for [`generate_service_template`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceTemplateTarget {
    /// Linux `systemd` unit file.
    Systemd,
    /// macOS `launchd` property list (`.plist`).
    Launchd,
    /// Windows Task Scheduler XML import definition.
    WindowsTask,
}

/// Foreground daemon runner managing scheduling loops, `notify` watchers, PID/state files,
/// and graceful shutdown.
#[derive(Debug, Default, Clone, Copy)]
pub struct DaemonRunner;

struct PidAndStopGuard {
    pid_file: Option<PathBuf>,
    stop_file: Option<PathBuf>,
    armed: bool,
}

impl PidAndStopGuard {
    fn new(pid_file: Option<PathBuf>, stop_file: Option<PathBuf>) -> Self {
        Self {
            pid_file,
            stop_file,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PidAndStopGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(pid_path) = &self.pid_file {
                let _ = fs::remove_file(pid_path);
            }
            if let Some(stop_path) = &self.stop_file {
                let _ = fs::remove_file(stop_path);
            }
        }
    }
}

struct StopPollerHandle {
    finished: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StopPollerHandle {
    fn spawn(stop_file: Option<PathBuf>, shutdown: Arc<AtomicBool>) -> Self {
        let finished = Arc::new(AtomicBool::new(false));
        let handle = stop_file.map(|path| {
            let stop_flag = Arc::clone(&shutdown);
            let done_flag = Arc::clone(&finished);
            thread::spawn(move || {
                while !done_flag.load(Ordering::Acquire) {
                    if path.exists() {
                        stop_flag.store(true, Ordering::Release);
                        break;
                    }
                    thread::sleep(POLL_SLICE);
                }
            })
        });
        Self { finished, handle }
    }

    fn stop_and_join(&mut self) {
        self.finished.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for StopPollerHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    TriggerSweep,
    Shutdown,
}

impl DaemonRunner {
    /// Runs the daemon using production [`SystemClock`], [`OsLockProbe`], and [`ProcessHookRunner`].
    pub fn run(config: &Config, opts: &DaemonOptions) -> Result<DaemonState> {
        Self::run_with(config, opts, &SystemClock, &OsLockProbe, &ProcessHookRunner)
    }

    /// Runs the daemon with injectable [`Clock`], [`LockProbe`], and [`HookRunner`].
    pub fn run_with(
        config: &Config,
        opts: &DaemonOptions,
        clock: &dyn Clock,
        lock_probe: &dyn LockProbe,
        hook_runner: &dyn HookRunner,
    ) -> Result<DaemonState> {
        config.validate()?;

        let watch_mode = effective_watch_mode(&config.daemon, opts.mode_override);
        let pid = std::process::id();
        let pid_file = config.daemon.pid_file.clone();
        let state_file = config.daemon.state_file.clone();
        let stop_file = config.daemon.stop_file.clone();

        if let Some(pf) = &pid_file {
            claim_pid_file(pf, pid)?;
        }

        let mut cleanup_guard = PidAndStopGuard::new(pid_file.clone(), stop_file.clone());

        let shutdown = opts
            .shutdown_signal
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

        if opts.install_signal_handler {
            register_signal_flag(&shutdown)?;
        }

        // Remove any stale stop_file left over before startup? Wait: if stop_file already
        // exists on disk at startup, treat it as immediate shutdown request or fail-safe stop.
        let started_at = Utc::now();
        let mut state = DaemonState {
            version: DAEMON_STATE_VERSION.to_string(),
            pid,
            status: DaemonStatus::Starting,
            watch_mode,
            started_at,
            last_sweep_at: None,
            next_sweep_at: None,
            sweeps_completed: 0,
            total_files_evaluated: 0,
            total_files_processed: 0,
            total_bytes_reclaimed: 0,
            active_freezes: Vec::new(),
            constrained_rules: Vec::new(),
            last_error: None,
        };

        write_state_file_opt(state_file.as_deref(), &state)?;

        let effective_audit_path = opts
            .audit_log_override
            .as_deref()
            .or_else(|| config.effective_audit_log());
        let ignored_paths = build_ignored_paths(effective_audit_path, &config.daemon);

        let (_watcher, event_rx) = if watch_mode != DaemonMode::Interval {
            let (watcher, rx) = start_fs_watcher(config)?;
            (Some(watcher), Some(rx))
        } else {
            (None, None)
        };

        let mut stop_poller = StopPollerHandle::spawn(stop_file.clone(), Arc::clone(&shutdown));

        let run_opts = RunOptions {
            rule_filter: Vec::new(),
            dry_run: opts.dry_run,
            audit_log_override: opts.audit_log_override.clone(),
            fail_on_freeze: opts.fail_on_freeze,
            strict_constraints: opts.strict_constraints,
            shutdown: Some(Arc::clone(&shutdown)),
        };

        let mut first_iteration = true;
        while !is_shutdown_active(&shutdown, stop_file.as_deref()) {
            if let Some(max) = opts.max_sweeps {
                if state.sweeps_completed >= max {
                    break;
                }
            }

            let should_wait_before_sweep = first_iteration && !config.daemon.sweep_on_start;
            first_iteration = false;

            if should_wait_before_sweep {
                state.status = DaemonStatus::Idle;
                state.next_sweep_at = compute_next_sweep_at(watch_mode, config.daemon.interval);
                write_state_file_opt(state_file.as_deref(), &state)?;

                match wait_for_next_trigger(
                    watch_mode,
                    &config.daemon,
                    event_rx.as_ref(),
                    &ignored_paths,
                    &shutdown,
                    stop_file.as_deref(),
                ) {
                    WaitOutcome::TriggerSweep => {}
                    WaitOutcome::Shutdown => break,
                }
            }

            if is_shutdown_active(&shutdown, stop_file.as_deref()) {
                break;
            }

            state.status = DaemonStatus::Sweeping;
            state.next_sweep_at = None;
            write_state_file_opt(state_file.as_deref(), &state)?;

            match SweepEngine::run_config(config, &run_opts, clock, lock_probe, hook_runner) {
                Ok(report) => {
                    apply_sweep_report_to_state(&mut state, &report);
                    if report.interrupted {
                        shutdown.store(true, Ordering::Release);
                    }
                }
                Err(err) => {
                    state.last_sweep_at = Some(Utc::now());
                    state.sweeps_completed = state.sweeps_completed.saturating_add(1);
                    state.last_error = Some(err.to_string());
                }
            }

            // Discard filesystem events caused by FileCleaner's own sweep mutations.
            if let Some(rx) = &event_rx {
                while rx.try_recv().is_ok() {}
            }

            if let Some(max) = opts.max_sweeps {
                if state.sweeps_completed >= max {
                    break;
                }
            }

            if is_shutdown_active(&shutdown, stop_file.as_deref()) {
                break;
            }

            state.status = DaemonStatus::Idle;
            state.next_sweep_at = compute_next_sweep_at(watch_mode, config.daemon.interval);
            write_state_file_opt(state_file.as_deref(), &state)?;

            match wait_for_next_trigger(
                watch_mode,
                &config.daemon,
                event_rx.as_ref(),
                &ignored_paths,
                &shutdown,
                stop_file.as_deref(),
            ) {
                WaitOutcome::TriggerSweep => {}
                WaitOutcome::Shutdown => break,
            }
        }

        stop_poller.stop_and_join();

        state.status = DaemonStatus::Stopping;
        state.next_sweep_at = None;
        write_state_file_opt(state_file.as_deref(), &state)?;

        state.status = DaemonStatus::Stopped;
        write_state_file_opt(state_file.as_deref(), &state)?;

        if let Some(pf) = &pid_file {
            let _ = fs::remove_file(pf);
        }
        if let Some(sf) = &stop_file {
            let _ = fs::remove_file(sf);
        }
        cleanup_guard.disarm();

        Ok(state)
    }
}

/// Resolves the effective daemon scheduling mode from config and optional CLI override.
pub fn effective_watch_mode(daemon: &DaemonConfig, cli_mode: Option<DaemonMode>) -> DaemonMode {
    match cli_mode {
        Some(mode) => mode,
        None if daemon.mode == DaemonMode::Interval && daemon.watch => DaemonMode::Hybrid,
        None => daemon.mode,
    }
}

/// Inspects `pid_file` liveness and parses `state_file` metrics.
pub fn daemon_status(
    pid_file: Option<&Path>,
    state_file: Option<&Path>,
) -> Result<DaemonStatusReport> {
    if pid_file.is_none() && state_file.is_none() {
        return Err(FileCleanerError::Daemon(
            "daemon status requires pid_file or state_file".to_string(),
        ));
    }

    let state = if let Some(sf) = state_file {
        if sf.exists() {
            let raw = fs::read_to_string(sf).map_err(|err| FileCleanerError::io(sf, err))?;
            let parsed: DaemonState = serde_json::from_str(&raw).map_err(|err| {
                FileCleanerError::Daemon(format!(
                    "failed to parse daemon state file '{}': {err}",
                    sf.display()
                ))
            })?;
            Some(parsed)
        } else {
            None
        }
    } else {
        None
    };

    let (pid_file_present, pid, running, stale_pid) = if let Some(pf) = pid_file {
        if pf.exists() {
            let parsed_pid = read_pid_file(pf)?;
            let alive = is_pid_alive(parsed_pid);
            (true, Some(parsed_pid), alive, !alive)
        } else {
            (false, state.as_ref().map(|s| s.pid), false, false)
        }
    } else if let Some(st) = &state {
        let alive = st.status != DaemonStatus::Stopped && is_pid_alive(st.pid);
        (false, Some(st.pid), alive, false)
    } else {
        (false, None, false, false)
    };

    Ok(DaemonStatusReport {
        running,
        pid,
        pid_file_present,
        stale_pid,
        state,
    })
}

/// Requests cooperative shutdown via `stop_file` (plus `SIGTERM` on Unix when alive)
/// and waits up to `timeout` for the daemon process to exit.
pub fn daemon_stop(
    pid_file: Option<&Path>,
    stop_file: Option<&Path>,
    timeout: Duration,
) -> Result<DaemonStopReport> {
    if pid_file.is_none() && stop_file.is_none() {
        return Err(FileCleanerError::Daemon(
            "daemon stop requires pid_file or stop_file".to_string(),
        ));
    }

    let pid = match pid_file {
        Some(pf) if pf.exists() => Some(read_pid_file(pf)?),
        _ => None,
    };
    let pid_alive = pid.is_some_and(is_pid_alive);

    if pid_file.is_some() && !pid_alive {
        // Remove stale PID file if left behind by a crashed process.
        if let Some(pf) = pid_file {
            if pf.exists() {
                let _ = fs::remove_file(pf);
            }
        }
        return Ok(DaemonStopReport {
            stopped: true,
            was_running: false,
            pid,
            signal_sent: false,
            stop_file_written: false,
        });
    }

    let mut stop_file_written = false;
    if let Some(sf) = stop_file {
        write_atomic_bytes(sf, format!("stop {}\n", Utc::now().to_rfc3339()).as_bytes())?;
        stop_file_written = true;
    }

    #[cfg(unix)]
    let signal_sent = if let Some(target_pid) = pid.filter(|&p| is_pid_alive(p)) {
        send_sigterm(target_pid)?
    } else {
        false
    };
    #[cfg(not(unix))]
    let signal_sent = false;

    let deadline = Instant::now() + timeout;
    loop {
        let still_running = if let Some(target_pid) = pid {
            is_pid_alive(target_pid)
        } else if let Some(sf) = stop_file {
            sf.exists()
        } else {
            false
        };

        if !still_running {
            return Ok(DaemonStopReport {
                stopped: true,
                was_running: pid.is_some() || stop_file_written,
                pid,
                signal_sent,
                stop_file_written,
            });
        }

        if Instant::now() >= deadline {
            return Err(FileCleanerError::Daemon(match pid {
                Some(p) => format!(
                    "timed out after {}ms waiting for daemon PID {p} to stop",
                    timeout.as_millis()
                ),
                None => format!(
                    "timed out after {}ms waiting for daemon to process stop_file",
                    timeout.as_millis()
                ),
            }));
        }

        thread::sleep(STOP_POLL_SLICE);
    }
}

/// Generates a ready-to-install OS service template for `systemd`, `launchd`, or `windows-task`.
pub fn generate_service_template(
    target: ServiceTemplateTarget,
    config_path: &Path,
    user: Option<&str>,
) -> String {
    let cfg_str = config_path.display().to_string();
    match target {
        ServiceTemplateTarget::Systemd => {
            let user_lines = match user.map(str::trim).filter(|u| !u.is_empty()) {
                Some(u) => format!("User={u}\nGroup={u}\n"),
                None => String::new(),
            };
            format!(
                "[Unit]\n\
Description=FileCleaner Deterministic Directory Retention Daemon\n\
Documentation=https://github.com/jmrenouard/FileCleaner\n\
After=local-fs.target network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
{user_lines}ExecStart=/usr/local/bin/filecleaner daemon run --config {cfg_str}\n\
ExecStop=/usr/local/bin/filecleaner daemon stop --config {cfg_str} --timeout 15s\n\
Restart=on-failure\n\
RestartSec=5s\n\
KillSignal=SIGTERM\n\
TimeoutStopSec=20s\n\
NoNewPrivileges=true\n\
PrivateTmp=false\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n"
            )
        }
        ServiceTemplateTarget::Launchd => {
            let escaped_cfg = xml_escape(&cfg_str);
            let user_block = match user.map(str::trim).filter(|u| !u.is_empty()) {
                Some(u) => format!(
                    "    <key>UserName</key>\n    <string>{}</string>\n",
                    xml_escape(u)
                ),
                None => String::new(),
            };
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>Label</key>\n\
    <string>com.filecleaner.daemon</string>\n\
{user_block}    <key>ProgramArguments</key>\n\
    <array>\n\
        <string>/usr/local/bin/filecleaner</string>\n\
        <string>daemon</string>\n\
        <string>run</string>\n\
        <string>--config</string>\n\
        <string>{escaped_cfg}</string>\n\
    </array>\n\
    <key>RunAtLoad</key>\n\
    <true/>\n\
    <key>KeepAlive</key>\n\
    <true/>\n\
    <key>StandardOutPath</key>\n\
    <string>/var/log/filecleaner/daemon.stdout.log</string>\n\
    <key>StandardErrorPath</key>\n\
    <string>/var/log/filecleaner/daemon.stderr.log</string>\n\
</dict>\n\
</plist>\n"
            )
        }
        ServiceTemplateTarget::WindowsTask => {
            let escaped_cfg = xml_escape(&cfg_str);
            let principal = match user.map(str::trim).filter(|u| !u.is_empty()) {
                Some(u) => format!(
                    "    <Principal id=\"Author\">\n\
      <UserId>{}</UserId>\n\
      <LogonType>ServiceAccount</LogonType>\n\
      <RunLevel>HighestAvailable</RunLevel>\n\
    </Principal>\n",
                    xml_escape(u)
                ),
                None => "    <Principal id=\"Author\">\n\
      <UserId>NT AUTHORITY\\SYSTEM</UserId>\n\
      <LogonType>ServiceAccount</LogonType>\n\
      <RunLevel>HighestAvailable</RunLevel>\n\
    </Principal>\n"
                    .to_string(),
            };
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
  <RegistrationInfo>\n\
    <Description>FileCleaner Deterministic Directory Retention Daemon</Description>\n\
  </RegistrationInfo>\n\
  <Triggers>\n\
    <BootTrigger>\n\
      <Enabled>true</Enabled>\n\
    </BootTrigger>\n\
  </Triggers>\n\
  <Principals>\n\
{principal}  </Principals>\n\
  <Settings>\n\
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
    <RestartOnFailure>\n\
      <Interval>PT1M</Interval>\n\
      <Count>3</Count>\n\
    </RestartOnFailure>\n\
  </Settings>\n\
  <Actions Context=\"Author\">\n\
    <Exec>\n\
      <Command>C:\\Program Files\\FileCleaner\\filecleaner.exe</Command>\n\
      <Arguments>daemon run --config &quot;{escaped_cfg}&quot;</Arguments>\n\
    </Exec>\n\
  </Actions>\n\
</Task>\n"
            )
        }
    }
}

/// Checks whether an OS process identifier is currently alive (non-zombie).
pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        let Ok(pid_t) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: `kill(pid, 0)` probes process existence without delivering a signal.
        let rc = unsafe { libc::kill(pid_t, 0) };
        if rc == 0 {
            #[cfg(target_os = "linux")]
            if is_linux_zombie(pid) {
                return false;
            }
            #[cfg(all(unix, not(target_os = "linux")))]
            if is_non_linux_unix_zombie(pid) {
                return false;
            }
            return true;
        }
        let err = std::io::Error::last_os_error();
        err.raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(windows)]
    {
        is_windows_pid_alive(pid)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(target_os = "linux")]
fn is_linux_zombie(pid: u32) -> bool {
    let stat_path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(stat_path) else {
        return false;
    };
    let Some(rparen) = stat.rfind(')') else {
        return false;
    };
    stat[rparen + 1..].trim_start().starts_with('Z')
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_non_linux_unix_zombie(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return true;
    }
    let trimmed = String::from_utf8_lossy(&output.stdout);
    let stat = trimmed.trim();
    stat.is_empty() || stat.starts_with('Z')
}

#[cfg(windows)]
fn is_windows_pid_alive(pid: u32) -> bool {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: i32 = 5;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> *mut c_void;
        fn GetExitCodeProcess(h_process: *mut c_void, lp_exit_code: *mut u32) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    // SAFETY: `OpenProcess` is invoked with `PROCESS_QUERY_LIMITED_INFORMATION` and returns either
    // null or an owned process handle closed unconditionally below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED);
    }

    let mut exit_code: u32 = 0;
    // SAFETY: `handle` is a non-null process handle from `OpenProcess` and `exit_code` points to
    // valid stack storage; `CloseHandle` releases `handle` immediately afterward.
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    // SAFETY: `handle` is non-null and not closed elsewhere.
    unsafe {
        let _ = CloseHandle(handle);
    }

    ok != 0 && exit_code == STILL_ACTIVE
}

#[cfg(unix)]
fn send_sigterm(pid: u32) -> Result<bool> {
    let pid_t = libc::pid_t::try_from(pid)
        .map_err(|_| FileCleanerError::Daemon(format!("invalid PID {pid}")))?;
    // SAFETY: `pid_t` is a positive process identifier checked by caller.
    let rc = unsafe { libc::kill(pid_t, libc::SIGTERM) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(FileCleanerError::Daemon(format!(
        "failed to send SIGTERM to PID {pid}: {err}"
    )))
}

fn register_signal_flag(flag: &Arc<AtomicBool>) -> Result<()> {
    let registry = SIGNAL_TARGET_FLAGS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut guard) = registry.lock() {
        guard.retain(|w| w.strong_count() > 0);
        guard.push(Arc::downgrade(flag));
    }

    let init_res = SIGNAL_HANDLER_INIT.get_or_init(|| {
        ctrlc::set_handler(|| {
            if let Some(reg) = SIGNAL_TARGET_FLAGS.get() {
                if let Ok(mut guard) = reg.lock() {
                    guard.retain(|weak| {
                        if let Some(target) = weak.upgrade() {
                            target.store(true, Ordering::Release);
                            true
                        } else {
                            false
                        }
                    });
                }
            }
        })
        .map_err(|err| err.to_string())
    });

    if let Err(err_msg) = init_res {
        return Err(FileCleanerError::Daemon(format!(
            "failed to register signal handler: {err_msg}"
        )));
    }
    Ok(())
}

fn claim_pid_file(pid_file: &Path, self_pid: u32) -> Result<()> {
    if pid_file.exists() {
        if let Ok(existing_pid) = read_pid_file(pid_file) {
            if existing_pid != self_pid && is_pid_alive(existing_pid) {
                return Err(FileCleanerError::Daemon(format!(
                    "daemon already running with PID {existing_pid} ({})",
                    pid_file.display()
                )));
            }
        }
    }
    write_atomic_bytes(pid_file, format!("{self_pid}\n").as_bytes())
}

fn read_pid_file(pid_file: &Path) -> Result<u32> {
    let raw = fs::read_to_string(pid_file).map_err(|err| FileCleanerError::io(pid_file, err))?;
    raw.trim().parse::<u32>().map_err(|err| {
        FileCleanerError::Daemon(format!("invalid PID file '{}': {err}", pid_file.display()))
    })
}

fn write_state_file_opt(state_file: Option<&Path>, state: &DaemonState) -> Result<()> {
    let Some(path) = state_file else {
        return Ok(());
    };
    let mut json = serde_json::to_string_pretty(state)
        .map_err(|err| FileCleanerError::Serialization(err.to_string()))?;
    json.push('\n');
    write_atomic_bytes(path, json.as_bytes())
}

fn write_atomic_bytes(target: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| FileCleanerError::io(parent, err))?;
        }
    }
    let seq = DAEMON_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut tmp_os = target.as_os_str().to_os_string();
    tmp_os.push(format!(".fc-tmp.{pid}.{seq}"));
    let staged_tmp = PathBuf::from(tmp_os);

    let write_res = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_tmp)
            .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
        file.write_all(data)
            .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
        file.flush()
            .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
        file.sync_all()
            .map_err(|err| FileCleanerError::io(&staged_tmp, err))?;
        drop(file);
        rename_atomic_target(&staged_tmp, target)?;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_file(&staged_tmp);
    }
    write_res
}

fn rename_atomic_target(staged_tmp: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // Retry briefly on Windows when concurrent `daemon status` readers transiently hold target.
        for _ in 0..9 {
            match fs::rename(staged_tmp, target) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(FileCleanerError::io(target, err)),
            }
        }
        fs::rename(staged_tmp, target).map_err(|err| FileCleanerError::io(target, err))
    }
    #[cfg(not(windows))]
    {
        fs::rename(staged_tmp, target).map_err(|err| FileCleanerError::io(target, err))
    }
}

fn start_fs_watcher(
    config: &Config,
) -> Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).map_err(|err| {
        FileCleanerError::Daemon(format!("failed to initialize fs watcher: {err}"))
    })?;

    let mut watch_targets: BTreeMap<PathBuf, RecursiveMode> = BTreeMap::new();
    for rule in config.rules.iter().filter(|r| r.enabled) {
        let mode = if rule.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watch_targets
            .entry(rule.path.clone())
            .and_modify(|existing| {
                if mode == RecursiveMode::Recursive {
                    *existing = RecursiveMode::Recursive;
                }
            })
            .or_insert(mode);
    }

    for (path, rec_mode) in watch_targets {
        watcher.watch(&path, rec_mode).map_err(|err| {
            FileCleanerError::Daemon(format!(
                "failed to watch rule directory '{}': {err}",
                path.display()
            ))
        })?;
    }

    Ok((watcher, rx))
}

fn wait_for_next_trigger(
    mode: DaemonMode,
    daemon: &DaemonConfig,
    event_rx: Option<&Receiver<notify::Result<Event>>>,
    ignored_paths: &HashSet<PathBuf>,
    shutdown: &AtomicBool,
    stop_file: Option<&Path>,
) -> WaitOutcome {
    let interval_deadline = match mode {
        DaemonMode::Interval | DaemonMode::Hybrid => Some(Instant::now() + daemon.interval),
        DaemonMode::Notify => None,
    };
    let mut last_event_at: Option<Instant> = None;

    loop {
        if is_shutdown_active(shutdown, stop_file) {
            return WaitOutcome::Shutdown;
        }

        if let Some(deadline) = interval_deadline {
            if Instant::now() >= deadline {
                return WaitOutcome::TriggerSweep;
            }
        }

        if let Some(rx) = event_rx {
            while let Ok(ev_res) = rx.try_recv() {
                if let Ok(event) = ev_res {
                    if is_relevant_fs_event(&event, ignored_paths) {
                        last_event_at = Some(Instant::now());
                    }
                }
            }

            if let Some(ev_time) = last_event_at {
                if ev_time.elapsed() >= daemon.debounce {
                    return WaitOutcome::TriggerSweep;
                }
            }
        }

        let mut step = POLL_SLICE;
        if let Some(deadline) = interval_deadline {
            step = step.min(deadline.saturating_duration_since(Instant::now()));
        }
        if let Some(ev_time) = last_event_at {
            let deb_deadline = ev_time + daemon.debounce;
            step = step.min(deb_deadline.saturating_duration_since(Instant::now()));
        }
        let step = step.max(Duration::from_millis(5));

        if let Some(rx) = event_rx {
            match rx.recv_timeout(step) {
                Ok(Ok(event)) => {
                    if is_relevant_fs_event(&event, ignored_paths) {
                        last_event_at = Some(Instant::now());
                    }
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return WaitOutcome::Shutdown,
            }
        } else {
            thread::sleep(step);
        }
    }
}

fn is_relevant_fs_event(event: &Event, ignored_paths: &HashSet<PathBuf>) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| {
        let is_tmp = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.ends_with(".fc-tmp") || name.contains(".fc-tmp."));
        if is_tmp {
            return false;
        }
        let norm = normalize_lexical(path);
        !ignored_paths.contains(path) && !ignored_paths.contains(&norm)
    })
}

fn build_ignored_paths(audit_log: Option<&Path>, daemon: &DaemonConfig) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    for p in [
        audit_log,
        daemon.pid_file.as_deref(),
        daemon.state_file.as_deref(),
        daemon.stop_file.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        set.insert(p.to_path_buf());
        set.insert(normalize_lexical(p));
    }
    set
}

fn normalize_lexical(path: &Path) -> PathBuf {
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

    let mut existing = lexical.as_path();
    let mut tail: Vec<OsString> = Vec::new();
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

fn apply_sweep_report_to_state(state: &mut DaemonState, report: &SweepReport) {
    state.last_sweep_at = Some(report.finished_at);
    state.sweeps_completed = state.sweeps_completed.saturating_add(1);
    state.total_files_evaluated = state
        .total_files_evaluated
        .saturating_add(report.summary.total_evaluated as u64);
    state.total_files_processed = state
        .total_files_processed
        .saturating_add(report.summary.total_acted as u64);
    state.total_bytes_reclaimed = state
        .total_bytes_reclaimed
        .saturating_add(report.summary.total_bytes_freed);
    state.active_freezes = report.active_freezes.clone();
    state.constrained_rules = report.constrained_rules.clone();

    let errors: Vec<String> = report
        .rule_reports
        .iter()
        .flat_map(|r| r.errors.iter().cloned())
        .collect();
    state.last_error = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };
}

fn compute_next_sweep_at(mode: DaemonMode, interval: Duration) -> Option<DateTime<Utc>> {
    match mode {
        DaemonMode::Interval | DaemonMode::Hybrid => chrono::Duration::from_std(interval)
            .ok()
            .map(|delta| Utc::now() + delta),
        DaemonMode::Notify => None,
    }
}

fn is_shutdown_active(shutdown: &AtomicBool, stop_file: Option<&Path>) -> bool {
    shutdown.load(Ordering::Acquire) || stop_file.is_some_and(Path::exists)
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ActionStep, AuditConfig, BackupGuardConfig, CombineMode, CompressionFormat, EvictionOrder,
        InUseConfig, RuleConfig, TimeBasis,
    };
    use crate::hooks::ProcessHookRunner;
    use crate::in_use::OsLockProbe;
    use crate::scanner::SystemClock;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    fn make_daemon_config(dir: &Path, watch_dir: &Path, mode: DaemonMode) -> Config {
        Config {
            dry_run: false,
            audit_log: None,
            daemon: DaemonConfig {
                mode,
                interval: Duration::from_millis(80),
                watch: mode != DaemonMode::Interval,
                debounce: Duration::from_millis(40),
                sweep_on_start: true,
                pid_file: Some(dir.join("fc.pid")),
                state_file: Some(dir.join("fc.state.json")),
                stop_file: Some(dir.join("fc.stop")),
            },
            audit: AuditConfig {
                log_path: Some(dir.join("audit.jsonl")),
                jsonl: true,
            },
            global_trash_dir: None,
            rules: vec![RuleConfig {
                name: "spool".to_string(),
                path: watch_dir.to_path_buf(),
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
                max_files: Some(1),
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
            }],
        }
    }

    #[test]
    fn daemon_interval_max_sweeps_writes_state_and_cleans_pid_and_stop_files() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("spool");
        fs::create_dir_all(&watch_dir).unwrap();

        let f1 = watch_dir.join("old.log");
        let f2 = watch_dir.join("new.log");
        fs::write(&f1, b"old-data").unwrap();
        fs::write(&f2, b"new-data").unwrap();
        {
            let f1_handle = fs::OpenOptions::new().write(true).open(&f1).unwrap();
            f1_handle
                .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
                .unwrap();
        }

        let cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Interval);
        let opts = DaemonOptions {
            max_sweeps: Some(2),
            ..DaemonOptions::default()
        };

        let final_state =
            DaemonRunner::run_with(&cfg, &opts, &SystemClock, &OsLockProbe, &ProcessHookRunner)
                .unwrap();

        assert_eq!(final_state.version, "1.0.0");
        assert_eq!(final_state.status, DaemonStatus::Stopped);
        assert_eq!(final_state.watch_mode, DaemonMode::Interval);
        assert_eq!(final_state.sweeps_completed, 2);
        assert_eq!(final_state.total_files_processed, 1);
        assert_eq!(final_state.total_bytes_reclaimed, 8);
        assert!(!f1.exists());
        assert!(f2.exists());
        assert!(!cfg.daemon.pid_file.as_ref().unwrap().exists());
        assert!(!cfg.daemon.stop_file.as_ref().unwrap().exists());

        let state_path = cfg.daemon.state_file.as_ref().unwrap();
        let parsed: DaemonState =
            serde_json::from_str(&fs::read_to_string(state_path).unwrap()).unwrap();
        assert_eq!(parsed.status, DaemonStatus::Stopped);
        assert_eq!(parsed.sweeps_completed, 2);
    }

    #[test]
    fn daemon_rejects_live_pid_and_replaces_stale_pid() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("watched");
        fs::create_dir_all(&watch_dir).unwrap();

        let mut cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Interval);
        cfg.daemon.sweep_on_start = false;
        let pid_path = cfg.daemon.pid_file.clone().unwrap();

        // Live external PID (spawn a short-lived child process to test active PID collision).
        #[cfg(unix)]
        let mut sleeper = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut sleeper = std::process::Command::new("ping")
            .args(["-n", "6", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let live_pid = sleeper.id();
        fs::write(&pid_path, format!("{live_pid}\n")).unwrap();

        let err = DaemonRunner::run(
            &cfg,
            &DaemonOptions {
                max_sweeps: Some(0),
                ..DaemonOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, FileCleanerError::Daemon(_)));
        let _ = sleeper.kill();
        let _ = sleeper.wait();

        // Now PID is dead -> stale PID file must be replaced cleanly.
        let final_state = DaemonRunner::run(
            &cfg,
            &DaemonOptions {
                max_sweeps: Some(0),
                ..DaemonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(final_state.status, DaemonStatus::Stopped);
        assert!(!pid_path.exists());
    }

    #[test]
    fn daemon_notify_and_cooperative_stop_file_work_deterministically() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("notify_watch");
        fs::create_dir_all(&watch_dir).unwrap();

        let mut cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Notify);
        cfg.daemon.sweep_on_start = false;
        cfg.daemon.debounce = Duration::from_millis(30);

        let cfg_clone = cfg.clone();
        let thread = thread::spawn(move || {
            DaemonRunner::run(
                &cfg_clone,
                &DaemonOptions {
                    max_sweeps: Some(5),
                    ..DaemonOptions::default()
                },
            )
        });

        let state_path = cfg.daemon.state_file.clone().unwrap();
        let wait_idle_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(raw) = fs::read_to_string(&state_path) {
                if let Ok(st) = serde_json::from_str::<DaemonState>(&raw) {
                    if st.status == DaemonStatus::Idle {
                        break;
                    }
                }
            }
            assert!(
                Instant::now() < wait_idle_deadline,
                "daemon never reached Idle"
            );
            thread::sleep(Duration::from_millis(20));
        }

        // Create two files; Notify + debounce triggers sweep #1 and keeps newest 1.
        let old = watch_dir.join("a_old.log");
        let newer = watch_dir.join("b_new.log");
        fs::write(&old, b"old").unwrap();
        {
            let old_f = fs::OpenOptions::new().write(true).open(&old).unwrap();
            old_f
                .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
                .unwrap();
        }
        fs::write(&newer, b"new").unwrap();

        let wait_sweep_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(raw) = fs::read_to_string(&state_path) {
                if let Ok(st) = serde_json::from_str::<DaemonState>(&raw) {
                    if st.sweeps_completed >= 1 && st.status == DaemonStatus::Idle {
                        break;
                    }
                }
            }
            assert!(
                Instant::now() < wait_sweep_deadline,
                "notify sweep did not complete in time"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!old.exists());
        assert!(newer.exists());

        // Write stop_file and verify fast cooperative shutdown responsiveness.
        let stop_started = Instant::now();
        fs::write(cfg.daemon.stop_file.as_ref().unwrap(), b"stop\n").unwrap();
        let final_state = thread.join().unwrap().unwrap();
        assert!(stop_started.elapsed() < Duration::from_secs(2));
        assert_eq!(final_state.status, DaemonStatus::Stopped);
        assert_eq!(final_state.sweeps_completed, 1);
    }

    #[test]
    fn service_templates_cover_all_three_targets_and_escape_xml() {
        let cfg = Path::new("/etc/filecleaner/prod & main.toml");

        let systemd = generate_service_template(
            ServiceTemplateTarget::Systemd,
            Path::new("/etc/filecleaner/prod.toml"),
            Some("backup-svc"),
        );
        assert!(systemd.contains("[Unit]"));
        assert!(systemd.contains("User=backup-svc"));
        assert!(systemd.contains(
            "ExecStart=/usr/local/bin/filecleaner daemon run --config /etc/filecleaner/prod.toml"
        ));

        let launchd = generate_service_template(ServiceTemplateTarget::Launchd, cfg, Some("ops"));
        assert!(launchd.contains("<plist version=\"1.0\">"));
        assert!(launchd.contains("<string>ops</string>"));
        assert!(launchd.contains("prod &amp; main.toml"));

        let win = generate_service_template(ServiceTemplateTarget::WindowsTask, cfg, None);
        assert!(win.contains("<Task version=\"1.4\""));
        assert!(win.contains("NT AUTHORITY\\SYSTEM"));
        assert!(win.contains("prod &amp; main.toml"));
    }

    #[test]
    fn status_and_stop_handle_missing_or_stale_files_cleanly() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            daemon_status(None, None),
            Err(FileCleanerError::Daemon(_))
        ));
        assert!(matches!(
            daemon_stop(None, None, Duration::from_secs(1)),
            Err(FileCleanerError::Daemon(_))
        ));

        let pid_file = tmp.path().join("stale.pid");
        let state_file = tmp.path().join("state.json");
        let stop_file = tmp.path().join("stop");
        fs::write(&pid_file, "99999999\n").unwrap();

        let status = daemon_status(Some(&pid_file), Some(&state_file)).unwrap();
        assert!(!status.running);
        assert!(status.stale_pid);
        assert_eq!(status.pid, Some(99_999_999));

        let stop_rep =
            daemon_stop(Some(&pid_file), Some(&stop_file), Duration::from_secs(1)).unwrap();
        assert!(stop_rep.stopped);
        assert!(!stop_rep.was_running);
        assert!(!pid_file.exists());

        // Corrupt state JSON fails cleanly.
        fs::write(&state_file, "{not-valid-json").unwrap();
        assert!(matches!(
            daemon_status(None, Some(&state_file)),
            Err(FileCleanerError::Daemon(_))
        ));

        // Valid state JSON without pid_file reports non-running when status=Stopped or dead PID.
        let sample_state = DaemonState {
            version: "1.0.0".to_string(),
            pid: 99_999_998,
            status: DaemonStatus::Idle,
            watch_mode: DaemonMode::Hybrid,
            started_at: Utc::now(),
            last_sweep_at: None,
            next_sweep_at: None,
            sweeps_completed: 0,
            total_files_evaluated: 0,
            total_files_processed: 0,
            total_bytes_reclaimed: 0,
            active_freezes: vec![],
            constrained_rules: vec![],
            last_error: None,
        };
        fs::write(&state_file, serde_json::to_string(&sample_state).unwrap()).unwrap();
        let state_only = daemon_status(None, Some(&state_file)).unwrap();
        assert!(!state_only.running);
        assert_eq!(state_only.pid, Some(99_999_998));

        // Missing pid_file and missing state_file reports stopped cleanly.
        let missing_pid = tmp.path().join("none.pid");
        let missing_state = tmp.path().join("none.json");
        let empty_status = daemon_status(Some(&missing_pid), Some(&missing_state)).unwrap();
        assert!(!empty_status.running);
        assert!(!empty_status.pid_file_present);

        let no_pid_only = daemon_status(None, Some(&missing_state)).unwrap();
        assert!(!no_pid_only.running);
        assert!(no_pid_only.pid.is_none());
    }

    #[test]
    fn daemon_hybrid_mode_effective_mode_signal_hook_stop_timeouts_and_helpers() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("hybrid_dir");
        fs::create_dir_all(&watch_dir).unwrap();

        // effective_watch_mode resolves Interval+watch=true -> Hybrid unless overridden.
        let mut d_cfg = DaemonConfig {
            mode: DaemonMode::Interval,
            watch: true,
            ..DaemonConfig::default()
        };
        assert_eq!(effective_watch_mode(&d_cfg, None), DaemonMode::Hybrid);
        assert_eq!(
            effective_watch_mode(&d_cfg, Some(DaemonMode::Interval)),
            DaemonMode::Interval
        );
        d_cfg.watch = false;
        assert_eq!(effective_watch_mode(&d_cfg, None), DaemonMode::Interval);

        // is_pid_alive edge cases.
        assert!(!is_pid_alive(0));
        assert!(!is_pid_alive(u32::MAX));
        assert!(is_pid_alive(std::process::id()));

        // Service templates without user and WindowsTask with explicit user.
        let systemd_no_user = generate_service_template(
            ServiceTemplateTarget::Systemd,
            Path::new("/etc/fc.toml"),
            None,
        );
        assert!(!systemd_no_user.contains("User="));
        let launchd_no_user = generate_service_template(
            ServiceTemplateTarget::Launchd,
            Path::new("/etc/fc.toml"),
            None,
        );
        assert!(!launchd_no_user.contains("<key>UserName</key>"));
        let win_with_user = generate_service_template(
            ServiceTemplateTarget::WindowsTask,
            Path::new("/etc/fc.toml"),
            Some("CORP\\svc_fc"),
        );
        assert!(win_with_user.contains("CORP\\svc_fc"));

        // Hybrid mode with sweep_on_start=false, two rules sharing root (recursive merge), and install_signal_handler=true.
        let mut cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Hybrid);
        cfg.daemon.sweep_on_start = false;
        cfg.daemon.interval = Duration::from_millis(60);
        let mut r2 = cfg.rules[0].clone();
        r2.name = "spool-rec".to_string();
        r2.recursive = true;
        cfg.rules.push(r2);

        let st = DaemonRunner::run(
            &cfg,
            &DaemonOptions {
                max_sweeps: Some(1),
                install_signal_handler: true,
                ..DaemonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(st.watch_mode, DaemonMode::Hybrid);
        assert_eq!(st.sweeps_completed, 1);

        // Unwatched non-existent directory fails cleanly and cleans up pid_file via PidAndStopGuard.
        let mut missing_watch_cfg = make_daemon_config(
            tmp.path(),
            &tmp.path().join("missing_watch"),
            DaemonMode::Notify,
        );
        missing_watch_cfg.daemon.pid_file = Some(tmp.path().join("cleanup_guard.pid"));
        let err = DaemonRunner::run(&missing_watch_cfg, &DaemonOptions::default()).unwrap_err();
        assert!(matches!(err, FileCleanerError::Daemon(_)));
        assert!(!missing_watch_cfg.daemon.pid_file.unwrap().exists());

        // daemon_stop via stop_file-only times out when no daemon consumes stop_file.
        let orphan_stop = tmp.path().join("orphan.stop");
        let stop_timeout = daemon_stop(None, Some(&orphan_stop), Duration::from_millis(40));
        assert!(matches!(stop_timeout, Err(FileCleanerError::Daemon(_))));

        // is_relevant_fs_event filters Access events and .fc-tmp files.
        let mut ignored = HashSet::new();
        ignored.insert(tmp.path().join("audit.jsonl"));
        let access_ev = Event {
            kind: EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Read,
            )),
            paths: vec![watch_dir.join("data.sql")],
            attrs: Default::default(),
        };
        assert!(!is_relevant_fs_event(&access_ev, &ignored));
        let tmp_ev = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![watch_dir.join("data.sql.fc-tmp.1.2")],
            attrs: Default::default(),
        };
        assert!(!is_relevant_fs_event(&tmp_ev, &ignored));
        let empty_ev = Event {
            kind: EventKind::Any,
            paths: vec![],
            attrs: Default::default(),
        };
        assert!(is_relevant_fs_event(&empty_ev, &ignored));

        // SweepEngine error inside DaemonRunner sets last_error without crashing daemon.
        let mut bad_audit_cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Interval);
        let dir_as_audit = tmp.path().join("audit_is_dir");
        fs::create_dir_all(&dir_as_audit).unwrap();
        bad_audit_cfg.audit.log_path = Some(dir_as_audit);
        let err_state = DaemonRunner::run(
            &bad_audit_cfg,
            &DaemonOptions {
                max_sweeps: Some(1),
                ..DaemonOptions::default()
            },
        )
        .unwrap();
        assert!(err_state.last_error.is_some());
    }

    #[test]
    fn daemon_stop_times_out_when_target_ignores_sigterm_and_succeeds_with_stop_file_only() {
        let tmp = TempDir::new().unwrap();
        let watch_dir = tmp.path().join("stop_test");
        fs::create_dir_all(&watch_dir).unwrap();

        // 1. Stop-file-only shutdown while DaemonRunner runs in background thread without pid_file.
        let mut sf_only_cfg = make_daemon_config(tmp.path(), &watch_dir, DaemonMode::Interval);
        sf_only_cfg.daemon.pid_file = None;
        sf_only_cfg.daemon.interval = Duration::from_secs(10);
        let sf_path = sf_only_cfg.daemon.stop_file.clone().unwrap();

        let cfg_clone = sf_only_cfg.clone();
        let handle =
            thread::spawn(move || DaemonRunner::run(&cfg_clone, &DaemonOptions::default()));
        let state_path = sf_only_cfg.daemon.state_file.clone().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(raw) = fs::read_to_string(&state_path) {
                if let Ok(st) = serde_json::from_str::<DaemonState>(&raw) {
                    if st.status == DaemonStatus::Idle {
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        let rep = daemon_stop(None, Some(&sf_path), Duration::from_secs(2)).unwrap();
        assert!(rep.stopped && rep.was_running && rep.stop_file_written && !rep.signal_sent);
        let final_st = handle.join().unwrap().unwrap();
        assert_eq!(final_st.status, DaemonStatus::Stopped);

        // 2. PID-based stop timeout when child traps SIGTERM (synchronize on READY before signaling).
        #[cfg(unix)]
        {
            use std::io::{BufRead, BufReader};
            use std::process::Stdio;
            let mut trap_child = std::process::Command::new("sh")
                .arg("-c")
                .arg("trap '' TERM; printf 'READY\\n'; sleep 5")
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut reader = BufReader::new(trap_child.stdout.take().unwrap());
            let mut ready = String::new();
            reader.read_line(&mut ready).unwrap();
            assert_eq!(ready.trim(), "READY");

            let trap_pid_file = tmp.path().join("trap.pid");
            fs::write(&trap_pid_file, format!("{}\n", trap_child.id())).unwrap();

            let timeout_err =
                daemon_stop(Some(&trap_pid_file), None, Duration::from_millis(60)).unwrap_err();
            assert!(matches!(timeout_err, FileCleanerError::Daemon(_)));
            let _ = trap_child.kill();
            let _ = trap_child.wait();
        }
    }
}
