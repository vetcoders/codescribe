//! Dependency-mode proof: core is compiled without cfg(test), and the tripwire
//! catches credential acquisition before any test-host bypass or OS operation.

use codescribe_core::config::{Config, keychain::CredentialAcquisitionProbe};

#[test]
fn no_keychain_then_authorized_load_acquires_after_env_window_closes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"schema_version":3}"#).unwrap();
    // This integration binary has one test and no application worker threads.
    // Its isolated process environment disappears when the witness exits.
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", root.path());
        std::env::set_var("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        std::env::remove_var("CODESCRIBE_VOICE_LAB_SRC");
        std::env::remove_var("STT_API_KEY");
    }
    {
        let probe = CredentialAcquisitionProbe::forbid();
        let _ = Config::load_without_keychain();
        assert!(probe.attempts().is_empty());
    }
    let before: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
    let probe = CredentialAcquisitionProbe::forbid();
    let attempt = std::panic::catch_unwind(Config::load);
    assert!(
        attempt.is_err(),
        "authorized load must reach acquisition even after no-Keychain bootstrap"
    );
    assert_eq!(probe.attempts(), ["populate"]);
    let after: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
    assert!(
        before == after,
        "closed bootstrap must not mutate process env"
    );
}
