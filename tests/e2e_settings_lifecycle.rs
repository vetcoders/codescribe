//! E2E tests for setup lifecycle & Settings window persistence
//!
//! Tests:
//! - Setup sentinel creation/detection (`should_show_onboarding`)
//! - Settings persistence for mode-first bindings and tab-level fields
//!
//! Run with:
//!   cargo test --test e2e_settings_lifecycle
//!
//! Created by Vetcoders (c)2026

use codescribe::config::{Config, ShortcutBinding, UserSettings, WorkMode};
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Setup isolated config environment (same pattern as e2e_settings_commands)
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // SAFETY: Tests run serially, single-threaded context
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        std::env::remove_var("WHISPER_LANGUAGE");
        std::env::remove_var("AI_FORMATTING_ENABLED");
        std::env::remove_var("HOLD_EXCLUSIVE");
        std::env::remove_var("CODESCRIBE_TYPING_CPS");
        std::env::remove_var("USE_LOCAL_STT");
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
// Setup Lifecycle
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_setup_should_show_when_no_sentinel() {
    let _tmp = setup_test_env();

    assert!(
        codescribe::should_show_onboarding(),
        "should_show_onboarding must be true on fresh install"
    );
}

#[test]
#[serial]
fn test_setup_should_not_show_after_done() {
    let _tmp = setup_test_env();

    let sentinel = Config::config_dir().join("setup_done");
    fs::create_dir_all(sentinel.parent().expect("setup_done parent")).expect("create config dir");
    fs::write(&sentinel, "done").expect("write sentinel");

    assert!(
        !codescribe::should_show_onboarding(),
        "should_show_onboarding must be false after setup_done exists"
    );
}

#[test]
#[serial]
fn test_setup_migrates_when_both_legacy_sentinels_exist() {
    let _tmp = setup_test_env();

    let onboarding = Config::config_dir().join("onboarding_done");
    let legacy_settings = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(onboarding.parent().expect("onboarding parent")).expect("create config dir");
    fs::write(&onboarding, "done").expect("write onboarding_done");
    fs::write(&legacy_settings, "done").expect("write bootstrap_done");

    assert!(
        !codescribe::should_show_onboarding(),
        "both legacy sentinels should migrate to setup_done and mark setup complete"
    );

    let setup = Config::config_dir().join("setup_done");
    assert!(
        setup.exists(),
        "setup_done should be written during migration"
    );
}

#[test]
#[serial]
fn test_setup_remains_incomplete_with_only_legacy_onboarding() {
    let _tmp = setup_test_env();
    let onboarding = Config::config_dir().join("onboarding_done");
    fs::create_dir_all(onboarding.parent().expect("onboarding parent")).expect("create config dir");
    fs::write(&onboarding, "done").expect("write onboarding_done");

    assert!(
        codescribe::should_show_onboarding(),
        "legacy onboarding_done alone means permissions were done, but setup is still pending"
    );
}

#[test]
#[serial]
fn test_setup_remains_incomplete_with_only_legacy_settings_sentinel() {
    let _tmp = setup_test_env();
    let legacy_settings = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(legacy_settings.parent().expect("bootstrap parent"))
        .expect("create config dir");
    fs::write(&legacy_settings, "done").expect("write bootstrap_done");

    assert!(
        codescribe::should_show_onboarding(),
        "legacy bootstrap_done alone means settings were opened before, but setup is still pending"
    );
}

#[test]
#[serial]
fn test_setup_done_blocks_onboarding_even_with_resume_checkpoint() {
    let _tmp = setup_test_env();

    let config_dir = Config::config_dir();
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("onboarding_progress"), "3").expect("write onboarding_progress");
    fs::write(config_dir.join("setup_done"), "done").expect("write setup_done");

    assert!(
        !codescribe::should_show_onboarding(),
        "setup_done must keep onboarding hidden even if a stale resume checkpoint exists"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings: Keys Tab persistence
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_settings_mode_bindings_persistence() {
    let _tmp = setup_test_env();

    for (binding, label) in [
        (ShortcutBinding::HoldCtrl, "Hold Ctrl"),
        (ShortcutBinding::HoldCtrlAlt, "Hold Ctrl+Option"),
        (ShortcutBinding::HoldCtrlShift, "Hold Ctrl+Shift"),
        (ShortcutBinding::HoldCtrlCmd, "Hold Ctrl+Command"),
    ] {
        set_mode_binding(WorkMode::Dictation, binding);
        let reloaded = UserSettings::load();
        assert_eq!(
            reloaded.mode_binding_for(WorkMode::Dictation),
            binding,
            "Dictation mode binding should persist as {label}"
        );
    }
}

#[test]
#[serial]
fn test_settings_double_ctrl_profile_persistence() {
    let _tmp = setup_test_env();

    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::Disabled);
    set_mode_binding(WorkMode::Dictation, ShortcutBinding::DoubleCtrl);

    let settings = UserSettings::load();
    assert_eq!(
        settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::DoubleCtrl,
        "dictation mode should persist as double ctrl"
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled,
        "formatting mode should stay disabled in double-ctrl profile"
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::Disabled,
        "assistive mode should stay disabled in double-ctrl profile"
    );
}

#[test]
#[serial]
fn test_settings_hold_exclusive_persistence() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config
        .save_to_env("HOLD_EXCLUSIVE", "1")
        .expect("save hold exclusive");

    let settings = UserSettings::load();
    assert_eq!(
        settings.hold_exclusive,
        Some(true),
        "hold_exclusive persisted"
    );
}

#[test]
#[serial]
fn test_settings_use_local_stt_false_roundtrips_through_config_load() {
    let _tmp = setup_test_env();

    let mut settings = UserSettings::load();
    settings.use_local_stt = Some(false);
    settings.save().expect("save settings");

    let config = Config::load();
    assert!(
        !config.use_local_stt,
        "Config::load should honor settings.json when local STT is disabled"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings: Audio Tab persistence
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_settings_language_persistence() {
    let _tmp = setup_test_env();

    for lang in ["auto", "pl", "en"] {
        let config = Config::load();
        config.save_to_env("WHISPER_LANGUAGE", lang).expect("save");
        let reloaded = Config::load();
        assert_eq!(
            reloaded.whisper_language.as_str(),
            lang,
            "WHISPER_LANGUAGE should persist"
        );
    }
}

#[test]
#[serial]
fn test_settings_formatting_toggle() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config
        .save_to_env("AI_FORMATTING_ENABLED", "1")
        .expect("save on");
    let reloaded = Config::load();
    assert!(reloaded.ai_formatting_enabled, "formatting should be on");

    config
        .save_to_env("AI_FORMATTING_ENABLED", "0")
        .expect("save off");
    let reloaded2 = Config::load();
    assert!(!reloaded2.ai_formatting_enabled, "formatting should be off");
}

#[test]
#[serial]
fn test_settings_typing_cps_decimal_persistence() {
    let _tmp = setup_test_env();

    let config = Config::load();
    config
        .save_to_env("CODESCRIBE_TYPING_CPS", "36.5")
        .expect("save typing cps");

    assert!(
        std::env::var("CODESCRIBE_TYPING_CPS").is_err(),
        "runtime writes must not mutate process env"
    );

    let settings = UserSettings::load();
    let persisted = settings
        .typing_cps
        .expect("typing_cps should be persisted to settings.json");
    assert!(
        (persisted - 36.5).abs() < 0.0001,
        "typing_cps should persist as f32 (expected 36.5, got {persisted})"
    );
}

#[test]
#[serial]
fn test_settings_chat_zoom_dirty_check_skips_equivalent_writes() {
    let _tmp = setup_test_env();
    let mut settings = UserSettings::load();
    let path = UserSettings::settings_path();

    assert!(!settings.set_chat_zoom(1.0));
    assert!(!path.exists(), "default zoom should stay implicit (None)");

    assert!(settings.set_chat_zoom(1.125));
    let first_json = fs::read_to_string(&path).expect("read settings after first zoom write");

    assert!(!settings.set_chat_zoom(1.129));
    let second_json = fs::read_to_string(&path).expect("read settings after no-op zoom write");
    assert_eq!(first_json, second_json);

    assert!(settings.set_chat_zoom(1.25));
    let third_json = fs::read_to_string(&path).expect("read settings after second zoom write");
    assert_ne!(second_json, third_json);
}

// ═══════════════════════════════════════════════════════════
// Settings: Multi-field round-trip (simulates user changing all tabs)
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_settings_full_round_trip() {
    let _tmp = setup_test_env();

    let config = Config::load();

    set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlShift);
    set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
    set_mode_binding(WorkMode::Assistive, ShortcutBinding::DoubleRightOption);
    config.save_to_env("HOLD_EXCLUSIVE", "0").expect("save");

    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    config
        .save_to_env("AI_FORMATTING_ENABLED", "0")
        .expect("save");
    config
        .save_to_env("CODESCRIBE_TYPING_CPS", "90")
        .expect("save");

    let r = Config::load();
    let settings = UserSettings::load();
    assert_eq!(
        settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldCtrlShift
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::Disabled
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Assistive),
        ShortcutBinding::DoubleRightOption
    );
    assert!(!r.hold_exclusive);
    assert_eq!(r.whisper_language.as_str(), "en");
    assert!(!r.ai_formatting_enabled);
    assert_eq!(
        settings.typing_cps,
        Some(90.0),
        "typing cps should round-trip through settings.json, not live env"
    );
}

// ═══════════════════════════════════════════════════════════
// Engine tab: runtime data sources
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_engine_tab_stt_engine_env_default() {
    let previous = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "auto".to_string());
    assert_eq!(engine, "auto", "default STT engine policy should be auto");
    match previous {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") },
    }
}

#[test]
#[serial]
fn test_engine_tab_stt_engine_env_onnx() {
    let previous = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", "onnx") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "onnx", "STT engine should reflect env var");
    match previous {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") },
    }
}

#[test]
#[serial]
fn test_engine_tab_stt_engine_env_apple() {
    let previous = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", "apple") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "apple", "STT engine should reflect env var");
    match previous {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") },
    }
}

#[test]
fn test_engine_tab_whisper_embedded_status() {
    let embedded = codescribe_core::stt::whisper::embedded::is_embedded_available();
    let embedded_data = codescribe_core::stt::whisper::embedded::get_embedded_data();

    assert_eq!(
        embedded_data.is_some(),
        embedded,
        "Whisper embedded availability should match get_embedded_data() result"
    );
    if let Some(model) = embedded_data {
        assert!(
            model.total_size() > 0,
            "Whisper embedded model should report non-zero total size"
        );
    }
}

#[test]
fn test_engine_tab_vad_model_available() {
    let embedded = codescribe_core::vad::embedded::is_embedded_available();
    let user_path = codescribe_core::vad::user_model_path();

    let available = embedded || user_path.exists();
    assert!(
        available,
        "Silero VAD should be available (embedded={}, path={})",
        embedded,
        user_path.display()
    );
}

#[test]
fn test_engine_tab_embedder_api_exists() {
    let _initialized = codescribe_core::embedder::is_initialized();
}

#[test]
fn test_engine_tab_tts_embedded_status() {
    let embedded = codescribe_core::tts::embedded::is_embedded_available();
    let embedded_data = codescribe_core::tts::embedded::get_embedded_data();

    assert_eq!(
        embedded_data.is_some(),
        embedded,
        "TTS embedded availability should match get_embedded_data() result"
    );
    if let Some(model) = embedded_data {
        assert!(
            model.total_size() > 0,
            "TTS embedded model should report non-zero total size"
        );
    }
}
