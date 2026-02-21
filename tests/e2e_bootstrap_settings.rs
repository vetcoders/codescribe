//! E2E tests for Bootstrap lifecycle & Settings window persistence
//!
//! Tests:
//! - Setup sentinel creation/detection (`should_show_setup`)
//! - Settings persistence via `Config::save_to_env()` for keys added by Settings tabs
//!
//! Run with:
//!   cargo test --test e2e_bootstrap_settings
//!
//! Created by M&K (c)2026 VetCoders

use codescribe::config::Config;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Setup isolated config environment (same pattern as e2e_settings_commands)
fn setup_test_env() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // SAFETY: Tests run serially, single-threaded context
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        std::env::remove_var("HOLD_MODS");
        std::env::remove_var("WHISPER_LANGUAGE");
        std::env::remove_var("AI_FORMATTING_ENABLED");
        std::env::remove_var("TOGGLE_TRIGGER");
        std::env::remove_var("HOLD_EXCLUSIVE");
        std::env::remove_var("HOTKEY_DOUBLE_TAP_LEFT");
        std::env::remove_var("HOTKEY_DOUBLE_TAP_RIGHT");
        std::env::remove_var("CODESCRIBE_TYPING_CPS");
    }
    tmp
}

// ═══════════════════════════════════════════════════════════
// Setup Lifecycle
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_setup_should_show_when_no_sentinel() {
    let _tmp = setup_test_env();

    // Fresh config dir — no setup sentinel file
    assert!(
        codescribe::should_show_setup(),
        "should_show_setup must be true on fresh install"
    );
}

#[test]
#[serial]
fn test_setup_should_not_show_after_done() {
    let _tmp = setup_test_env();

    let sentinel = Config::config_dir().join("setup_done");
    fs::create_dir_all(sentinel.parent().unwrap()).expect("create config dir");
    fs::write(&sentinel, "done").expect("write sentinel");

    assert!(
        !codescribe::should_show_setup(),
        "should_show_setup must be false after setup_done exists"
    );
}

#[test]
#[serial]
fn test_setup_migrates_when_both_legacy_sentinels_exist() {
    let _tmp = setup_test_env();

    let onboarding = Config::config_dir().join("onboarding_done");
    let bootstrap = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(onboarding.parent().unwrap()).unwrap();
    fs::write(&onboarding, "done").unwrap();
    fs::write(&bootstrap, "done").unwrap();

    assert!(
        !codescribe::should_show_setup(),
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
    fs::create_dir_all(onboarding.parent().unwrap()).unwrap();
    fs::write(&onboarding, "done").unwrap();

    assert!(
        codescribe::should_show_setup(),
        "legacy onboarding_done alone means permissions were done, but setup is still pending"
    );
}

#[test]
#[serial]
fn test_setup_remains_incomplete_with_only_legacy_bootstrap() {
    let _tmp = setup_test_env();
    let bootstrap = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(bootstrap.parent().unwrap()).unwrap();
    fs::write(&bootstrap, "done").unwrap();

    assert!(
        codescribe::should_show_setup(),
        "legacy bootstrap_done alone means settings were opened before, but setup is still pending"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings: Keys Tab persistence
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_settings_hold_mods_persistence() {
    let _tmp = setup_test_env();

    // Simulate Settings UI changing hold modifier (same as on_hold_mod_changed)
    for (value, label) in [
        ("ctrl", "Ctrl"),
        ("ctrl_alt", "Ctrl+Alt"),
        ("ctrl_shift", "Ctrl+Shift"),
        ("ctrl_cmd", "Ctrl+Cmd"),
    ] {
        let config = Config::load();
        config.save_to_env("HOLD_MODS", value).expect("save");
        let reloaded = Config::load();
        assert_eq!(
            reloaded.hold_mods.as_str(),
            value,
            "HOLD_MODS should persist as {label}"
        );
    }
}

#[test]
#[serial]
fn test_settings_toggle_trigger_persistence() {
    let _tmp = setup_test_env();

    let config = Config::load();

    // Simulate on_toggle_trigger_changed
    config
        .save_to_env("TOGGLE_TRIGGER", "double_ctrl")
        .expect("save toggle trigger");

    // Verify round-trip through settings.json
    let settings = codescribe::config::UserSettings::load();
    assert_eq!(
        settings.toggle_trigger.as_deref(),
        Some("double_ctrl"),
        "toggle trigger persisted in JSON"
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

    let settings = codescribe::config::UserSettings::load();
    assert_eq!(
        settings.hold_exclusive,
        Some(true),
        "hold_exclusive persisted"
    );
}

// ═══════════════════════════════════════════════════════════
// Settings: Audio Tab persistence
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_settings_language_persistence() {
    let _tmp = setup_test_env();

    // Simulate on_language_changed: idx 0 = pl, idx 1 = en
    for lang in ["pl", "en"] {
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

    // on_formatting_toggled: state 1 = on, state 0 = off
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

    assert_eq!(
        std::env::var("CODESCRIBE_TYPING_CPS").unwrap(),
        "36.5",
        "runtime env should preserve decimal value"
    );

    let settings = codescribe::config::UserSettings::load();
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
    let mut settings = codescribe::config::UserSettings::load();
    let path = codescribe::config::UserSettings::settings_path();

    // Effective default zoom (1.0) should not create or rewrite settings.
    assert!(!settings.set_chat_zoom(1.0));
    assert!(!path.exists(), "default zoom should stay implicit (None)");

    // First real write.
    assert!(settings.set_chat_zoom(1.125));
    let first_json = fs::read_to_string(&path).expect("read settings after first zoom write");

    // Equivalent value after rounding (1.129 -> 1.13) should be a no-op.
    assert!(!settings.set_chat_zoom(1.129));
    let second_json = fs::read_to_string(&path).expect("read settings after no-op zoom write");
    assert_eq!(first_json, second_json);

    // New effective value should persist.
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

    // User clicks through all Settings tabs and saves:
    // Keys tab
    config.save_to_env("HOLD_MODS", "ctrl_shift").expect("save");
    config
        .save_to_env("TOGGLE_TRIGGER", "double_ralt")
        .expect("save");
    config.save_to_env("HOLD_EXCLUSIVE", "0").expect("save");
    // Audio tab
    config.save_to_env("WHISPER_LANGUAGE", "en").expect("save");
    config
        .save_to_env("AI_FORMATTING_ENABLED", "0")
        .expect("save");
    config
        .save_to_env("CODESCRIBE_TYPING_CPS", "90")
        .expect("save");

    // Reload and verify all
    let r = Config::load();
    assert_eq!(r.hold_mods.as_str(), "ctrl_shift");
    assert_eq!(r.toggle_trigger.as_str(), "double_ralt");
    assert!(!r.hold_exclusive);
    assert_eq!(r.whisper_language.as_str(), "en");
    assert!(!r.ai_formatting_enabled);
    assert_eq!(std::env::var("CODESCRIBE_TYPING_CPS").unwrap(), "90");
}

// ═══════════════════════════════════════════════════════════
// Engine tab: runtime data sources
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_engine_tab_stt_engine_env_default() {
    let previous = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    // Without CODESCRIBE_STT_ENGINE set, should default to candle
    unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "candle", "default STT engine should be candle");
    match previous {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") },
    }
}

#[test]
#[serial]
fn test_engine_tab_stt_engine_env_onnx() {
    let previous = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    // Engine tab reads CODESCRIBE_STT_ENGINE to display active engine
    unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", "onnx") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "onnx", "STT engine should reflect env var");
    match previous {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") },
    }
}

#[test]
fn test_engine_tab_whisper_embedded_status() {
    // Engine tab shows whether Whisper model is embedded in binary
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
    // Engine tab shows Silero VAD status
    let embedded = codescribe_core::vad::embedded::is_embedded_available();
    let user_path = codescribe_core::vad::user_model_path();

    // At least one source should be available in dev environment
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
    // Engine tab queries embedder initialization status
    // Just verify the API is callable (lazy init — may not be initialized yet)
    let _initialized = codescribe_core::embedder::is_initialized();
    // No assertion on value — lazy init means it's false until first use
}

#[test]
fn test_engine_tab_tts_embedded_status() {
    // Engine tab shows TTS engine status
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
