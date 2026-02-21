//! One-time migration from .env-only to tiered config.
//!
//! Moves non-secret settings into `settings.json` and API keys into macOS Keychain.

use super::keychain;
use super::settings::UserSettings;
use tracing::{debug, info};

/// Runs the one-time migration if `settings.json` does not yet exist.
///
/// 1. Skips if `settings.json` already exists.
/// 2. Reads current env vars to build a `UserSettings`.
/// 3. Saves to `settings.json`.
/// 4. Migrates API keys from env to Keychain.
pub fn migrate_if_needed() {
    let path = UserSettings::settings_path();
    if path.exists() {
        debug!("settings.json already exists, skipping migration");
        return;
    }

    let mut settings = UserSettings::default();

    // Migrate string settings from current env/config state
    if let Ok(v) = std::env::var("WHISPER_LANGUAGE") {
        settings.whisper_language = Some(v);
    }
    if let Ok(v) = std::env::var("HOLD_MODS") {
        settings.hold_mods = Some(v);
    }
    if let Ok(v) = std::env::var("TOGGLE_TRIGGER") {
        settings.toggle_trigger = Some(v);
    }
    if let Ok(v) = std::env::var("LLM_ENDPOINT") {
        settings.llm_endpoint = Some(v);
    }
    if let Ok(v) = std::env::var("LLM_MODEL") {
        settings.llm_model = Some(v);
    }
    if let Ok(v) = std::env::var("LLM_ASSISTIVE_ENDPOINT") {
        settings.llm_assistive_endpoint = Some(v);
    }
    if let Ok(v) = std::env::var("LLM_ASSISTIVE_MODEL") {
        settings.llm_assistive_model = Some(v);
    }
    if let Ok(v) = std::env::var("FORMATTING_LEVEL") {
        settings.formatting_level = Some(v);
    }
    // Promoted fields (previously .env only)
    if let Ok(v) = std::env::var("LLM_FORMATTING_ENDPOINT") {
        settings.llm_formatting_endpoint = Some(v);
    }
    if let Ok(v) = std::env::var("LLM_FORMATTING_MODEL") {
        settings.llm_formatting_model = Some(v);
    }
    if let Ok(v) = std::env::var("LOCAL_MODEL") {
        settings.local_model = Some(v);
    }
    if let Ok(v) = std::env::var("STT_ENDPOINT") {
        settings.stt_endpoint = Some(v);
    }
    if let Ok(v) = std::env::var("TRANSCRIPT_SEND_MODE") {
        settings.transcript_send_mode = Some(v);
    }
    if let Ok(v) = std::env::var("AUDIO_INPUT_DEVICE") {
        settings.audio_input_device = Some(v);
    }
    if let Ok(v) = std::env::var("SOUND_NAME") {
        settings.sound_name = Some(v);
    }
    if let Ok(v) = std::env::var("WHISPER_MODEL") {
        settings.whisper_model = Some(v);
    }

    // Migrate boolean settings
    if let Ok(v) = std::env::var("AI_FORMATTING_ENABLED") {
        settings.ai_formatting_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("BEEP_ON_START") {
        settings.beep_on_start = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("HOLD_EXCLUSIVE") {
        settings.hold_exclusive = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("HOTKEY_DOUBLE_TAP_LEFT") {
        settings.double_tap_left = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("HOTKEY_DOUBLE_TAP_RIGHT") {
        settings.double_tap_right = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    // Promoted booleans
    if let Ok(v) = std::env::var("USE_LOCAL_STT") {
        settings.use_local_stt = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("HISTORY_ENABLED") {
        settings.history_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("QUICK_NOTES_ENABLED") {
        settings.quick_notes_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("QUICK_NOTES_SAVE_ONLY") {
        settings.quick_notes_save_only = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("START_AT_LOGIN") {
        settings.start_at_login = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("AGENT_ENTER_SENDS") {
        settings.agent_enter_sends = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }

    // Migrate numeric settings
    if let Ok(v) = std::env::var("HOLD_START_DELAY_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.hold_start_delay_ms = Some(n);
    }
    if let Ok(v) = std::env::var("SOUND_VOLUME")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.sound_volume = Some(n);
    }
    if let Ok(v) = std::env::var("TOGGLE_SILENCE_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.toggle_silence_sec = Some(n);
    }
    if let Ok(v) = std::env::var("DOUBLE_TAP_INTERVAL_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.double_tap_interval_ms = Some(n);
    }
    // Voice Lab survivors
    if let Ok(v) = std::env::var("CODESCRIBE_BUFFER_DELAY_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.buffer_delay_ms = Some(n);
    }
    if let Ok(v) = std::env::var("CODESCRIBE_TYPING_CPS")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.typing_cps = Some(n);
    }
    if let Ok(v) = std::env::var("CODESCRIBE_EMIT_WORDS_MAX")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.emit_words_max = Some(n);
    }
    if let Ok(v) = std::env::var("CODESCRIBE_BUFFERED_INTERIM_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.buffered_interim_sec = Some(n);
    }
    if let Ok(v) = std::env::var("BACKEND_MAX_UPLOAD_MB")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.backend_max_upload_mb = Some(n);
    }

    // Save settings.json
    if let Err(e) = settings.save() {
        tracing::warn!("Migration: failed to save settings.json: {e}");
        return;
    }

    // Migrate API keys to Keychain
    for &account in keychain::KEYCHAIN_ACCOUNTS {
        if let Ok(secret) = std::env::var(account)
            && !secret.is_empty()
            && let Err(e) = keychain::save_key(account, &secret)
        {
            tracing::warn!("Migration: failed to save {account} to Keychain: {e}");
        }
    }

    info!("Migrated config to settings.json + Keychain");
}
