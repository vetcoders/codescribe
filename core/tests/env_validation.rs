use codescribe_core::config::Config;
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
            .stt_endpoint
            .as_ref()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            missing.push("STT_ENDPOINT");
        }
        if config
            .stt_api_key
            .as_ref()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            missing.push("STT_API_KEY");
        }
    }

    if config.ai_formatting_enabled {
        let has_key = std::env::var("LLM_FORMATTING_API_KEY")
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        if !has_key {
            missing.push("LLM_FORMATTING_API_KEY");
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
fn env_precedence_stt_endpoint() {
    let _g1 = EnvGuard::set("STT_ENDPOINT", "https://example.com/stt");
    let _g2 = EnvGuard::set("WHISPER_SERVER_URL", "https://legacy.example.com/stt");

    let mut cfg = Config::default();
    cfg.load_from_env();

    assert_eq!(cfg.stt_endpoint.as_deref(), Some("https://example.com/stt"));
}

#[test]
#[serial]
fn env_ignores_legacy_llm_host() {
    let _g1 = EnvGuard::set("LLM_HOST", "http://llm-host");
    let _g2 = EnvGuard::set("OLLAMA_HOST", "http://ollama-host");
    let _g3 = EnvGuard::unset("LLM_ENDPOINT");

    let mut cfg = Config::default();
    cfg.load_from_env();

    assert!(cfg.llm_endpoint.is_none());
}

#[test]
#[serial]
fn required_cloud_stt_vars_when_local_disabled() {
    let _g1 = EnvGuard::set("USE_LOCAL_STT", "0");
    let _g2 = EnvGuard::unset("STT_ENDPOINT");
    let _g3 = EnvGuard::unset("STT_API_KEY");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(missing.contains(&"STT_ENDPOINT"));
    assert!(missing.contains(&"STT_API_KEY"));
}

#[test]
#[serial]
fn required_llm_key_when_ai_enabled() {
    let _g1 = EnvGuard::set("AI_FORMATTING_ENABLED", "1");
    let _g2 = EnvGuard::unset("LLM_FORMATTING_API_KEY");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(missing.contains(&"LLM_FORMATTING_API_KEY"));
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
fn env_path_override_is_respected() {
    let tmp = TempDir::new().expect("tempdir");
    let env_path = tmp.path().join("custom.env");
    fs::write(&env_path, "HOLD_MODS=ctrl_alt\n").expect("write env");

    let _g0 = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path().to_string_lossy().as_ref());
    let _g1 = EnvGuard::set("CODESCRIBE_ENV_PATH", env_path.to_string_lossy().as_ref());
    let _g2 = EnvGuard::unset("HOLD_MODS");

    let cfg = Config::load();
    assert_eq!(cfg.hold_mods.as_str(), "ctrl_alt");
}

// NOTE: Legacy key migration test removed - no users have legacy keys yet.
// If we ever need migration, we'll add proper tests then.
