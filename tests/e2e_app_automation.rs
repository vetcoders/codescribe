#![cfg(target_os = "macos")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serial_test::serial;
use tempfile::TempDir;

fn cli_binary() -> PathBuf {
    if let Some(current) = option_env!("CARGO_BIN_EXE_codescribe") {
        return PathBuf::from(current);
    }

    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = base.join("target/release/codescribe");
    let debug = base.join("target/debug/codescribe");

    if release.exists() { release } else { debug }
}

fn ensure_cli_built() {
    let binary = cli_binary();
    if !binary.exists() {
        let status = Command::new("cargo")
            .args(["build", "-p", "codescribe"])
            .status()
            .expect("failed to build CLI");
        assert!(status.success(), "CLI build failed");
    }
}

fn base_command() -> Command {
    let mut cmd = Command::new(cli_binary());
    cmd.env("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    cmd
}

#[derive(Debug, Deserialize)]
struct AppAutomationState {
    creator_visible: bool,
    voice_chat_visible: bool,
    transcription_overlay_visible: bool,
    setup_required: bool,
    dock_icon_visible: bool,
}

fn read_log(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|_| "<missing>".to_string())
}

fn wait_for_socket(socket_path: &Path, child: &mut Child, stdout_log: &Path, stderr_log: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            panic!(
                "automation daemon exited early with status {}\nstdout:\n{}\nstderr:\n{}",
                status,
                read_log(stdout_log),
                read_log(stderr_log)
            );
        }
        if socket_path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }

    panic!(
        "timed out waiting for IPC socket at {}\nstdout:\n{}\nstderr:\n{}",
        socket_path.display(),
        read_log(stdout_log),
        read_log(stderr_log)
    );
}

fn spawn_automation_daemon(data_dir: &Path, stdout_log: &Path, stderr_log: &Path) -> Child {
    base_command()
        .arg("daemon")
        .env("CODESCRIBE_DATA_DIR", data_dir)
        .env("CODESCRIBE_APP_AUTOMATION_MODE", "1")
        .stdout(File::create(stdout_log).expect("create stdout log"))
        .stderr(File::create(stderr_log).expect("create stderr log"))
        .spawn()
        .expect("failed to start automation daemon")
}

fn run_app_command(data_dir: &Path, args: &[&str]) -> Output {
    base_command()
        .args(args)
        .env("CODESCRIBE_DATA_DIR", data_dir)
        .output()
        .expect("failed to run app command")
}

fn parse_state(output: Output) -> AppAutomationState {
    assert!(
        output.status.success(),
        "command failed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<AppAutomationState>(&output.stdout).expect("valid app automation json")
}

#[test]
#[serial]
fn e2e_app_automation_commands_drive_native_surface() {
    let enabled = std::env::var("CODESCRIBE_E2E_APP_AUTOMATION")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping app automation E2E (set CODESCRIBE_E2E_APP_AUTOMATION=1 to enable)");
        return;
    }

    ensure_cli_built();

    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("codescribe-data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");

    let socket_path = data_dir.join("ipc").join("codescribe.sock");
    let stdout_log = tmp.path().join("daemon.stdout.log");
    let stderr_log = tmp.path().join("daemon.stderr.log");
    let mut child = spawn_automation_daemon(&data_dir, &stdout_log, &stderr_log);
    wait_for_socket(&socket_path, &mut child, &stdout_log, &stderr_log);
    thread::sleep(Duration::from_millis(500));

    let initial = parse_state(run_app_command(&data_dir, &["app", "state"]));
    assert!(!initial.creator_visible);
    assert!(!initial.voice_chat_visible);
    assert!(!initial.transcription_overlay_visible);
    assert!(initial.setup_required);
    assert!(initial.dock_icon_visible);

    let creator = parse_state(run_app_command(
        &data_dir,
        &["app", "action", "show-creator"],
    ));
    assert!(creator.creator_visible);

    let hidden_creator = parse_state(run_app_command(
        &data_dir,
        &["app", "action", "hide-creator"],
    ));
    assert!(!hidden_creator.creator_visible);

    let dock_reopen = parse_state(run_app_command(
        &data_dir,
        &["app", "action", "trigger-dock-reopen"],
    ));
    assert!(dock_reopen.creator_visible);

    let reset = parse_state(run_app_command(&data_dir, &["app", "action", "reset-ui"]));
    assert!(!reset.creator_visible);
    assert!(!reset.voice_chat_visible);
    assert!(!reset.transcription_overlay_visible);

    let show_agent = parse_state(run_app_command(
        &data_dir,
        &["app", "action", "trigger-tray-show-agent"],
    ));
    assert!(show_agent.voice_chat_visible);

    let hide_agent = parse_state(run_app_command(
        &data_dir,
        &["app", "action", "hide-voice-chat"],
    ));
    assert!(!hide_agent.voice_chat_visible);

    let _ = child.kill();
    let _ = child.wait();
}
