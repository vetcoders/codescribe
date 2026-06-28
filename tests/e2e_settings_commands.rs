//! E2E tests for Settings UI Tauri commands
//!
//! Tests the backend commands used by the simplified Settings UI.
//! Hotkeys contract is mode-first: each WorkMode has one ShortcutBinding.
//!
//! Run with:
//!   cargo test --test e2e_settings_commands
//!
//! Created by M&K (c)2026 VetCoders

use codescribe::config::{Config, Language, ShortcutBinding, UserSettings, WorkMode};
use serial_test::serial;
use tempfile::TempDir;

/// Setup isolated config environment
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // SAFETY: Tests run serially, single-threaded context
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        std::env::remove_var("WHISPER_LANGUAGE");
        std::env::remove_var("AUDIO_INPUT_DEVICE");
        std::env::remove_var("HOLD_MODS");
        std::env::remove_var("TOGGLE_TRIGGER");
    }
    tmp
}

fn set_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    let mut settings = UserSettings::load();
    settings.set_mode_binding(mode, binding);
}

// ═══════════════════════════════════════════════════════════
// Config Load/Save Tests (simulates get_config/save_config commands)
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_get_config_defaults() {
    let _tmp = setup_test_env();

    let settings = UserSettings::load();
    assert_eq!(
        settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldFn,
        "Default dictation binding"
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::DoubleLeftOption,
        "Default formatting binding"
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::DoubleRightOption,
        "Default assistive binding"
    );

    let config = Config::load();
    assert_eq!(config.whisper_language, Language::Auto, "Default language");
}

#[test]
#[serial]
fn test_save_config_dictation_mode_binding() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlAlt);

    let reloaded = UserSettings::load();
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlAlt
    );
}

#[test]
#[serial]
fn test_save_config_formatting_mode_binding() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);

    let reloaded = UserSettings::load();
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled
    );
}

#[test]
#[serial]
fn test_save_config_language() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");

    let reloaded = Config::load();
    assert_eq!(reloaded.whisper_language, Language::English);
}

#[test]
#[serial]
fn test_save_config_audio_device() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config
        .save_to_env("AUDIO_INPUT_DEVICE", "MacBook Pro Microphone")
        .expect("save");

    let reloaded = Config::load();
    assert_eq!(
        reloaded.audio_input_device,
        Some("MacBook Pro Microphone".to_string())
    );
}

#[test]
#[serial]
fn test_save_config_multiple_fields() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlShift);
    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::DoubleRightOption);

    let config = Config::load();
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    config
        .save_to_env("AUDIO_INPUT_DEVICE", "USB Mic")
        .expect("save");

    let reloaded_settings = UserSettings::load();
    assert_eq!(
        reloaded_settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlShift
    );
    assert_eq!(
        reloaded_settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled
    );
    assert_eq!(
        reloaded_settings.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::DoubleRightOption
    );

    let reloaded = Config::load();
    assert_eq!(reloaded.whisper_language, Language::English);
    assert_eq!(reloaded.audio_input_device, Some("USB Mic".to_string()));
}

// ═══════════════════════════════════════════════════════════
// Generic Config Validation Tests
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_empty_audio_device_uses_default() {
    let _tmp = setup_test_env();
    let config = Config::load();
    assert!(
        config.audio_input_device.is_none(),
        "Default should be None (system default)"
    );
}

#[test]
#[serial]
fn test_auto_language_maps_to_auto() {
    let _tmp = setup_test_env();

    unsafe {
        std::env::set_var("WHISPER_LANGUAGE", "auto");
    }

    let config = Config::load();
    assert_eq!(
        config.whisper_language,
        Language::Auto,
        "'auto' should use Whisper language detection"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings UI Flow Simulation
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_full_settings_flow() {
    let _tmp = setup_test_env();

    let settings = UserSettings::load();
    println!(
        "Step 1: Loaded mode bindings: dictation={:?}, formatting={:?}, assistive={:?}",
        settings.mode_binding_for(WorkMode::Dictation),
        settings.mode_binding_for(WorkMode::Formatting),
        settings.mode_binding_for(WorkMode::Assistive)
    );

    println!("Step 2: User modifies mode bindings + language");
    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlCmd);
    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
    let config = Config::load();
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");

    let reloaded_settings = UserSettings::load();
    let reloaded = Config::load();
    println!(
        "Step 3: Reloaded mode bindings: dictation={:?}, formatting={:?}, assistive={:?}",
        reloaded_settings.mode_binding_for(WorkMode::Dictation),
        reloaded_settings.mode_binding_for(WorkMode::Formatting),
        reloaded_settings.mode_binding_for(WorkMode::Assistive)
    );

    assert_eq!(
        reloaded_settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlCmd
    );
    assert_eq!(
        reloaded_settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled
    );
    assert_eq!(reloaded.whisper_language, Language::English);
}

#[test]
#[serial]
fn test_all_dictation_mode_binding_variants() {
    let _tmp = setup_test_env();

    let variants = [
        ShortcutBinding::HoldFn,
        ShortcutBinding::HoldCtrl,
        ShortcutBinding::HoldCtrlAlt,
        ShortcutBinding::HoldCtrlShift,
        ShortcutBinding::HoldCtrlCmd,
        ShortcutBinding::DoubleCtrl,
        ShortcutBinding::Disabled,
    ];

    for binding in variants {
        set_mode_binding(WorkMode::Dictation, binding);
        let reloaded = UserSettings::load();
        assert_eq!(
            reloaded.mode_binding_for(WorkMode::Dictation),
            binding,
            "Failed for binding: {}",
            binding.as_str()
        );
    }
}

#[test]
#[serial]
fn test_toggle_mode_binding_variants() {
    let _tmp = setup_test_env();

    for (mode, variants) in [
        (
            WorkMode::Formatting,
            [ShortcutBinding::DoubleLeftOption, ShortcutBinding::Disabled],
        ),
        (
            WorkMode::Assistive,
            [
                ShortcutBinding::DoubleRightOption,
                ShortcutBinding::Disabled,
            ],
        ),
    ] {
        for binding in variants {
            set_mode_binding(mode, binding);
            let reloaded = UserSettings::load();
            assert_eq!(
                reloaded.mode_binding_for(mode),
                binding,
                "Failed for mode={} binding={}",
                mode.as_str(),
                binding.as_str()
            );
        }
    }
}

#[test]
#[serial]
fn test_all_language_variants() {
    let _tmp = setup_test_env();

    let variants = [
        ("auto", Language::Auto),
        ("detect", Language::Auto),
        ("multilingual", Language::Auto),
        ("any", Language::Auto),
        ("", Language::Auto),
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
