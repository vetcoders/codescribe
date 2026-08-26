use codescribe_core::ai_formatting::{AiFormatStatus, format_text_with_status_for_policy};
use codescribe_core::config::{
    Config, FormattingPolicy, PromptKind, PromptWriteReason, prompt_snapshot, prompts, write_prompt,
};
use serial_test::serial;
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: these tests are serialized and restore every process variable.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: these tests are serialized and restore every process variable.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: restores the serialized test's prior process environment.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Registry defaults are sealed by the loader when all four overlays are
/// absent. Formatter and Agent execution only borrow these selected facts.
#[test]
#[serial]
fn runtime_ai_execution_uses_registered_defaults() {
    let _max_retries = EnvGuard::unset("CODESCRIBE_AI_MAX_RETRIES");
    let _retry_delay = EnvGuard::unset("CODESCRIBE_AI_RETRY_DELAY_MS");
    let _attempt_timeout = EnvGuard::unset("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS");
    let _inter_chunk_timeout = EnvGuard::unset("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS");
    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    let formatter = runtime_settings.ai_execution().formatter();
    let timing = runtime_settings.ai_execution().request_timing();
    assert_eq!(formatter.max_retries(), 3);
    assert_eq!(formatter.retry_delay(), Duration::from_millis(2_000));
    assert_eq!(timing.attempt_timeout(), Duration::from_millis(30_000));
    assert_eq!(timing.inter_chunk_timeout(), Duration::from_millis(30_000));
}

#[test]
#[serial]
fn formatting_policy_selects_exact_prompt() {
    let sandbox = tempfile::TempDir::new().expect("isolated prompt data");
    let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", sandbox.path());
    let fixtures = [
        (
            FormattingPolicy::Correction,
            PromptKind::Formatting,
            "correction fixture",
        ),
        (
            FormattingPolicy::Smart,
            PromptKind::FormattingSmart,
            "smart fixture",
        ),
        (
            FormattingPolicy::Max,
            PromptKind::FormattingMax,
            "max fixture",
        ),
    ];

    for (_, kind, content) in fixtures {
        write_prompt(kind, content, PromptWriteReason::SettingsSave).expect("seed prompt fixture");
    }
    std::fs::write(
        prompts::prompts_dir().join("formatting_tuning.txt"),
        "shared tuning fixture",
    )
    .expect("seed common tuning");

    for (policy, _, content) in fixtures {
        let _runtime_policy = EnvGuard::set("FORMATTING_LEVEL", policy.as_str());
        let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
        let expected = format!("{content}\n\nshared tuning fixture");
        assert_eq!(
            runtime_settings
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .map(|prompt| prompt.composed_content()),
            Some(expected.as_str()),
            "provider seam selected the wrong prompt for {policy:?}"
        );
    }
    let _runtime_policy = EnvGuard::set("FORMATTING_LEVEL", FormattingPolicy::Off.as_str());
    let runtime_settings = Config::load_runtime_snapshot().expect("seal Off runtime settings");
    assert_eq!(
        runtime_settings
            .ai_execution()
            .formatter()
            .formatting_prompt(),
        None
    );
}

#[test]
#[serial]
fn formatting_policy_walkaround_receipt() {
    let external_root = std::env::var_os("CODESCRIBE_WALKAROUND_DIR").map(PathBuf::from);
    let sandbox = external_root
        .is_none()
        .then(|| tempfile::TempDir::new().expect("isolated walkaround data"));
    let root = external_root
        .as_deref()
        .or_else(|| sandbox.as_ref().map(|dir| dir.path()))
        .expect("walkaround root");
    std::fs::create_dir_all(root).expect("create walkaround root");

    let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", root);
    let _runtime_policy = EnvGuard::unset("FORMATTING_LEVEL");
    let fixtures = [
        (
            FormattingPolicy::Correction,
            PromptKind::Formatting,
            "walkaround correction fixture 0718",
        ),
        (
            FormattingPolicy::Smart,
            PromptKind::FormattingSmart,
            "walkaround smart fixture 0718",
        ),
        (
            FormattingPolicy::Max,
            PromptKind::FormattingMax,
            "walkaround max fixture 0718",
        ),
    ];
    for (_, kind, content) in fixtures {
        write_prompt(kind, content, PromptWriteReason::SettingsSave).expect("seed prompt fixture");
    }

    let config = Config::default();
    config
        .save_to_env("FORMATTING_LEVEL", FormattingPolicy::Off.as_str())
        .expect("persist Off");
    assert_eq!(
        Config::formatting_policy().expect("resolve Off"),
        FormattingPolicy::Off
    );
    let runtime_settings = Config::load_runtime_snapshot().expect("seal Off runtime settings");
    assert_eq!(
        runtime_settings
            .ai_execution()
            .formatter()
            .formatting_prompt(),
        None
    );
    println!(
        "{}",
        serde_json::json!({
            "level": "off",
            "provider_bypassed": true,
            "selected_prompt_sha256": serde_json::Value::Null,
        })
    );

    for (policy, kind, _) in fixtures {
        config
            .save_to_env("FORMATTING_LEVEL", policy.as_str())
            .expect("persist normalized policy");
        assert_eq!(Config::formatting_policy().expect("resolve policy"), policy);

        let snapshot = prompt_snapshot(kind);
        let runtime_settings =
            Config::load_runtime_snapshot().expect("seal selected runtime settings");
        let selected = runtime_settings
            .ai_execution()
            .formatter()
            .formatting_prompt()
            .expect("enabled policy selects provider prompt")
            .composed_content();
        let prompt_digest = format!("{:x}", Sha256::digest(snapshot.content.as_bytes()));
        let selected_digest = format!("{:x}", Sha256::digest(selected.as_bytes()));
        assert_eq!(selected_digest, prompt_digest);
        println!(
            "{}",
            serde_json::json!({
                "level": policy.as_str(),
                "path": snapshot.path,
                "source": snapshot.source.as_str(),
                "prompt_sha256": prompt_digest,
                "selected_prompt_sha256": selected_digest,
            })
        );
    }
}

#[tokio::test]
#[serial]
async fn formatting_off_bypasses_llm() {
    let mut server = mockito::Server::new_async().await;
    let provider = server
        .mock("POST", "/v1/responses")
        .expect(0)
        .create_async()
        .await;
    let _endpoint = EnvGuard::set("LLM_FORMATTING_ENDPOINT", server.url());
    let _model = EnvGuard::set("LLM_FORMATTING_MODEL", "test-model");
    let _key = EnvGuard::set("LLM_FORMATTING_API_KEY", "test-key");
    let _policy = EnvGuard::set("FORMATTING_LEVEL", FormattingPolicy::Off.as_str());
    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");

    let input = "This transcript is intentionally long enough to reach the provider path.";
    let result = format_text_with_status_for_policy(input, Some("en"), &runtime_settings).await;

    assert_eq!(result.text, input);
    assert_eq!(result.status, AiFormatStatus::Skipped);
    provider.assert_async().await;
}
