use serial_test::serial;
use tempfile::TempDir;

use crate::config::Config;
use crate::os::permissions::PermissionStatus;

use super::permission_flow::{
    PermissionUiStatus, should_refresh_hotkey_runtime_after_grant, should_wait_for_restart,
};
use super::session::{load_onboarding_progress, mark_onboarding_done, save_onboarding_progress};
use super::should_show_onboarding;
use super::steps::{PermissionKind, PermissionRecoveryStrategy};

fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
    }
    tmp
}

#[test]
#[serial]
fn fresh_install_requires_onboarding() {
    let _tmp = setup_test_env();
    assert!(should_show_onboarding());
}

#[test]
#[serial]
fn onboarding_completion_writes_canonical_setup_done() {
    let _tmp = setup_test_env();

    save_onboarding_progress(4);
    mark_onboarding_done();

    assert!(Config::config_dir().join("setup_done").exists());
    assert!(!Config::config_dir().join("onboarding_done").exists());
    assert!(!Config::config_dir().join("onboarding_progress").exists());
    assert!(!should_show_onboarding());
}

#[test]
#[serial]
fn onboarding_progress_round_trips_for_resume() {
    let _tmp = setup_test_env();

    save_onboarding_progress(3);

    assert_eq!(load_onboarding_progress(), 3);
}

#[test]
fn runtime_recovery_strategy_maps_permissions_to_runtime_truth() {
    assert_eq!(
        PermissionKind::Microphone.recovery_strategy(),
        PermissionRecoveryStrategy::LiveRecheck
    );
    assert_eq!(
        PermissionKind::Accessibility.recovery_strategy(),
        PermissionRecoveryStrategy::LiveReinitialize
    );
    assert_eq!(
        PermissionKind::InputMonitoring.recovery_strategy(),
        PermissionRecoveryStrategy::LiveReinitialize
    );
    assert_eq!(
        PermissionKind::ScreenRecording.recovery_strategy(),
        PermissionRecoveryStrategy::AppRestartRequired
    );
    assert_eq!(
        PermissionKind::FullDiskAccess.recovery_strategy(),
        PermissionRecoveryStrategy::AppRestartRequired
    );
}

#[test]
fn restart_required_permissions_wait_for_relaunch_only_after_same_process_grant() {
    assert!(should_wait_for_restart(
        PermissionKind::ScreenRecording,
        PermissionUiStatus::Granted,
        true
    ));
    assert!(!should_wait_for_restart(
        PermissionKind::ScreenRecording,
        PermissionUiStatus::Granted,
        false
    ));
    assert!(!should_wait_for_restart(
        PermissionKind::Accessibility,
        PermissionUiStatus::Granted,
        true
    ));
}

#[test]
fn hotkey_runtime_refresh_waits_for_both_permissions() {
    assert!(!should_refresh_hotkey_runtime_after_grant(
        PermissionKind::Accessibility,
        PermissionStatus::Granted,
        PermissionStatus::Denied,
    ));
    assert!(!should_refresh_hotkey_runtime_after_grant(
        PermissionKind::Microphone,
        PermissionStatus::Granted,
        PermissionStatus::Granted,
    ));
    assert!(should_refresh_hotkey_runtime_after_grant(
        PermissionKind::InputMonitoring,
        PermissionStatus::Granted,
        PermissionStatus::Granted,
    ));
}
