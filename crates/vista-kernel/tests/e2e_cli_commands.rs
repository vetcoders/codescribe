//! E2E tests for CLI commands
//!
//! Tests the simplified CLI interface:
//! - `codescribe transcribe <file>` - transcription
//! - `codescribe --config` - config management
//! - `codescribe` (no args) - daemon (tray/hotkeys)
//!
//! Run with:
//!   cargo test --test e2e_cli_commands
//!
//! For transcription tests (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_cli_commands
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;
use std::process::Command;

use serial_test::serial;
use tempfile::TempDir;

/// Path to CLI binary (prefers release for embedded model)
fn cli_binary() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = base.join("target/release/codescribe");
    let debug = base.join("target/debug/codescribe");

    // Prefer release (has embedded model)
    if release.exists() { release } else { debug }
}

/// Build a CLI command with test-safe env defaults.
fn cli_command() -> Command {
    let mut cmd = Command::new(cli_binary());
    // Never touch the real Keychain in tests.
    cmd.env("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    cmd
}

/// Path to test audio file
fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

/// Build CLI if not exists
fn ensure_cli_built() {
    let binary = cli_binary();
    if !binary.exists() {
        let status = Command::new("cargo")
            .args(["build", "-p", "codescribe"])
            .status()
            .expect("Failed to build CLI");
        assert!(status.success(), "CLI build failed");
    }
}

/// Guard: live CLI must use EventSink contract (no legacy delta callback path).
#[test]
fn test_cli_live_uses_event_sink_contract() {
    let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/codescribe.rs");
    let source = std::fs::read_to_string(&source_path).expect("Failed to read CLI source");

    assert!(
        !source.contains("set_delta_callback("),
        "CLI live path should not depend on set_delta_callback"
    );
    assert!(
        source.contains("set_event_sink(Some"),
        "CLI live path should install an event sink"
    );
    assert!(
        source.contains("start_event_session("),
        "CLI live path should start via start_event_session"
    );
}

// ═══════════════════════════════════════════════════════════
// CLI Help & Version Tests
// ═══════════════════════════════════════════════════════════

/// Test: `codescribe --help` shows usage
#[test]
fn test_cli_help() {
    ensure_cli_built();

    let output = cli_command()
        .arg("--help")
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "CLI --help should succeed");
    assert!(
        stdout.contains("transcribe"),
        "Should mention transcribe command"
    );
    assert!(stdout.contains("--config"), "Should mention --config flag");
}

/// Test: `codescribe --version` shows version
#[test]
fn test_cli_version() {
    ensure_cli_built();

    let output = cli_command()
        .arg("--version")
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "CLI --version should succeed");
    assert!(stdout.contains("codescribe"), "Should contain app name");
}

/// Test: `codescribe` (no args) starts daemon (opt-in)
#[test]
fn test_cli_no_args() {
    ensure_cli_built();

    let enabled = std::env::var("CODESCRIBE_E2E_DAEMON")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping daemon E2E (set CODESCRIBE_E2E_DAEMON=1 to enable)");
        return;
    }

    let mut child = cli_command()
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to start daemon");

    std::thread::sleep(std::time::Duration::from_millis(300));

    if let Some(status) = child.try_wait().expect("Failed to poll daemon") {
        panic!("Daemon exited early with status: {}", status);
    }

    let _ = child.kill();
    let _ = child.wait();
}

/// Test: `codescribe transcribe --help` shows transcribe options
#[test]
fn test_cli_transcribe_help() {
    ensure_cli_built();

    let output = cli_command()
        .args(["transcribe", "--help"])
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "transcribe --help should succeed");
    assert!(stdout.contains("--language"), "Should have language option");
    assert!(stdout.contains("--stream"), "Should have stream option");
    assert!(stdout.contains("--format"), "Should have format option");
    assert!(stdout.contains("--llm"), "Should have llm option");
    assert!(stdout.contains("live"), "Should mention live subcommand");
}

// ═══════════════════════════════════════════════════════════
// CLI Config Tests
// ═══════════════════════════════════════════════════════════

/// Test: `codescribe --config` creates default config if missing
#[test]
#[serial]
fn test_cli_config_creates_default() {
    ensure_cli_built();

    let tmp = TempDir::new().expect("tempdir");
    let config_dir = tmp.path().join(".codescribe");
    let config_path = config_dir.join(".env");

    // Run with custom HOME to isolate
    let output = cli_command()
        .arg("--config")
        .env("HOME", tmp.path())
        // Prevent editor from opening (no TTY)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should create config
    assert!(
        config_path.exists() || stdout.contains("Created") || stdout.contains("Config"),
        "Should create or mention config file"
    );
}

// ═══════════════════════════════════════════════════════════
// CLI Transcription Tests (require model)
// ═══════════════════════════════════════════════════════════

/// Test: `codescribe transcribe <file>` with non-existent file
#[test]
fn test_cli_transcribe_file_not_found() {
    ensure_cli_built();

    let output = cli_command()
        .args(["transcribe", "/nonexistent/audio.wav"])
        .output()
        .expect("Failed to run CLI");

    assert!(
        !output.status.success(),
        "Should fail for non-existent file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("No such file"),
        "Should report file not found: {}",
        stderr
    );
}

/// Test: `codescribe transcribe <file>` with real audio (requires model)
#[test]
#[serial]
fn test_cli_transcribe_real_audio() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping transcription E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    ensure_cli_built();

    let audio_path = test_audio_path();
    if !audio_path.exists() {
        eprintln!("Test audio not found: {}", audio_path.display());
        return;
    }

    let output = cli_command()
        .args(["transcribe", audio_path.to_str().unwrap(), "-l", "pl"])
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("STDOUT: {}", stdout);
    println!("STDERR: {}", stderr);

    assert!(
        output.status.success(),
        "Transcription should succeed: {}",
        stderr
    );
    assert!(!stdout.is_empty(), "Should output transcription");
}

/// Test: `codescribe transcribe <file> --language en`
#[test]
#[serial]
fn test_cli_transcribe_with_language() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping transcription E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    ensure_cli_built();

    let audio_path = test_audio_path();
    if !audio_path.exists() {
        return;
    }

    let output = cli_command()
        .args([
            "transcribe",
            audio_path.to_str().unwrap(),
            "--language",
            "en",
        ])
        .output()
        .expect("Failed to run CLI");

    // Should work (even if transcription is in different language)
    assert!(output.status.success(), "Should handle --language flag");
}

// ═══════════════════════════════════════════════════════════
// CLI Error Handling Tests
// ═══════════════════════════════════════════════════════════

/// Test: Invalid subcommand shows error
#[test]
fn test_cli_invalid_subcommand() {
    ensure_cli_built();

    let output = cli_command()
        .args(["invalid-command"])
        .output()
        .expect("Failed to run CLI");

    assert!(
        !output.status.success(),
        "Should fail for invalid subcommand"
    );
}

/// Test: transcribe without file argument shows error
#[test]
fn test_cli_transcribe_missing_file() {
    ensure_cli_built();

    let output = cli_command()
        .args(["transcribe"])
        .output()
        .expect("Failed to run CLI");

    assert!(!output.status.success(), "Should fail when file is missing");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing") || stderr.contains("<FILE>") || stderr.contains("live"),
        "Should mention missing file argument"
    );
}

/// Test: `codescribe transcribe --stream <file>` outputs transcription
#[test]
#[serial]
fn test_cli_transcribe_streaming_output() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping streaming E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    ensure_cli_built();

    let audio_path = test_audio_path();
    if !audio_path.exists() {
        eprintln!("Test audio not found: {}", audio_path.display());
        return;
    }

    let output = cli_command()
        .args([
            "transcribe",
            "--stream",
            audio_path.to_str().unwrap(),
            "-l",
            "pl",
        ])
        .output()
        .expect("Failed to run CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("STDOUT: {}", stdout);
    println!("STDERR: {}", stderr);

    assert!(
        output.status.success(),
        "Streaming transcription should succeed: {}",
        stderr
    );
    assert!(!stdout.is_empty(), "Should output streaming transcription");
}
