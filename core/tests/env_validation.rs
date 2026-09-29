use codescribe_core::config::{Config, ShortcutBinding, UserSettings, WorkMode};
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests run single-threaded with controlled env usage.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests run single-threaded with controlled env usage.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::set_var(self.key, prev) };
        } else {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

fn missing_required_envs(config: &Config) -> Vec<&'static str> {
    let mut missing = Vec::new();

    if !config.use_local_stt {
        if config
            .stt_file_endpoint
            .as_ref()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            missing.push("STT_FILE_ENDPOINT");
        }
        let endpoint = config
            .stt_file_endpoint
            .as_deref()
            .unwrap_or_default()
            .trim();
        if codescribe_core::stt::tail_provider::stt_auth_mode(endpoint)
            != codescribe_core::stt::tail_provider::SttAuthMode::Unauthenticated
            && config
                .stt_file_api_key
                .as_ref()
                .map(|v| v.trim().is_empty())
                .unwrap_or(true)
        {
            missing.push("STT_FILE_API_KEY");
        }
    }

    if std::env::var("CODESCRIBE_NO_EMBED").is_ok()
        && std::env::var("CODESCRIBE_MODEL_PATH").is_err()
    {
        missing.push("CODESCRIBE_MODEL_PATH");
    }

    missing
}

#[test]
#[serial]
fn env_precedence_stt_file_endpoint() {
    let _g1 = EnvGuard::set("STT_FILE_ENDPOINT", "https://example.com/stt");
    let _g2 = EnvGuard::set("WHISPER_SERVER_URL", "https://legacy.example.com/stt");

    let mut cfg = Config::default();
    cfg.load_from_env();

    assert_eq!(
        cfg.stt_file_endpoint.as_deref(),
        Some("https://example.com/stt")
    );
}

#[test]
#[serial]
fn required_cloud_stt_vars_when_local_disabled() {
    let _alias = EnvGuard::unset("STT_ENDPOINT");
    let _g1 = EnvGuard::set("USE_LOCAL_STT", "0");
    let _g2 = EnvGuard::unset("STT_FILE_ENDPOINT");
    let _g3 = EnvGuard::unset("STT_FILE_API_KEY");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(missing.contains(&"STT_FILE_ENDPOINT"));
    assert!(missing.contains(&"STT_FILE_API_KEY"));
}

#[test]
#[serial]
fn loopback_cloud_stt_does_not_require_api_key() {
    let _g1 = EnvGuard::set("USE_LOCAL_STT", "0");
    let _g2 = EnvGuard::set(
        "STT_FILE_ENDPOINT",
        "http://127.0.0.1:8000/v1/audio/transcriptions",
    );
    let _g3 = EnvGuard::unset("STT_FILE_API_KEY");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(!missing.contains(&"STT_FILE_API_KEY"));
}

#[test]
fn default_env_carries_no_llm_endpoint_or_key_rows() {
    let default_env = include_str!("../config/default_env.txt");

    assert!(
        !default_env.contains("dragon:"),
        "default_env must not ship internal-only hosts"
    );
    for retired in [
        "LLM_ENDPOINT=",
        "LLM_FORMATTING_ENDPOINT=",
        "LLM_ASSISTIVE_ENDPOINT=",
        "LLM_API_KEY=",
        "LLM_FORMATTING_API_KEY=",
        "LLM_ASSISTIVE_API_KEY=",
    ] {
        assert!(
            !default_env.contains(retired),
            "{retired} is provider-owned now and must not be seeded from .env"
        );
    }
}

#[test]
#[serial]
fn required_model_path_when_no_embed() {
    let _g1 = EnvGuard::set("CODESCRIBE_NO_EMBED", "1");
    let _g2 = EnvGuard::unset("CODESCRIBE_MODEL_PATH");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(missing.contains(&"CODESCRIBE_MODEL_PATH"));
}

#[test]
#[serial]
fn mode_binding_contract_uses_settings_when_env_path_is_overridden() {
    let tmp = TempDir::new().expect("tempdir");
    let env_path = tmp.path().join("custom.env");
    fs::write(&env_path, "WHISPER_LANGUAGE=en\n").expect("write env");

    let _g0 = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path().to_string_lossy().as_ref());
    let _g1 = EnvGuard::set("CODESCRIBE_ENV_PATH", env_path.to_string_lossy().as_ref());
    let _g2 = EnvGuard::unset("WHISPER_LANGUAGE");
    let _g3 = EnvGuard::unset("HOLD_MODS");
    let _g4 = EnvGuard::unset("TOGGLE_TRIGGER");

    let cfg = Config::load();
    assert_eq!(cfg.whisper_language.as_str(), "en");

    let settings = UserSettings::load();
    assert_eq!(
        settings.mode_binding_for(WorkMode::Dictation),
        ShortcutBinding::HoldFn
    );
    assert_eq!(
        settings.mode_binding_for(WorkMode::Formatting),
        ShortcutBinding::DoubleLeftOption
    );
}

#[test]
#[serial]
fn env_alias_stt_endpoint_routes_by_scheme_with_warn() {
    let _file = EnvGuard::unset("STT_FILE_ENDPOINT");
    let _live = EnvGuard::unset("STT_LIVE_ENDPOINT");
    for (url, file, live) in [
        (
            "https://example.com/stt",
            Some("https://example.com/stt"),
            None,
        ),
        (
            "wss://api.libraxis.cloud/v1/audio/transcribe",
            Some("https://api.libraxis.cloud/v1/audio/transcriptions"),
            Some("wss://api.libraxis.cloud/v1/audio/transcribe"),
        ),
    ] {
        let _alias = EnvGuard::set("STT_ENDPOINT", url);
        let mut config = Config::default();
        config.load_from_env();
        assert_eq!(config.stt_file_endpoint.as_deref(), file);
        assert_eq!(config.stt_live_endpoint.as_deref(), live);
        let _explicit = EnvGuard::set("STT_FILE_ENDPOINT", "https://other.example/file");
        config.load_from_env();
        assert_eq!(
            config.stt_file_endpoint.as_deref(),
            Some("https://other.example/file")
        );
    }
}

#[test]
#[serial]
fn stt_file_env_override_survives_repeated_load_and_settings_write() {
    let tmp = TempDir::new().unwrap();
    let _root = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path().to_str().unwrap());
    let env_path = tmp.path().join("test.env");
    let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", env_path.to_str().unwrap());
    let _file = EnvGuard::unset("STT_FILE_ENDPOINT");
    let _live = EnvGuard::unset("STT_LIVE_ENDPOINT");
    let _alias = EnvGuard::unset("STT_ENDPOINT");
    let _keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    UserSettings {
        stt_file_endpoint: Some("https://settings.example/stt".into()),
        ..Default::default()
    }
    .save()
    .unwrap();
    fs::write(&env_path, "STT_FILE_ENDPOINT=https://env.example/stt\n").unwrap();
    for _ in 0..2 {
        assert_eq!(
            Config::load().stt_file_endpoint.as_deref(),
            Some("https://env.example/stt")
        );
    }
    let _explicit = EnvGuard::set("STT_FILE_ENDPOINT", "https://process.example/stt");
    assert_eq!(
        Config::load().stt_file_endpoint.as_deref(),
        Some("https://process.example/stt")
    );
    let mut settings = UserSettings::load();
    settings.set_string("STT_FILE_ENDPOINT", "wss://wrong.example/live");
    assert_eq!(
        settings.stt_file_endpoint.as_deref(),
        Some("https://settings.example/stt")
    );
    settings.set_string("STT_FILE_ENDPOINT", "  ");
    assert!(UserSettings::load().stt_file_endpoint.is_none());
    assert!(
        Config::default()
            .save_to_env_many(&[("STT_LIVE_ENDPOINT", "https://wrong.example/file")])
            .is_err()
    );
}
