use crate::config::UserSettings;
use crate::qube_daemon::QubeDaemonState;
use chrono::{DateTime, Duration, Utc};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

const DAEMON_STATE_FRESHNESS_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QubeLifecycleState {
    Disabled,
    MissingBinary { attempted: PathBuf },
    Running { pid: Option<u32>, owned: bool },
    Stopped,
    StartFailed { message: String },
}

impl QubeLifecycleState {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

#[derive(Debug, Clone)]
pub struct QubeDashboardSnapshot {
    pub daemon_state: QubeDaemonState,
    pub lifecycle: QubeLifecycleState,
    pub available: bool,
    pub last_check_fresh: bool,
}

impl QubeDashboardSnapshot {
    pub fn availability_label(&self) -> &'static str {
        if self.available {
            "Available"
        } else {
            match self.lifecycle {
                QubeLifecycleState::Disabled => "Disabled",
                QubeLifecycleState::MissingBinary { .. } => "Binary missing",
                QubeLifecycleState::Running { .. } => "Stale",
                QubeLifecycleState::Stopped => "Not running",
                QubeLifecycleState::StartFailed { .. } => "Start failed",
            }
        }
    }
}

#[derive(Debug, Default)]
struct QubeLifecycleRuntime {
    child: Option<Child>,
    last_state: Option<QubeLifecycleState>,
}

static QUBE_LIFECYCLE: OnceLock<Mutex<QubeLifecycleRuntime>> = OnceLock::new();

fn runtime() -> &'static Mutex<QubeLifecycleRuntime> {
    QUBE_LIFECYCLE.get_or_init(|| Mutex::new(QubeLifecycleRuntime::default()))
}

fn autostart_enabled() -> bool {
    UserSettings::load()
        .qube_daemon_autostart
        .unwrap_or_else(|| {
            std::env::var("QUBE_DAEMON_AUTOSTART")
                .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false)
        })
}

pub fn qube_daemon_candidate_for_exe(current_exe: &Path) -> PathBuf {
    current_exe.with_file_name("qube-daemon")
}

pub fn resolve_qube_daemon_executable_from(
    current_exe: &Path,
    path_env: Option<&OsStr>,
) -> Option<PathBuf> {
    let sibling = qube_daemon_candidate_for_exe(current_exe);
    if sibling.exists() {
        return Some(sibling);
    }

    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join("qube-daemon");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

pub fn resolve_qube_daemon_executable() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    resolve_qube_daemon_executable_from(&current_exe, std::env::var_os("PATH").as_deref())
}

fn process_list_contains_qube_daemon(ps_output: &str, executable: &Path) -> bool {
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("qube-daemon");
    let executable_path = executable.to_string_lossy();

    ps_output.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.contains("--daemon") || !trimmed.contains(executable_name)
        {
            return false;
        }

        trimmed.contains(executable_path.as_ref()) || trimmed.contains(executable_name)
    })
}

fn is_qube_daemon_running(executable: &Path) -> bool {
    let output = Command::new("ps")
        .args(["-ax", "-o", "comm=,args="])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            process_list_contains_qube_daemon(&stdout, executable)
        }
        Ok(output) => {
            warn!(
                "Qube lifecycle: failed to inspect process list (ps exit={})",
                output.status
            );
            false
        }
        Err(err) => {
            warn!("Qube lifecycle: failed to run ps: {err}");
            false
        }
    }
}

fn reap_child_state(child: &mut Child) -> Option<ExitStatus> {
    match child.try_wait() {
        Ok(Some(status)) => Some(status),
        Ok(None) => None,
        Err(err) => {
            warn!("Qube lifecycle: failed to query child status: {err}");
            None
        }
    }
}

fn sync_runtime_state(runtime: &mut QubeLifecycleRuntime) {
    let Some(child) = runtime.child.as_mut() else {
        return;
    };

    if let Some(status) = reap_child_state(child) {
        runtime.child = None;
        runtime.last_state = Some(QubeLifecycleState::StartFailed {
            message: format!("qube-daemon exited with status {status}"),
        });
    }
}

pub fn start_if_enabled() -> QubeLifecycleState {
    if autostart_enabled() {
        start_managed()
    } else {
        current_state()
    }
}

pub fn start_managed() -> QubeLifecycleState {
    let mut runtime = runtime().lock().unwrap_or_else(|e| e.into_inner());
    sync_runtime_state(&mut runtime);

    if let Some(child) = runtime.child.as_ref() {
        let state = QubeLifecycleState::Running {
            pid: Some(child.id()),
            owned: true,
        };
        runtime.last_state = Some(state.clone());
        return state;
    }

    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            let state = QubeLifecycleState::StartFailed {
                message: format!("failed to resolve current executable: {err}"),
            };
            runtime.last_state = Some(state.clone());
            return state;
        }
    };
    let attempted = qube_daemon_candidate_for_exe(&current_exe);
    let Some(executable) =
        resolve_qube_daemon_executable_from(&current_exe, std::env::var_os("PATH").as_deref())
    else {
        let state = QubeLifecycleState::MissingBinary { attempted };
        runtime.last_state = Some(state.clone());
        warn!("Qube lifecycle: autostart enabled, but qube-daemon binary was not found");
        return state;
    };

    if is_qube_daemon_running(&executable) {
        let state = QubeLifecycleState::Running {
            pid: None,
            owned: false,
        };
        runtime.last_state = Some(state.clone());
        return state;
    }

    match Command::new(&executable)
        .arg("--daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            runtime.child = Some(child);
            let state = QubeLifecycleState::Running {
                pid: Some(pid),
                owned: true,
            };
            runtime.last_state = Some(state.clone());
            info!(
                "Started qube-daemon via lifecycle manager (pid={}, path={})",
                pid,
                executable.display()
            );
            state
        }
        Err(err) => {
            let state = QubeLifecycleState::StartFailed {
                message: format!("failed to spawn {}: {err}", executable.display()),
            };
            runtime.last_state = Some(state.clone());
            if let QubeLifecycleState::StartFailed { message } = &state {
                warn!("Qube lifecycle: {message}");
            }
            state
        }
    }
}

pub fn stop_managed() -> QubeLifecycleState {
    let mut runtime = runtime().lock().unwrap_or_else(|e| e.into_inner());
    sync_runtime_state(&mut runtime);

    if let Some(mut child) = runtime.child.take() {
        let pid = child.id();
        if let Err(err) = child.kill() {
            let state = QubeLifecycleState::StartFailed {
                message: format!("failed to stop managed qube-daemon (pid={pid}): {err}"),
            };
            runtime.last_state = Some(state.clone());
            return state;
        }
        let _ = child.wait();
        let state = QubeLifecycleState::Stopped;
        runtime.last_state = Some(state.clone());
        info!("Stopped managed qube-daemon (pid={pid})");
        return state;
    }

    let state = if autostart_enabled() {
        QubeLifecycleState::Stopped
    } else {
        QubeLifecycleState::Disabled
    };
    runtime.last_state = Some(state.clone());
    state
}

pub fn current_state() -> QubeLifecycleState {
    let mut runtime = runtime().lock().unwrap_or_else(|e| e.into_inner());
    sync_runtime_state(&mut runtime);

    if let Some(child) = runtime.child.as_ref() {
        let state = QubeLifecycleState::Running {
            pid: Some(child.id()),
            owned: true,
        };
        runtime.last_state = Some(state.clone());
        return state;
    }

    if let Some(executable) = resolve_qube_daemon_executable() {
        if is_qube_daemon_running(&executable) {
            let state = QubeLifecycleState::Running {
                pid: None,
                owned: false,
            };
            runtime.last_state = Some(state.clone());
            return state;
        }
    } else if autostart_enabled() {
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codescribe"));
        let state = QubeLifecycleState::MissingBinary {
            attempted: qube_daemon_candidate_for_exe(&current_exe),
        };
        runtime.last_state = Some(state.clone());
        return state;
    }

    let state = if autostart_enabled() {
        runtime
            .last_state
            .clone()
            .unwrap_or(QubeLifecycleState::Stopped)
    } else {
        QubeLifecycleState::Disabled
    };
    runtime.last_state = Some(state.clone());
    state
}

fn is_last_check_fresh(last_check: &str, now: DateTime<Utc>) -> bool {
    let trimmed = last_check.trim();
    if trimmed.is_empty() {
        return false;
    }

    DateTime::parse_from_rfc3339(trimmed)
        .map(|parsed| {
            parsed.with_timezone(&Utc) >= now - Duration::seconds(DAEMON_STATE_FRESHNESS_SECS)
        })
        .unwrap_or(false)
}

pub fn dashboard_snapshot() -> QubeDashboardSnapshot {
    let daemon_state = crate::qube_daemon::read_daemon_state();
    let lifecycle = current_state();
    let last_check_fresh = is_last_check_fresh(&daemon_state.last_check, Utc::now());
    let available = daemon_state.available && lifecycle.is_running() && last_check_fresh;

    QubeDashboardSnapshot {
        daemon_state,
        lifecycle,
        available,
        last_check_fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qube_daemon_candidate_uses_sibling_binary() {
        let current_exe = PathBuf::from("/Applications/CodeScribe.app/Contents/MacOS/codescribe");
        assert_eq!(
            qube_daemon_candidate_for_exe(&current_exe),
            PathBuf::from("/Applications/CodeScribe.app/Contents/MacOS/qube-daemon")
        );
    }

    #[test]
    fn resolve_qube_daemon_executable_prefers_sibling_binary() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let macos_dir = tmp.path().join("CodeScribe.app/Contents/MacOS");
        std::fs::create_dir_all(&macos_dir).expect("create bundle dir");
        let current_exe = macos_dir.join("codescribe");
        let sibling = macos_dir.join("qube-daemon");
        std::fs::write(&current_exe, "").expect("write fake current exe");
        std::fs::write(&sibling, "").expect("write fake qube daemon");

        let resolved = resolve_qube_daemon_executable_from(&current_exe, None);
        assert_eq!(resolved, Some(sibling));
    }

    #[test]
    fn process_list_contains_running_qube_daemon_with_daemon_flag() {
        let executable = Path::new("/usr/local/bin/qube-daemon");
        let ps_output = "qube-daemon /usr/local/bin/qube-daemon --daemon\n";
        assert!(process_list_contains_qube_daemon(ps_output, executable));
    }

    #[test]
    fn process_list_ignores_non_daemon_qube_invocations() {
        let executable = Path::new("/usr/local/bin/qube-daemon");
        let ps_output = "qube-daemon /usr/local/bin/qube-daemon --date 2026-04-21\n";
        assert!(!process_list_contains_qube_daemon(ps_output, executable));
    }

    #[test]
    fn dashboard_requires_fresh_last_check() {
        let now = DateTime::parse_from_rfc3339("2026-04-21T03:00:00+00:00")
            .expect("parse time")
            .with_timezone(&Utc);
        assert!(is_last_check_fresh("2026-04-21T02:56:00+00:00", now));
        assert!(!is_last_check_fresh("2026-04-21T02:40:00+00:00", now));
        assert!(!is_last_check_fresh("", now));
    }
}
