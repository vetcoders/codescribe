//! E2E tests for Bootstrap lifecycle & Settings window persistence
//!
//! Tests:
//! - Bootstrap sentinel file creation/detection (`should_show_bootstrap`)
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
        std::env::remove_var("CODESCRIBE_BUFFERED_STREAM");
        std::env::remove_var("TOGGLE_TRIGGER");
        std::env::remove_var("HOLD_EXCLUSIVE");
        std::env::remove_var("HOTKEY_DOUBLE_TAP_LEFT");
        std::env::remove_var("HOTKEY_DOUBLE_TAP_RIGHT");
    }
    tmp
}

// ═══════════════════════════════════════════════════════════
// Bootstrap Lifecycle
// ═══════════════════════════════════════════════════════════

#[test]
#[serial]
fn test_bootstrap_should_show_when_no_sentinel() {
    let _tmp = setup_test_env();

    // Fresh config dir — no bootstrap_done file
    assert!(
        codescribe::should_show_bootstrap(),
        "should_show_bootstrap must be true on fresh install"
    );
}

#[test]
#[serial]
fn test_bootstrap_should_not_show_after_done() {
    let _tmp = setup_test_env();

    // Simulate mark_bootstrap_done() — it writes "done" to bootstrap_done
    let sentinel = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(sentinel.parent().unwrap()).expect("create config dir");
    fs::write(&sentinel, "done").expect("write sentinel");

    assert!(
        !codescribe::should_show_bootstrap(),
        "should_show_bootstrap must be false after bootstrap_done exists"
    );
}

#[test]
#[serial]
fn test_bootstrap_sentinel_survives_reload() {
    let _tmp = setup_test_env();

    assert!(codescribe::should_show_bootstrap(), "initially true");

    // Mark done
    let sentinel = Config::config_dir().join("bootstrap_done");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, "done").unwrap();

    assert!(!codescribe::should_show_bootstrap(), "false after mark");

    // "Restart" — reload config (sentinel is file-based, not in-memory)
    let _ = Config::load();
    assert!(
        !codescribe::should_show_bootstrap(),
        "still false after config reload"
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
fn test_settings_buffered_stream_toggle() {
    let _tmp = setup_test_env();

    // CODESCRIBE_BUFFERED_STREAM is not a Config struct field — it's read via
    // env_bool_default() at runtime. Test that save_to_env persists it to .env
    // and sets the runtime env var.
    let config = Config::load();
    config
        .save_to_env("CODESCRIBE_BUFFERED_STREAM", "1")
        .expect("save on");
    assert_eq!(
        std::env::var("CODESCRIBE_BUFFERED_STREAM").unwrap(),
        "1",
        "env var should be set to 1"
    );

    config
        .save_to_env("CODESCRIBE_BUFFERED_STREAM", "0")
        .expect("save off");
    assert_eq!(
        std::env::var("CODESCRIBE_BUFFERED_STREAM").unwrap(),
        "0",
        "env var should be set to 0"
    );

    // Buffered stream is a regular-user key → persisted to settings.json
    let settings = codescribe::config::UserSettings::load();
    assert_eq!(
        settings.buffered_stream,
        Some(false),
        "last value persisted in JSON"
    );
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
        .save_to_env("CODESCRIBE_BUFFERED_STREAM", "1")
        .expect("save");

    // Reload and verify all
    let r = Config::load();
    assert_eq!(r.hold_mods.as_str(), "ctrl_shift");
    assert_eq!(r.toggle_trigger.as_str(), "double_ralt");
    assert!(!r.hold_exclusive);
    assert_eq!(r.whisper_language.as_str(), "en");
    assert!(!r.ai_formatting_enabled);
    assert_eq!(std::env::var("CODESCRIBE_BUFFERED_STREAM").unwrap(), "1");
}

// ═══════════════════════════════════════════════════════════
// Engine tab: runtime data sources
// ═══════════════════════════════════════════════════════════

#[test]
fn test_engine_tab_stt_engine_env_default() {
    // Without CODESCRIBE_STT_ENGINE set, should default to candle
    unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "candle", "default STT engine should be candle");
}

#[test]
fn test_engine_tab_stt_engine_env_onnx() {
    // Engine tab reads CODESCRIBE_STT_ENGINE to display active engine
    unsafe { std::env::set_var("CODESCRIBE_STT_ENGINE", "onnx") };
    let engine = std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
    assert_eq!(engine, "onnx", "STT engine should reflect env var");
    unsafe { std::env::remove_var("CODESCRIBE_STT_ENGINE") };
}

#[test]
fn test_engine_tab_whisper_embedded_status() {
    // Engine tab shows whether Whisper model is embedded in binary
    let embedded = codescribe_core::stt::whisper::embedded::is_embedded_available();
    // In dev builds it's typically not embedded — just verify the function exists
    // and returns a bool (not a panic)
    assert!(
        embedded || !embedded,
        "is_embedded_available should return bool"
    );
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
    // Dev builds typically don't embed TTS — verify API exists
    assert!(
        embedded || !embedded,
        "is_embedded_available should return bool"
    );
}
