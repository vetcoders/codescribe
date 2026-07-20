//! One-time import from legacy `.env` installs into tiered config.
//!
//! Moves promoted non-secret settings into `settings.json` and API keys into
//! macOS Keychain. This is an import path, not an ongoing precedence rule.

use super::keychain;
use super::settings::{FormattingPolicy, UserSettings};
use std::collections::HashMap;
use tracing::{debug, info};

/// Runs the one-time migration if `settings.json` does not yet exist.
///
/// 1. Skips if `settings.json` already exists.
/// 2. Reads the existing `.env` contents to build a `UserSettings`.
/// 3. Saves to `settings.json`.
/// 4. Migrates API keys from env to Keychain.
pub fn migrate_if_needed(file_env: Option<&HashMap<String, String>>) {
    let path = UserSettings::settings_path();
    if path.exists() {
        debug!("settings.json already exists, skipping migration");
        return;
    }

    let Some(file_env) = file_env else {
        debug!("No .env snapshot present, skipping migration");
        return;
    };
    if file_env.is_empty() {
        debug!("Empty .env snapshot, skipping migration");
        return;
    }
    let file_env = Some(file_env);

    let mut settings = UserSettings::default();

    // Migrate string settings from current env/config state
    if let Some(v) = migrated_value(file_env, "WHISPER_LANGUAGE") {
        settings.whisper_language = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LLM_ENDPOINT") {
        settings.llm_endpoint = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LLM_MODEL") {
        settings.llm_model = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LLM_ASSISTIVE_ENDPOINT") {
        settings.llm_assistive_endpoint = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LLM_ASSISTIVE_MODEL") {
        settings.llm_assistive_model = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "FORMATTING_LEVEL") {
        match FormattingPolicy::parse(&v) {
            Ok(policy) => settings.formatting_level = Some(policy.as_str().to_string()),
            Err(error) => tracing::warn!("Migration: ignored invalid formatting policy: {error}"),
        }
    }
    // Promoted fields (previously .env only)
    if let Some(v) = migrated_value(file_env, "LLM_FORMATTING_ENDPOINT") {
        settings.llm_formatting_endpoint = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LLM_FORMATTING_MODEL") {
        settings.llm_formatting_model = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "LOCAL_MODEL") {
        settings.local_model = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "STT_ENDPOINT") {
        settings.stt_endpoint = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "TRANSCRIPT_SEND_MODE") {
        settings.transcript_send_mode = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "AUDIO_INPUT_DEVICE") {
        settings.audio_input_device = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "SOUND_NAME") {
        settings.sound_name = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "WHISPER_MODEL") {
        settings.whisper_model = Some(v);
    }

    // Migrate boolean settings
    if let Some(v) = migrated_value(file_env, "AI_FORMATTING_ENABLED") {
        settings.ai_formatting_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "AUTO_PASTE_ENABLED") {
        settings.auto_paste_enabled = Some(matches!(
            v.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "enabled"
        ));
    }
    if let Some(v) = migrated_value(file_env, "BEEP_ON_START") {
        settings.beep_on_start = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "HOLD_EXCLUSIVE") {
        settings.hold_exclusive = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    // Promoted booleans
    if let Some(v) = migrated_value(file_env, "USE_LOCAL_STT") {
        settings.use_local_stt = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "HISTORY_ENABLED") {
        settings.history_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "QUICK_NOTES_ENABLED") {
        settings.quick_notes_enabled = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "QUICK_NOTES_SAVE_ONLY") {
        settings.quick_notes_save_only = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "START_AT_LOGIN") {
        settings.start_at_login = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }
    if let Some(v) = migrated_value(file_env, "AGENT_ENTER_SENDS") {
        settings.agent_enter_sends = Some(v == "1" || v.eq_ignore_ascii_case("true"));
    }

    // Migrate numeric settings
    if let Some(v) = migrated_value(file_env, "HOLD_START_DELAY_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.hold_start_delay_ms = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "SOUND_VOLUME")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.sound_volume = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "TOGGLE_SILENCE_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.toggle_silence_sec = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "DOUBLE_TAP_INTERVAL_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.double_tap_interval_ms = Some(n);
    }
    // Voice Lab survivors
    if let Some(v) = migrated_value(file_env, "CODESCRIBE_BUFFER_DELAY_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.buffer_delay_ms = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "CODESCRIBE_TYPING_CPS")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.typing_cps = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "CODESCRIBE_EMIT_WORDS_MAX")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.emit_words_max = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "CODESCRIBE_BUFFERED_INTERIM_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.buffered_interim_sec = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "BACKEND_MAX_UPLOAD_MB")
        && let Ok(n) = v.parse::<u64>()
    {
        settings.backend_max_upload_mb = Some(n);
    }

    // Migrate API keys to Keychain before writing settings.json. The existence
    // of settings.json is the migration-complete sentinel, so a failed secret
    // write must leave the migration retryable on the next launch.
    for &account in keychain::KEYCHAIN_ACCOUNTS {
        if let Some(secret) = migrated_value(file_env, account)
            && !secret.is_empty()
            && let Err(e) = save_migrated_key(account, &secret)
        {
            tracing::warn!(
                "Migration: failed to save {account} to Keychain; will retry on next launch: {e}"
            );
            return;
        }
    }

    // Save settings.json last because its presence marks the one-time import as complete.
    if let Err(e) = settings.save() {
        tracing::warn!("Migration: failed to save settings.json: {e}");
        return;
    }

    info!("Migrated config to settings.json + Keychain");
}

fn migrated_value(file_env: Option<&HashMap<String, String>>, key: &str) -> Option<String> {
    file_env.and_then(|vars| vars.get(key).cloned())
}

fn save_migrated_key(account: &str, secret: &str) -> anyhow::Result<()> {
    #[cfg(test)]
    if test_save_key_failure_account(account) {
        anyhow::bail!("injected save_key failure for {account}");
    }

    keychain::save_key(account, secret)
}

#[cfg(test)]
static TEST_SAVE_KEY_FAILURE: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn test_save_key_failure_account(account: &str) -> bool {
    TEST_SAVE_KEY_FAILURE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map(|guard| guard.as_deref() == Some(account))
        .unwrap_or(false)
}

#[cfg(test)]
fn set_test_save_key_failure(account: Option<&str>) {
    if let Ok(mut guard) = TEST_SAVE_KEY_FAILURE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = account.map(str::to_owned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn set_env_for_test<V: AsRef<std::ffi::OsStr>>(key: &str, value: V) {
        // SAFETY: these tests are marked `serial` and intentionally isolate the
        // process env so `UserSettings::settings_path()` resolves inside the temp dir.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_for_test(key: &str) {
        // SAFETY: same invariant as `set_env_for_test` above.
        unsafe { std::env::remove_var(key) };
    }

    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        set_env_for_test("CODESCRIBE_DATA_DIR", tmp.path());
        tmp
    }

    #[test]
    #[serial]
    fn migrate_skips_when_env_snapshot_is_absent() {
        let _tmp = setup_isolated_data_dir();

        migrate_if_needed(None);

        assert!(
            !UserSettings::settings_path().exists(),
            "missing .env snapshot must not synthesize settings.json"
        );

        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn migrate_skips_when_env_snapshot_is_empty() {
        let _tmp = setup_isolated_data_dir();
        let empty = HashMap::new();

        migrate_if_needed(Some(&empty));

        assert!(
            !UserSettings::settings_path().exists(),
            "empty .env snapshot must not synthesize settings.json"
        );

        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn migrate_does_not_persist_runtime_env_when_env_file_lacks_key() {
        let _tmp = setup_isolated_data_dir();
        let mut file_env = HashMap::new();
        file_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());

        set_env_for_test("AI_FORMATTING_ENABLED", "1");
        migrate_if_needed(Some(&file_env));
        remove_env_for_test("AI_FORMATTING_ENABLED");

        let path = UserSettings::settings_path();
        assert!(path.exists(), "non-empty .env snapshot triggers migration");

        let persisted = UserSettings::load();
        assert_eq!(
            persisted.whisper_language.as_deref(),
            Some("en"),
            ".env-supplied promoted key migrates"
        );
        assert_eq!(
            persisted.ai_formatting_enabled, None,
            "runtime env value must not leak into migrated settings.json"
        );

        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn formatting_policy_env_migration_normalizes_aliases_and_preserves_correction_digest() {
        use sha2::{Digest, Sha256};

        let cases = [
            ("off", Some("off")),
            ("correction", Some("correction")),
            ("smart", Some("smart")),
            ("max", Some("max")),
            ("raw", Some("off")),
            ("medium", Some("correction")),
            ("creative", Some("max")),
            ("aggressive", None),
        ];

        for (input, expected) in cases {
            let _tmp = setup_isolated_data_dir();
            let correction_path = crate::config::prompts::get_formatting_prompt_path();
            std::fs::create_dir_all(correction_path.parent().expect("prompt parent"))
                .expect("create prompt directory");
            let original = b"existing correction bytes\n\0tail\n";
            std::fs::write(&correction_path, original).expect("seed correction prompt");
            let before = format!("{:x}", Sha256::digest(original));

            let mut file_env = HashMap::new();
            file_env.insert("FORMATTING_LEVEL".to_string(), input.to_string());
            migrate_if_needed(Some(&file_env));

            assert_eq!(UserSettings::load().formatting_level.as_deref(), expected);
            for _probe in 0..3 {
                let snapshot = crate::config::prompts::prompt_snapshot(
                    crate::config::prompts::PromptKind::Formatting,
                );
                assert_eq!(snapshot.content.as_bytes(), original);
            }
            let after = format!(
                "{:x}",
                Sha256::digest(std::fs::read(&correction_path).expect("read correction prompt"))
            );
            assert_eq!(
                after, before,
                "migration changed formatting.txt for {input}"
            );
        }
    }

    #[test]
    #[serial]
    fn auto_paste_env_migration_preserves_explicit_policy() {
        for (input, expected) in [("0", false), ("1", true), ("false", false), ("true", true)] {
            let _tmp = setup_isolated_data_dir();
            let mut file_env = HashMap::new();
            file_env.insert("AUTO_PASTE_ENABLED".to_string(), input.to_string());

            migrate_if_needed(Some(&file_env));

            assert_eq!(
                UserSettings::load().auto_paste_enabled,
                Some(expected),
                "input={input}"
            );
        }
    }

    #[test]
    #[serial]
    fn migrate_retries_when_keychain_save_fails() {
        let _tmp = setup_isolated_data_dir();
        let mut file_env = HashMap::new();
        file_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());
        file_env.insert("LLM_API_KEY".to_string(), "retry-secret".to_string());

        set_test_save_key_failure(Some("LLM_API_KEY"));
        remove_env_for_test("LLM_API_KEY");

        migrate_if_needed(Some(&file_env));

        assert!(
            !UserSettings::settings_path().exists(),
            "failed keychain save must not mark migration complete"
        );
        assert!(
            std::env::var("LLM_API_KEY").is_err(),
            "injected failure happens before test key persistence"
        );

        set_test_save_key_failure(None);
        migrate_if_needed(Some(&file_env));

        assert!(
            UserSettings::settings_path().exists(),
            "retry after keychain recovery should complete migration"
        );
        assert_eq!(
            std::env::var("LLM_API_KEY").as_deref(),
            Ok("retry-secret"),
            "retry writes the migrated secret"
        );

        remove_env_for_test("LLM_API_KEY");
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn successful_migration_marks_complete_once() {
        let _tmp = setup_isolated_data_dir();
        let mut first_env = HashMap::new();
        first_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());
        first_env.insert("LLM_API_KEY".to_string(), "first-secret".to_string());

        migrate_if_needed(Some(&first_env));

        let path = UserSettings::settings_path();
        assert!(
            path.exists(),
            "successful migration writes completion sentinel"
        );
        assert_eq!(std::env::var("LLM_API_KEY").as_deref(), Ok("first-secret"));

        let mut second_env = HashMap::new();
        second_env.insert("WHISPER_LANGUAGE".to_string(), "pl".to_string());
        second_env.insert("LLM_API_KEY".to_string(), "second-secret".to_string());

        migrate_if_needed(Some(&second_env));

        let persisted = UserSettings::load();
        assert_eq!(
            persisted.whisper_language.as_deref(),
            Some("en"),
            "existing settings.json skips re-migration"
        );
        assert_eq!(
            std::env::var("LLM_API_KEY").as_deref(),
            Ok("first-secret"),
            "existing completion sentinel skips duplicate key migration"
        );

        remove_env_for_test("LLM_API_KEY");
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }
}
