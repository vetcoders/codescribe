//! E2E tests for recording state machine logic.
//!
//! Tests state transitions and hotkey prerequisites without hardware.
//! Hotkey contract is mode-first (WorkMode -> ShortcutBinding).
//!
//! Run with:
//!   cargo test --test e2e_state_machine
//!
//! Created by M&K (c)2026 VetCoders

use codescribe::config::{Config, Language, ShortcutBinding, UserSettings, WorkMode};
use serial_test::serial;
use tempfile::TempDir;

/// Setup isolated test environment
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
    }
    tmp
}

fn set_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    let mut settings = UserSettings::load();
    settings.set_mode_binding(mode, binding);
}

#[test]
#[serial]
fn test_dictation_hold_ctrl_shift_binding() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlShift);

    let reloaded = UserSettings::load();
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlShift
    );
}

#[test]
#[serial]
fn test_hold_start_delay_config() {
    let _tmp = setup_test_env();

    let config = Config::load();
    assert!(
        config.hold_start_delay_ms >= 100 && config.hold_start_delay_ms <= 2000,
        "Default hold delay should be between 100-2000ms, got: {}",
        config.hold_start_delay_ms
    );

    config
        .save_to_env("HOLD_START_DELAY_MS", "500")
        .expect("save");

    let reloaded = Config::load();
    assert_eq!(reloaded.hold_start_delay_ms, 500);
}

#[test]
#[serial]
fn test_toggle_modes_can_be_disabled() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::Disabled);

    let reloaded = UserSettings::load();
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled
    );
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::Disabled
    );
}

#[test]
#[serial]
fn test_hold_exclusive_mode() {
    let _tmp = setup_test_env();

    let config = Config::load();

    config.save_to_env("HOLD_EXCLUSIVE", "0").expect("save");
    let reloaded = Config::load();
    assert!(!reloaded.hold_exclusive, "After save=0, should be false");

    config.save_to_env("HOLD_EXCLUSIVE", "1").expect("save");
    let reloaded2 = Config::load();
    assert!(reloaded2.hold_exclusive, "After save=1, should be true");
}

#[test]
#[serial]
fn test_mode_bindings_reload_from_settings_storage() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlCmd);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::DoubleRightOption);

    let reloaded = UserSettings::load();
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlCmd
    );
    assert_eq!(
        reloaded.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::DoubleRightOption
    );
}

#[test]
#[serial]
fn test_assistive_mode_concept() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrl);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::DoubleRightOption);

    let settings = UserSettings::load();
    assert_eq!(
        settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrl
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::DoubleRightOption
    );
}

#[test]
fn test_state_machine_documentation() {
    // State machine transitions:
    //
    // IDLE + dictation hold binding down -> (wait delay) -> REC_HOLD
    //   - release before delay: cancel, stay IDLE
    //
    // REC_HOLD + dictation hold binding up -> BUSY
    //   - stop recording
    //   - process: transcribe -> format -> paste
    //
    // IDLE + toggle mode binding press -> REC_TOGGLE
    // REC_TOGGLE + same binding press -> BUSY
    //
    // BUSY -> (processing complete) -> IDLE
    let _doc = "State machine documentation lives in comments above.";
}

#[test]
#[serial]
fn test_whisper_language_config() {
    let _tmp = setup_test_env();

    let config = Config::load();
    assert_eq!(config.whisper_language, Language::Polish);

    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    let reloaded = Config::load();
    assert_eq!(reloaded.whisper_language, Language::English);
}

#[test]
#[serial]
fn test_sound_config() {
    let _tmp = setup_test_env();

    let config = Config::load();

    config.save_to_env("BEEP_ON_START", "0").expect("save");
    let reloaded = Config::load();
    assert!(!reloaded.beep_on_start, "After save=0, should be false");

    config.save_to_env("BEEP_ON_START", "1").expect("save");
    let reloaded2 = Config::load();
    assert!(reloaded2.beep_on_start, "After save=1, should be true");

    config.save_to_env("SOUND_VOLUME", "0.5").expect("save");
    let reloaded3 = Config::load();
    assert!((reloaded3.sound_volume - 0.5).abs() < 0.01);
}
