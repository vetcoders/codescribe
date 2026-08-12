//! Runtime proof that a bare Cargo integration-test binary cannot create the
//! production-shaped `~/.codescribe/logs/codescribe.log` sink.

use std::process::Command;

#[test]
fn bare_cargo_integration_test_refuses_production_log() {
    const CHILD: &str = "CODESCRIBE_LOG_ISOLATION_CHILD";

    if std::env::var_os(CHILD).is_some() {
        let home = std::env::var_os("HOME").expect("child HOME");
        let production_log = std::path::PathBuf::from(home).join(".codescribe/logs/codescribe.log");

        codescribe::logging::init_logging();
        tracing::info!(target: "logging_isolation", "integration test probe");

        assert!(
            !production_log.exists(),
            "integration-test logger created production sink: {}",
            production_log.display()
        );
        return;
    }

    let fake_home = tempfile::tempdir().expect("create isolated HOME");
    let status = Command::new(std::env::current_exe().expect("resolve integration-test binary"))
        .args([
            "--exact",
            "bare_cargo_integration_test_refuses_production_log",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("HOME", fake_home.path())
        .env_remove("CODESCRIBE_DATA_DIR")
        .status()
        .expect("launch isolated integration-test child");

    assert!(status.success(), "isolated logging child must pass");
}
