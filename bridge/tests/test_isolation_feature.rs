#[test]
fn bridge_integration_test_refuses_account_home_lock_without_runtime_arming() {
    let home = codescribe_core::test_isolation::account_home().expect("passwd account home");
    let path = home.join(format!(
        ".codescribe/test-isolation-bridge-{}.lock",
        std::process::id()
    ));
    assert!(!path.exists(), "probe path must be absent");
    let result = std::panic::catch_unwind(|| {
        let _ = codescribe_core::config::acquire_app_runtime_install_lease_at(&path);
    });
    let panic = result.expect_err("test feature must refuse before lease creation");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("guard panic text");
    assert!(message.contains("test process refused write under real home"));
    assert!(!path.exists(), "guard must refuse before opening the lock");
}
