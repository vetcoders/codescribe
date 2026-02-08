//! E2E tests for Settings UI Tauri commands
//!
//! Tests the backend commands used by the simplified Settings UI:
//! - get_config / save_config
//!
//! These test the config layer that Tauri frontend invokes.
//!
//! Run with:
//!   cargo test --test e2e_settings_commands
//!
//! Created by M&K (c)2026 VetCoders

use codescribe::config::{Config, HoldMods, Language, ToggleTrigger};
use serial_test::serial;
use tempfile::TempDir;

/// Setup isolated config environment
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // SAFETY: Tests run serially, single-threaded context
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        // Clear relevant env vars
        std::env::remove_var("HOLD_MODS");
        std::env::remove_var("TOGGLE_TRIGGER");
        std::env::remove_var("WHISPER_LANGUAGE");
        std::env::remove_var("AUDIO_INPUT_DEVICE");
    }
    tmp
}

// ═══════════════════════════════════════════════════════════
// Config Load/Save Tests (simulates get_config/save_config commands)
// ═══════════════════════════════════════════════════════════

/// Test: Load config returns expected defaults
#[test]
#[serial]
fn test_get_config_defaults() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Verify defaults match what Settings UI expects
    assert_eq!(config.hold_mods, HoldMods::Fn, "Default hold_mods");
    assert_eq!(
        config.toggle_trigger,
        ToggleTrigger::DoubleOption,
        "Default toggle_trigger"
    );
    assert_eq!(
        config.whisper_language,
        Language::Polish,
        "Default language"
    );
}

/// Test: Save config persists hold_mods
#[test]
#[serial]
fn test_save_config_hold_mods() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate Settings UI changing hold_mods
    config.save_to_env("HOLD_MODS", "ctrl_alt").expect("save");

    // Reload and verify
    let reloaded = Config::load();
    assert_eq!(reloaded.hold_mods, HoldMods::CtrlAlt);
}

/// Test: Save config persists toggle_trigger
#[test]
#[serial]
fn test_save_config_toggle_trigger() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate Settings UI disabling toggle
    config.save_to_env("TOGGLE_TRIGGER", "none").expect("save");

    let reloaded = Config::load();
    assert_eq!(reloaded.toggle_trigger, ToggleTrigger::None);
}

/// Test: Save config persists whisper_language
#[test]
#[serial]
fn test_save_config_language() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate Settings UI setting English
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");

    let reloaded = Config::load();
    assert_eq!(reloaded.whisper_language, Language::English);
}

/// Test: Save config persists audio_input_device
#[test]
#[serial]
fn test_save_config_audio_device() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate Settings UI selecting specific device
    config
        .save_to_env("AUDIO_INPUT_DEVICE", "MacBook Pro Microphone")
        .expect("save");

    let reloaded = Config::load();
    assert_eq!(
        reloaded.audio_input_device,
        Some("MacBook Pro Microphone".to_string())
    );
}

/// Test: Multiple config saves in sequence
#[test]
#[serial]
fn test_save_config_multiple_fields() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate Settings UI save (all fields at once)
    config.save_to_env("HOLD_MODS", "ctrl_shift").expect("save");
    config
        .save_to_env("TOGGLE_TRIGGER", "double_ralt")
        .expect("save");
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    config
        .save_to_env("AUDIO_INPUT_DEVICE", "USB Mic")
        .expect("save");

    // Reload and verify all
    let reloaded = Config::load();
    assert_eq!(reloaded.hold_mods, HoldMods::CtrlShift);
    assert_eq!(reloaded.toggle_trigger, ToggleTrigger::DoubleRightOption);
    assert_eq!(reloaded.whisper_language, Language::English);
    assert_eq!(reloaded.audio_input_device, Some("USB Mic".to_string()));
}

// ═══════════════════════════════════════════════════════════
// Config Validation Tests
// ═══════════════════════════════════════════════════════════

/// Test: Invalid hold_mods value falls back to default
#[test]
#[serial]
fn test_invalid_hold_mods_fallback() {
    let _tmp = setup_test_env();

    // Set invalid value
    unsafe {
        std::env::set_var("HOLD_MODS", "invalid_value");
    }

    let config = Config::load();

    // Should fallback to default (Fn)
    assert_eq!(
        config.hold_mods,
        HoldMods::Fn,
        "Invalid value should fallback to Fn"
    );
}

/// Test: Invalid toggle_trigger value falls back to default
#[test]
#[serial]
fn test_invalid_toggle_trigger_fallback() {
    let _tmp = setup_test_env();

    unsafe {
        std::env::set_var("TOGGLE_TRIGGER", "triple_tap");
    }

    let config = Config::load();

    // Should fallback to default (DoubleOption)
    assert_eq!(
        config.toggle_trigger,
        ToggleTrigger::DoubleOption,
        "Invalid value should fallback to DoubleOption"
    );
}

/// Test: Empty audio_input_device means use system default
#[test]
#[serial]
fn test_empty_audio_device_uses_default() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // None means "use system default"
    assert!(
        config.audio_input_device.is_none(),
        "Default should be None (system default)"
    );
}

/// Test: "auto" language maps to Polish (default)
#[test]
#[serial]
fn test_auto_language_maps_to_polish() {
    let _tmp = setup_test_env();

    unsafe {
        std::env::set_var("WHISPER_LANGUAGE", "auto");
    }

    let config = Config::load();

    // "auto" maps to Polish (the default)
    assert_eq!(
        config.whisper_language,
        Language::Polish,
        "'auto' should map to Polish"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings UI Flow Simulation
// ═══════════════════════════════════════════════════════════

/// Test: Full Settings UI flow - load, modify, save, reload
#[test]
#[serial]
fn test_full_settings_flow() {
    let _tmp = setup_test_env();

    // 1. Initial load (simulates SettingsView mount)
    let config = Config::load();
    println!("Step 1: Loaded config");
    println!("  hold_mods: {:?}", config.hold_mods);
    println!("  toggle_trigger: {:?}", config.toggle_trigger);
    println!("  language: {:?}", config.whisper_language);

    // 2. User changes settings
    println!("Step 2: User modifies settings");

    // 3. User clicks Save (simulates save_config command)
    config.save_to_env("HOLD_MODS", "ctrl_cmd").expect("save");
    config.save_to_env("TOGGLE_TRIGGER", "none").expect("save");
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    println!("Step 3: Saved config");

    // 4. App restart - reload config
    let reloaded = Config::load();
    println!("Step 4: Reloaded config");
    println!("  hold_mods: {:?}", reloaded.hold_mods);
    println!("  toggle_trigger: {:?}", reloaded.toggle_trigger);
    println!("  language: {:?}", reloaded.whisper_language);

    // 5. Verify persistence
    assert_eq!(reloaded.hold_mods, HoldMods::CtrlCmd);
    assert_eq!(reloaded.toggle_trigger, ToggleTrigger::None);
    assert_eq!(reloaded.whisper_language, Language::English);
    println!("Step 5: Verified - all settings persisted correctly");
}

/// Test: Settings with all HoldMods variants
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

    for (value, expected) in variants {
        let config = Config::load();
        config.save_to_env("HOLD_MODS", value).expect("save");
        let reloaded = Config::load();
        assert_eq!(reloaded.hold_mods, expected, "Failed for value: {}", value);
    }
}

/// Test: Settings with all ToggleTrigger variants
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

    for (value, expected) in variants {
        let config = Config::load();
        config.save_to_env("TOGGLE_TRIGGER", value).expect("save");
        let reloaded = Config::load();
        assert_eq!(
            reloaded.toggle_trigger, expected,
            "Failed for value: {}",
            value
        );
    }
}

/// Test: Settings with all Language variants
#[test]
#[serial]
fn test_all_language_variants() {
    let _tmp = setup_test_env();

    let variants = [
        ("pl", Language::Polish),
        ("polish", Language::Polish),
        ("en", Language::English),
        ("english", Language::English),
    ];

    for (value, expected) in variants {
        let config = Config::load();
        config.save_to_env("WHISPER_LANGUAGE", value).expect("save");
        let reloaded = Config::load();
        assert_eq!(
            reloaded.whisper_language, expected,
            "Failed for value: {}",
            value
        );
    }
}
