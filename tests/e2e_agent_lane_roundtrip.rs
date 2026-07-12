//! Deterministic single-shot roundtrip through the real agent engine path — the
//! same `create_default_provider()` the Swift chat send uses via the bridge.
//!
//! The test owns an isolated config directory and a local Responses endpoint,
//! so it runs in the ordinary workspace gate without a real API key. Run with:
//!
//! ```bash
//! cargo test --test e2e_agent_lane_roundtrip -- --nocapture
//! ```
//!
//! This is the regression net for the "I can't reach the model yet" loop: the
//! lane must resolve from CURRENT settings (lane_truth), a key-optional local
//! endpoint must stream without auth headers, and the turn must finish cleanly.

use codescribe::agent::create_default_provider;
use codescribe_core::agent::{AgentEvent, ContentBlock, Message, Role, StreamOptions};
use mockito::Matcher;
use serial_test::serial;
use tempfile::TempDir;

#[tokio::test]
#[serial]
async fn assistive_lane_answers_one_single_shot_turn() {
    let data_dir = TempDir::new().expect("isolated Codescribe data directory");
    let mut server = mockito::Server::new_async().await;
    let endpoint = format!("{}/v1/responses", server.url());
    let response_body = [
        r#"data: {"type":"response.created","response":{"id":"resp_fixture"}}"#,
        "",
        r#"data: {"type":"response.output_text.delta","delta":"pong"}"#,
        "",
        r#"data: {"type":"response.output_text.done","text":"pong"}"#,
        "",
        r#"data: {"type":"response.completed","response":{"id":"resp_fixture","status":"completed"}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");
    let mock = server
        .mock("POST", "/v1/responses")
        .match_header("authorization", Matcher::Missing)
        .match_header("x-api-key", Matcher::Missing)
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("fixture-model".to_string()),
            Matcher::Regex("Reply with the single word: pong".to_string()),
        ]))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(response_body)
        .expect(1)
        .create_async()
        .await;

    let _data_dir = EnvGuard::set(
        "CODESCRIBE_DATA_DIR",
        data_dir.path().to_string_lossy().as_ref(),
    );
    let _disable_keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    let _provider = EnvGuard::set("LLM_ASSISTIVE_PROVIDER", "openai-responses");
    let _endpoint = EnvGuard::set("LLM_ASSISTIVE_ENDPOINT", &endpoint);
    let _model = EnvGuard::set("LLM_ASSISTIVE_MODEL", "fixture-model");
    let _api_key = EnvGuard::remove("LLM_ASSISTIVE_API_KEY");
    let _attempt_timeout = EnvGuard::set("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
    let _chunk_timeout = EnvGuard::set("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS", "2000");

    let provider = create_default_provider()
        .expect("assistive lane must be available (see the reported reason)");

    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::Text(
            "Reply with the single word: pong".to_string(),
        )],
    )];
    let options = StreamOptions {
        model: String::new(),
        system_prompt: None,
        max_tokens: Some(32),
        temperature: None,
        reset_chain: false,
    };

    let mut rx = provider
        .stream(&messages, &[], &options)
        .await
        .expect("stream must start");

    let mut text = String::new();
    let mut clean_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta(delta) => text.push_str(&delta),
            AgentEvent::TextDone(done) if !done.trim().is_empty() => text = done,
            AgentEvent::ResponseDone { clean, .. } => clean_done = clean,
            AgentEvent::Error(error) => panic!("provider error: {error}"),
            _ => {}
        }
    }

    assert!(clean_done, "turn must end on a clean terminal");
    assert_eq!(text.trim(), "pong");
    mock.assert_async().await;
    eprintln!("agent replied: {text}");
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: this process-environment test is serialized.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: this process-environment test is serialized.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: this process-environment test is serialized.
        unsafe {
            match self.previous.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
