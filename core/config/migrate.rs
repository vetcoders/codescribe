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

    // Migrate boolean settings
    if let Ok(v) = std::env::var("AI_FORMATTING_ENABLED") {
        settings.ai_formatting_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Ok(v) = std::env::var("CODESCRIBE_BUFFERED_STREAM") {
        settings.buffered_stream = Some(v == "1" || v.eq_ignore_ascii_case("true"));
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
