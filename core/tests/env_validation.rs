use codescribe_core::config::Config;
use serial_test::serial;

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
        if std::env::var("STT_ENDPOINT").is_err() {
            missing.push("STT_ENDPOINT");
        }
        if std::env::var("STT_API_KEY").is_err() {
            missing.push("STT_API_KEY");
        }
    }

    if config.ai_formatting_enabled {
        let has_key = std::env::var("LLM_API_KEY").is_ok()
            || std::env::var("LLM_FORMATTING_API_KEY").is_ok()
            || std::env::var("LLM_ASSISTIVE_API_KEY").is_ok();
        if !has_key {
            missing.push("LLM_API_KEY (or mode-specific)");
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
fn env_precedence_llm_host() {
    let _g1 = EnvGuard::set("LLM_HOST", "http://llm-host");
    let _g2 = EnvGuard::set("OLLAMA_HOST", "http://ollama-host");

    let mut cfg = Config::default();
    cfg.load_from_env();

    assert_eq!(cfg.ollama_host, "http://llm-host");
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
    let _g2 = EnvGuard::unset("LLM_API_KEY");
    let _g3 = EnvGuard::unset("LLM_FORMATTING_API_KEY");
    let _g4 = EnvGuard::unset("LLM_ASSISTIVE_API_KEY");

    let mut cfg = Config::default();
    cfg.load_from_env();

    let missing = missing_required_envs(&cfg);
    assert!(missing.contains(&"LLM_API_KEY (or mode-specific)"));
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
