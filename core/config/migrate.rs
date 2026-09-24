//! One-time import from legacy `.env` installs into tiered config.
//!
//! Moves promoted non-secret settings into `settings.json` and API keys into
//! macOS Keychain. This is an import path, not an ongoing precedence rule.

use super::keychain;
use super::llm_migration::{SpeechV2Legacy, migrate_legacy_llm_lanes};
use super::settings::{FormattingPolicy, UserSettings, parse_agent_workspace_roots};
use std::collections::HashMap;
use tracing::{debug, info};

/// Exact, secret-free source-to-account intent, stable across Settings edits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingEnvKeyImport {
    pub env_path: std::path::PathBuf,
    pub source: String,
    pub target: String,
}

pub(super) fn import_pending_env_key(row: &PendingEnvKeyImport) -> anyhow::Result<()> {
    let values = super::Config::parse_env_file(&row.env_path)?;
    let secret = values
        .get(&row.source)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("pending credential source is absent: {}", row.source))?;
    save_migrated_key(&row.target, secret)
}

/// Prepare the first import once, then settle only its durable credential rows
/// when authorized. Existing settings without pending rows never reimport keys.
/// Return in-memory settings only when their initial persistence fails.
pub fn migrate_if_needed(
    file_env: Option<&HashMap<String, String>>,
    acquire_credentials: bool,
) -> Option<UserSettings> {
    let path = UserSettings::settings_path();
    if path.exists() {
        if acquire_credentials && let Err(error) = UserSettings::settle_pending_env_key_imports() {
            tracing::warn!(%error, "Credential import remains pending");
        }
        return None;
    }

    let Some(file_env) = file_env else {
        debug!("No .env snapshot present, skipping migration");
        return None;
    };
    if file_env.is_empty() {
        debug!("Empty .env snapshot, skipping migration");
        return None;
    }
    let file_env = Some(file_env);

    let mut settings = UserSettings::default();

    // Migrate string settings from current env/config state
    if let Some(v) = migrated_value(file_env, "WHISPER_LANGUAGE") {
        settings.whisper_language = Some(v);
    }
    // LLM lanes: the legacy endpoint/model/provider rows go through the same
    // one-shot migration as a legacy settings.json (vendor by host, Custom row
    // otherwise); the key rows below land directly in the account each lane
    // resolved to.
    let legacy_llm = SpeechV2Legacy {
        llm_endpoint: migrated_value(file_env, "LLM_ENDPOINT"),
        llm_model: migrated_value(file_env, "LLM_MODEL"),
        formatting_endpoint: migrated_value(file_env, "LLM_FORMATTING_ENDPOINT"),
        formatting_model: migrated_value(file_env, "LLM_FORMATTING_MODEL"),
        assistive_endpoint: migrated_value(file_env, "LLM_ASSISTIVE_ENDPOINT"),
        assistive_model: migrated_value(file_env, "LLM_ASSISTIVE_MODEL"),
        assistive_provider: migrated_value(file_env, "LLM_ASSISTIVE_PROVIDER"),
    };
    let legacy_key_targets: Vec<(String, String)> = if legacy_llm.needs_migration() {
        migrate_legacy_llm_lanes(&legacy_llm, &mut settings)
            .into_iter()
            .map(|step| (step.from, step.to))
            .collect()
    } else {
        settings.llm_formatting_model = legacy_llm.formatting_model.clone();
        settings.llm_assistive_model = legacy_llm.assistive_model.clone();
        settings.llm_assistive_provider = legacy_llm.assistive_provider.clone();
        Vec::new()
    };
    if let Some(v) = migrated_value(file_env, "LLM_FORMATTING_PROVIDER") {
        settings.llm_formatting_provider = Some(v);
    }
    if let Some(v) = migrated_value(file_env, "FORMATTING_LEVEL") {
        match FormattingPolicy::parse(&v) {
            Ok(policy) => settings.formatting_level = Some(policy.as_str().to_string()),
            Err(error) => tracing::warn!("Migration: ignored invalid formatting policy: {error}"),
        }
    }
    // Promoted fields (previously .env only)
    if let Some(v) = migrated_value(file_env, "LOCAL_MODEL") {
        settings.local_model = Some(v);
    }
    for (lane, target) in [
        (crate::stt::SttLane::File, &mut settings.stt_file_endpoint),
        (crate::stt::SttLane::Live, &mut settings.stt_live_endpoint),
    ] {
        if let Some(value) = migrated_value(file_env, lane.wire_key()) {
            *target = crate::stt::validate_stt_endpoint(lane, &value).ok();
        }
    }
    if let Some(v) = migrated_value(file_env, "STT_ENDPOINT") {
        super::stt_migration::migrate_legacy_stt_lanes(
            &super::stt_migration::SttV2Legacy::from_endpoint(&v),
            &mut settings,
        );
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
    if let Some(v) = migrated_value(file_env, "WHISPER_CONTEXT_WINDOW_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.whisper_context_window_sec = Some(n);
    }
    if let Some(v) = migrated_value(file_env, "LIGHT_PLUS_SENTENCE_PAUSE_SEC")
        && let Ok(n) = v.parse::<f32>()
    {
        settings.light_plus_sentence_pause_sec = Some(n);
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

    if let Some(value) = migrated_value(file_env, "AGENT_WORKSPACE_ROOTS") {
        let roots = parse_agent_workspace_roots(&value);
        if !roots.is_empty() {
            settings.agent_workspace_roots = Some(roots);
        }
    }
    // Persist exact account mappings before secret acquisition. No secret value
    // enters settings, and custom provider identities are never regenerated.
    let key_rows = keychain::KEYCHAIN_ACCOUNTS
        .iter()
        .map(|account| (*account, account.to_string()))
        .chain(
            legacy_key_targets
                .iter()
                .map(|(from, to)| (from.as_str(), to.clone())),
        );
    let key_rows = key_rows.chain(
        ["STT_FILE_API_KEY", "STT_LIVE_API_KEY"]
            .into_iter()
            .filter(|target| {
                migrated_value(file_env, target).is_none_or(|value| value.trim().is_empty())
            })
            .map(|target| ("STT_API_KEY", target.to_string())),
    );
    let env_path = super::Config::env_path();
    let env_path = if env_path.is_absolute() {
        env_path
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(env_path),
            Err(error) => {
                tracing::warn!(%error, "Cannot resolve credential import source path");
                return Some(settings);
            }
        }
    };
    for (source, target) in key_rows {
        if let Some(secret) = migrated_value(file_env, source)
            && !secret.trim().is_empty()
        {
            settings.pending_env_key_imports.push(PendingEnvKeyImport {
                env_path: env_path.clone(),
                source: source.into(),
                target,
            });
        }
    }

    if let Err(e) = settings.save() {
        tracing::warn!("Migration: failed to save settings.json: {e}");
        return Some(settings);
    }

    if acquire_credentials && let Err(error) = UserSettings::settle_pending_env_key_imports() {
        tracing::warn!(%error, "Credential import remains pending");
    }
    info!("Imported settings and recorded credential migration intent");
    None
}

/// Backfill the workspace roots from the legacy `.env` store even when an
/// unrelated `settings.json` already exists. This targeted migration is needed
/// because older Settings builds wrote only `AGENT_WORKSPACE_ROOTS=.env`; the
/// general migration correctly stops once settings.json exists.
pub fn migrate_agent_workspace_roots_if_needed(file_env: Option<&HashMap<String, String>>) {
    let Some(value) = migrated_value(file_env, "AGENT_WORKSPACE_ROOTS") else {
        return;
    };
    let roots = parse_agent_workspace_roots(&value);
    if roots.is_empty() {
        return;
    }

    let mut settings = UserSettings::load();
    if settings
        .agent_workspace_roots
        .as_ref()
        .is_some_and(|existing| !existing.is_empty())
    {
        return;
    }

    settings.agent_workspace_roots = Some(roots);
    if let Err(error) = settings.save() {
        tracing::warn!("Migration: failed to persist agent workspace roots: {error}");
    } else {
        info!("Migrated AGENT_WORKSPACE_ROOTS into settings.json");
    }
}

/// Read a key from the `.env` **snapshot only**.
///
/// Deliberately never falls back to `std::env`: the process environment holds
/// runtime values from many sources, and persisting those would turn a one-time
/// import of the user's file into an accidental capture of ambient state.
fn migrated_value(file_env: Option<&HashMap<String, String>>, key: &str) -> Option<String> {
    file_env.and_then(|vars| vars.get(key).cloned())
}

/// Write one secret to the Keychain, with a test-only failure injection point.
///
/// A failed write must retain the durable import row for the next authorized
/// load. Synthetic injection proves this without accessing the OS secret store.
fn save_migrated_key(account: &str, secret: &str) -> anyhow::Result<()> {
    #[cfg(test)]
    if test_save_key_failure_account(account) {
        anyhow::bail!("injected save_key failure for {account}");
    }

    keychain::save_key(account, secret)
}

/// Test-only account name that forces `save_migrated_key` to fail.
#[cfg(test)]
static TEST_SAVE_KEY_FAILURE: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

/// Whether `account` is the injected Keychain-save failure target.
#[cfg(test)]
fn test_save_key_failure_account(account: &str) -> bool {
    TEST_SAVE_KEY_FAILURE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map(|guard| guard.as_deref() == Some(account))
        .unwrap_or(false)
}

/// Arm or clear the Keychain-save failure injection for migration retry tests.
#[cfg(test)]
fn set_test_save_key_failure(account: Option<&str>) {
    if let Ok(mut guard) = TEST_SAVE_KEY_FAILURE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = account.map(str::to_owned);
    }
}

/// Migration path tests: snapshot-only import, retry, and completion sentinel.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Set a process env var under the serial-test isolation contract.
    fn set_env_for_test<V: AsRef<std::ffi::OsStr>>(key: &str, value: V) {
        // SAFETY: these tests are marked `serial` and intentionally isolate the
        // process env so `UserSettings::settings_path()` resolves inside the temp dir.
        unsafe { std::env::set_var(key, value) };
    }

    /// Clear a process env var under the serial-test isolation contract.
    fn remove_env_for_test(key: &str) {
        // SAFETY: same invariant as `set_env_for_test` above.
        unsafe { std::env::remove_var(key) };
    }

    /// Point `CODESCRIBE_DATA_DIR` at a temp tree so settings paths never touch home.
    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        set_env_for_test("CODESCRIBE_DATA_DIR", tmp.path());
        tmp
    }

    /// Absent `.env` snapshot must not synthesize `settings.json`.
    #[test]
    #[serial]
    fn migrate_skips_when_env_snapshot_is_absent() {
        let _tmp = setup_isolated_data_dir();

        migrate_if_needed(None, true);

        assert!(
            !UserSettings::settings_path().exists(),
            "missing .env snapshot must not synthesize settings.json"
        );

        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    /// Empty `.env` snapshot is a no-op; completion sentinel stays absent.
    #[test]
    #[serial]
    fn migrate_skips_when_env_snapshot_is_empty() {
        let _tmp = setup_isolated_data_dir();
        let empty = HashMap::new();

        migrate_if_needed(Some(&empty), true);

        assert!(
            !UserSettings::settings_path().exists(),
            "empty .env snapshot must not synthesize settings.json"
        );

        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    /// Ambient process env must not leak into migrated settings when absent from `.env`.
    #[test]
    #[serial]
    fn migrate_does_not_persist_runtime_env_when_env_file_lacks_key() {
        let _tmp = setup_isolated_data_dir();
        let mut file_env = HashMap::new();
        file_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());

        set_env_for_test("AI_FORMATTING_ENABLED", "1");
        migrate_if_needed(Some(&file_env), true);
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

    /// Targeted workspace-roots backfill still runs when settings.json already exists.
    #[test]
    #[serial]
    fn existing_settings_backfills_legacy_workspace_roots_once() {
        let _tmp = setup_isolated_data_dir();
        UserSettings {
            whisper_language: Some("pl".to_string()),
            ..Default::default()
        }
        .save()
        .expect("seed existing settings");
        let mut file_env = HashMap::new();
        file_env.insert(
            "AGENT_WORKSPACE_ROOTS".to_string(),
            "~/Git:/Volumes/work".to_string(),
        );

        migrate_agent_workspace_roots_if_needed(Some(&file_env));

        assert_eq!(
            UserSettings::load().agent_workspace_roots,
            Some(vec!["~/Git".to_string(), "/Volumes/work".to_string()])
        );
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    /// Non-empty settings roots win over a stale `AGENT_WORKSPACE_ROOTS` env value.
    #[test]
    #[serial]
    fn existing_workspace_roots_outrank_legacy_env() {
        let _tmp = setup_isolated_data_dir();
        let expected = vec!["/Volumes/current".to_string()];
        UserSettings {
            agent_workspace_roots: Some(expected.clone()),
            ..Default::default()
        }
        .save()
        .expect("seed current roots");
        let mut file_env = HashMap::new();
        file_env.insert(
            "AGENT_WORKSPACE_ROOTS".to_string(),
            "/Volumes/stale".to_string(),
        );

        migrate_agent_workspace_roots_if_needed(Some(&file_env));

        assert_eq!(UserSettings::load().agent_workspace_roots, Some(expected));
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    /// FORMATTING_LEVEL aliases normalize; correction prompt bytes must stay untouched.
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
            migrate_if_needed(Some(&file_env), true);

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

    /// AUTO_PASTE_ENABLED truthy/falsey strings map into an explicit bool setting.
    #[test]
    #[serial]
    fn auto_paste_env_migration_preserves_explicit_policy() {
        for (input, expected) in [("0", false), ("1", true), ("false", false), ("true", true)] {
            let _tmp = setup_isolated_data_dir();
            let mut file_env = HashMap::new();
            file_env.insert("AUTO_PASTE_ENABLED".to_string(), input.to_string());

            migrate_if_needed(Some(&file_env), true);

            assert_eq!(
                UserSettings::load().auto_paste_enabled,
                Some(expected),
                "input={input}"
            );
        }
    }

    /// Failed Keychain write leaves migration incomplete so the next launch retries.
    #[test]
    #[serial]
    fn migrate_retries_when_keychain_save_fails() {
        let _tmp = setup_isolated_data_dir();
        remove_env_for_test("CODESCRIBE_ENV_PATH");
        let mut file_env = HashMap::new();
        file_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());
        file_env.insert("LLM_OPENAI_API_KEY".to_string(), "retry-secret".to_string());
        std::fs::write(
            super::super::Config::env_path(),
            "WHISPER_LANGUAGE=en\nLLM_OPENAI_API_KEY=retry-secret\n",
        )
        .unwrap();

        set_test_save_key_failure(Some("LLM_OPENAI_API_KEY"));
        remove_env_for_test("LLM_OPENAI_API_KEY");

        migrate_if_needed(Some(&file_env), true);

        assert!(
            !UserSettings::load().pending_env_key_imports.is_empty(),
            "failed keychain save must retain durable import intent"
        );
        assert!(
            std::env::var("LLM_OPENAI_API_KEY").is_err(),
            "injected failure happens before test key persistence"
        );

        set_test_save_key_failure(None);
        let mut edited = UserSettings::load();
        edited.show_dock_icon = Some(true);
        edited.save().unwrap();
        migrate_if_needed(Some(&file_env), true);

        assert!(
            UserSettings::settings_path().exists(),
            "retry after keychain recovery should complete migration"
        );
        let persisted = UserSettings::load();
        assert!(persisted.pending_env_key_imports.is_empty());
        assert_eq!(persisted.show_dock_icon, Some(true));
        assert_eq!(
            std::env::var("LLM_OPENAI_API_KEY").as_deref(),
            Ok("retry-secret"),
            "retry writes the migrated secret"
        );

        remove_env_for_test("LLM_OPENAI_API_KEY");
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    /// `settings.json` presence is the once-only completion sentinel.
    #[test]
    #[serial]
    fn successful_migration_marks_complete_once() {
        let _tmp = setup_isolated_data_dir();
        remove_env_for_test("CODESCRIBE_ENV_PATH");
        let mut first_env = HashMap::new();
        first_env.insert("WHISPER_LANGUAGE".to_string(), "en".to_string());
        first_env.insert("LLM_OPENAI_API_KEY".to_string(), "first-secret".to_string());
        std::fs::write(
            super::super::Config::env_path(),
            "WHISPER_LANGUAGE=en\nLLM_OPENAI_API_KEY=first-secret\n",
        )
        .unwrap();

        migrate_if_needed(Some(&first_env), true);

        let path = UserSettings::settings_path();
        assert!(
            path.exists(),
            "successful migration writes completion sentinel"
        );
        assert_eq!(
            std::env::var("LLM_OPENAI_API_KEY").as_deref(),
            Ok("first-secret")
        );

        let mut second_env = HashMap::new();
        second_env.insert("WHISPER_LANGUAGE".to_string(), "pl".to_string());
        second_env.insert(
            "LLM_OPENAI_API_KEY".to_string(),
            "second-secret".to_string(),
        );

        migrate_if_needed(Some(&second_env), true);

        let persisted = UserSettings::load();
        assert_eq!(
            persisted.whisper_language.as_deref(),
            Some("en"),
            "existing settings.json skips re-migration"
        );
        assert_eq!(
            std::env::var("LLM_OPENAI_API_KEY").as_deref(),
            Ok("first-secret"),
            "existing completion sentinel skips duplicate key migration"
        );

        remove_env_for_test("LLM_OPENAI_API_KEY");
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn deferred_custom_import_keeps_account_and_selected_source_across_settings_edits() {
        let tmp = setup_isolated_data_dir();
        let old_env = std::env::var_os("CODESCRIBE_ENV_PATH");
        let source = tmp.path().join("selected.env");
        let replacement = tmp.path().join("other.env");
        let text = "LLM_FORMATTING_ENDPOINT=https://synthetic.example/v1/chat/completions\nLLM_FORMATTING_API_KEY=synthetic-original-secret\n";
        std::fs::write(&source, text).unwrap();
        std::fs::write(
            &replacement,
            text.replace("synthetic-original-secret", "synthetic-other-secret"),
        )
        .unwrap();
        set_env_for_test("CODESCRIBE_ENV_PATH", &source);
        let vars = super::super::Config::parse_env_file(&source).unwrap();
        let _bundle = super::super::keychain::test_support::install_bundle(&[]);
        {
            let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
            migrate_if_needed(Some(&vars), false);
            assert!(probe.attempts().is_empty());
        }
        let mut settings = UserSettings::load();
        let provider = settings.llm_custom_providers.first().unwrap().clone();
        let rows = settings.pending_env_key_imports.clone();
        assert!(
            rows.iter()
                .any(|row| row.target == provider.key_account() && row.env_path == source)
        );
        settings.show_dock_icon = Some(true);
        settings.save().unwrap();
        let bytes = std::fs::read_to_string(UserSettings::settings_path()).unwrap();
        assert!(!bytes.contains("synthetic-original-secret"));
        set_env_for_test("CODESCRIBE_ENV_PATH", &replacement);
        migrate_if_needed(None, true);
        let loaded = UserSettings::load();
        assert_eq!(loaded.show_dock_icon, Some(true));
        assert_eq!(loaded.llm_custom_providers.first(), Some(&provider));
        assert!(loaded.pending_env_key_imports.is_empty());
        assert_eq!(
            super::super::keychain::cached_runtime_key(&provider.key_account()).as_deref(),
            Some("synthetic-original-secret")
        );
        // Acknowledged imports never revive a subsequently removed credential.
        super::super::keychain::delete_key(&provider.key_account()).unwrap();
        migrate_if_needed(Some(&vars), true);
        assert!(super::super::keychain::cached_runtime_key(&provider.key_account()).is_none());
        match old_env {
            Some(value) => set_env_for_test("CODESCRIBE_ENV_PATH", value),
            None => remove_env_for_test("CODESCRIBE_ENV_PATH"),
        }
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn retired_stt_env_import_fills_missing_lanes_without_overwriting_explicit_lane() {
        let tmp = setup_isolated_data_dir();
        let old_env = std::env::var_os("CODESCRIBE_ENV_PATH");
        let old_file = std::env::var_os("STT_FILE_API_KEY");
        let old_live = std::env::var_os("STT_LIVE_API_KEY");
        let path = tmp.path().join("source.env");
        let text = "STT_API_KEY=synthetic-retired\nSTT_FILE_API_KEY=synthetic-explicit-file\n";
        std::fs::write(&path, text).unwrap();
        set_env_for_test("CODESCRIBE_ENV_PATH", &path);
        let vars = super::super::Config::parse_env_file(&path).unwrap();
        {
            let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
            migrate_if_needed(Some(&vars), false);
            assert!(probe.attempts().is_empty());
        }
        let rows = UserSettings::load().pending_env_key_imports;
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.source == "STT_FILE_API_KEY" && row.target == "STT_FILE_API_KEY")
        );
        assert!(
            rows.iter()
                .any(|row| row.source == "STT_API_KEY" && row.target == "STT_LIVE_API_KEY")
        );
        migrate_if_needed(None, true);
        assert!(UserSettings::load().pending_env_key_imports.is_empty());
        assert_eq!(
            std::env::var("STT_FILE_API_KEY").as_deref(),
            Ok("synthetic-explicit-file")
        );
        assert_eq!(
            std::env::var("STT_LIVE_API_KEY").as_deref(),
            Ok("synthetic-retired")
        );
        for (key, value) in [
            ("CODESCRIBE_ENV_PATH", old_env),
            ("STT_FILE_API_KEY", old_file),
            ("STT_LIVE_API_KEY", old_live),
        ] {
            match value {
                Some(value) => set_env_for_test(key, value),
                None => remove_env_for_test(key),
            }
        }
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }

    #[test]
    #[serial]
    fn retired_stt_env_import_records_both_absent_lane_targets() {
        let tmp = setup_isolated_data_dir();
        let old_env = std::env::var_os("CODESCRIBE_ENV_PATH");
        set_env_for_test("CODESCRIBE_ENV_PATH", tmp.path().join("selected.env"));
        let vars = HashMap::from([("STT_API_KEY".into(), "synthetic-both-lanes".into())]);
        let probe = super::super::keychain::CredentialAcquisitionProbe::forbid();
        migrate_if_needed(Some(&vars), false);
        let settings = UserSettings::load();
        assert_eq!(settings.pending_env_key_imports.len(), 2);
        for target in ["STT_FILE_API_KEY", "STT_LIVE_API_KEY"] {
            assert!(
                settings
                    .pending_env_key_imports
                    .iter()
                    .any(|row| row.source == "STT_API_KEY" && row.target == target)
            );
        }
        assert!(probe.attempts().is_empty());
        match old_env {
            Some(value) => set_env_for_test("CODESCRIBE_ENV_PATH", value),
            None => remove_env_for_test("CODESCRIBE_ENV_PATH"),
        }
        remove_env_for_test("CODESCRIBE_DATA_DIR");
    }
}
