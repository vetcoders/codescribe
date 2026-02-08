//! E2E tests for configuration persistence
//!
//! Tests that Config changes via save_to_env() persist correctly and
//! are reloaded properly. This validates the fix for tray menu toggles.
//!
//! Run with:
//!   cargo test --test e2e_config_persistence
//!
//! Created by M&K (c)2026 VetCoders

use codescribe::config::{Config, HoldMods, ToggleTrigger};
use serial_test::serial;
use tempfile::TempDir;

/// Setup isolated config environment
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // SAFETY: Tests run serially, single-threaded context
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        // Clear ALL config env vars that might interfere
        std::env::remove_var("HOLD_MODS");
        std::env::remove_var("TOGGLE_TRIGGER");
        std::env::remove_var("HOLD_EXCLUSIVE");
        std::env::remove_var("AI_FORMATTING_ENABLED");
        std::env::remove_var("HOLD_START_DELAY_MS");
        std::env::remove_var("WHISPER_LANGUAGE");
        std::env::remove_var("BEEP_ON_START");
        std::env::remove_var("SOUND_VOLUME");
        std::env::remove_var("SOUND_NAME");
        std::env::remove_var("HISTORY_ENABLED");
        std::env::remove_var("DUMP_AUDIO_LOGS");
    }
    tmp
}

/// Test that HoldMods persists after save_to_env
#[test]
#[serial]
fn test_hold_mods_persists() {
    let _tmp = setup_test_env();

    // Load default config
    let config = Config::load();
    assert_eq!(config.hold_mods, HoldMods::Fn, "Default should be Fn");

    // Change to CtrlShift and save
    config
        .save_to_env("HOLD_MODS", "ctrl_shift")
        .expect("save_to_env");

    // Reload config - should reflect the change
    let reloaded = Config::load();
    assert_eq!(
        reloaded.hold_mods,
        HoldMods::CtrlShift,
        "After save, should be CtrlShift"
    );

    // Verify env var was also set (runtime update)
    let env_val = std::env::var("HOLD_MODS").expect("HOLD_MODS should be set");
    assert_eq!(env_val, "ctrl_shift");
}

/// Test that ToggleTrigger persists after save_to_env
#[test]
#[serial]
fn test_toggle_trigger_persists() {
    let _tmp = setup_test_env();

    let config = Config::load();
    assert_eq!(
        config.toggle_trigger,
        ToggleTrigger::DoubleOption,
        "Default should be DoubleOption"
    );

    // Change to None (disabled) and save
    config
        .save_to_env("TOGGLE_TRIGGER", "none")
        .expect("save_to_env");

    let reloaded = Config::load();
    assert_eq!(
        reloaded.toggle_trigger,
        ToggleTrigger::None,
        "After save, should be None"
    );

    // Verify env var
    let env_val = std::env::var("TOGGLE_TRIGGER").expect("TOGGLE_TRIGGER should be set");
    assert_eq!(env_val, "none");
}

/// Test that AI Formatting toggle persists
#[test]
#[serial]
fn test_ai_formatting_toggle_persists() {
    let _tmp = setup_test_env();

    // Test toggling ON
    let config = Config::load();
    config
        .save_to_env("AI_FORMATTING_ENABLED", "1")
        .expect("save_to_env");

    let reloaded = Config::load();
    assert!(
        reloaded.ai_formatting_enabled,
        "After save=1, should be enabled"
    );

    // Test toggling OFF
    config
        .save_to_env("AI_FORMATTING_ENABLED", "0")
        .expect("save_to_env");

    let reloaded2 = Config::load();
    assert!(
        !reloaded2.ai_formatting_enabled,
        "After save=0, should be disabled"
    );

    // Toggle back ON to verify round-trip
    config
        .save_to_env("AI_FORMATTING_ENABLED", "1")
        .expect("save_to_env");

    let reloaded3 = Config::load();
    assert!(
        reloaded3.ai_formatting_enabled,
        "After save=1 again, should be enabled"
    );
}

/// Test that hold_exclusive persists
#[test]
#[serial]
fn test_hold_exclusive_persists() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Test setting to false
    config
        .save_to_env("HOLD_EXCLUSIVE", "0")
        .expect("save_to_env");

    let reloaded = Config::load();
    assert!(!reloaded.hold_exclusive, "After save=0, should be false");

    // Test setting to true
    config
        .save_to_env("HOLD_EXCLUSIVE", "1")
        .expect("save_to_env");

    let reloaded2 = Config::load();
    assert!(reloaded2.hold_exclusive, "After save=1, should be true");
}

/// Test multiple config changes in sequence
#[test]
#[serial]
fn test_multiple_config_changes() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Make multiple changes
    config
        .save_to_env("HOLD_MODS", "ctrl_alt")
        .expect("save hold_mods");
    config
        .save_to_env("TOGGLE_TRIGGER", "double_ralt")
        .expect("save toggle_trigger");
    config
        .save_to_env("AI_FORMATTING_ENABLED", "1")
        .expect("save ai_formatting");

    // Reload and verify all changes persisted
    let reloaded = Config::load();
    assert_eq!(reloaded.hold_mods, HoldMods::CtrlAlt);
    assert_eq!(reloaded.toggle_trigger, ToggleTrigger::DoubleRightOption);
    assert!(reloaded.ai_formatting_enabled);
}

/// Test that .env file is actually written
#[test]
#[serial]
fn test_env_file_created() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config
        .save_to_env("HOLD_MODS", "ctrl_cmd")
        .expect("save_to_env");

    // HOLD_MODS is a regular-user key → persisted to settings.json (not .env)
    let settings = codescribe::config::UserSettings::load();
    assert_eq!(
        settings.hold_mods.as_deref(),
        Some("ctrl_cmd"),
        "settings.json should contain hold_mods=ctrl_cmd"
    );

    // Also verify round-trip through Config::load
    let reloaded = Config::load();
    assert_eq!(reloaded.hold_mods.as_str(), "ctrl_cmd");
}

/// Test all HoldMods variants can be saved and loaded
#[test]
#[serial]
fn test_all_hold_mods_variants() {
    let _tmp = setup_test_env();

    let variants = [
        ("fn", HoldMods::Fn),
        ("ctrl", HoldMods::Ctrl),
        ("ctrl_alt", HoldMods::CtrlAlt),
        ("ctrl_shift", HoldMods::CtrlShift),
        ("ctrl_cmd", HoldMods::CtrlCmd),
    ];

    for (str_val, expected) in variants {
        let config = Config::load();
        config.save_to_env("HOLD_MODS", str_val).expect("save");

        let reloaded = Config::load();
        assert_eq!(
            reloaded.hold_mods, expected,
            "Failed for variant: {}",
            str_val
        );
    }
}

/// Test all ToggleTrigger variants can be saved and loaded
#[test]
#[serial]
fn test_all_toggle_trigger_variants() {
    let _tmp = setup_test_env();

    let variants = [
        ("double_option", ToggleTrigger::DoubleOption),
        ("double_lalt", ToggleTrigger::DoubleLeftOption),
        ("double_ralt", ToggleTrigger::DoubleRightOption),
        ("double_ctrl", ToggleTrigger::DoubleCtrl),
        ("none", ToggleTrigger::None),
    ];

    for (str_val, expected) in variants {
        let config = Config::load();
        config.save_to_env("TOGGLE_TRIGGER", str_val).expect("save");

        let reloaded = Config::load();
        assert_eq!(
            reloaded.toggle_trigger, expected,
            "Failed for variant: {}",
            str_val
        );
    }
}
