//! Real single-shot roundtrip through the LIVE agent engine path — the same
//! `create_default_provider()` the Swift chat send uses via the bridge.
//!
//! Ignored by default: it needs a reachable assistive endpoint plus whatever
//! key material that endpoint requires (Keychain, env, or ~/.codescribe/.env).
//! Run with:
//!
//! ```bash
//! cargo test --test e2e_agent_lane_roundtrip -- --ignored --nocapture
//! ```
//!
//! This is the regression net for the "I can't reach the model yet" loop: the
//! lane must resolve from CURRENT settings (lane_truth), a key-optional local
//! endpoint must stream without auth headers, and a configured cloud key must
//! stream with them.

use codescribe::agent::create_default_provider;
use codescribe_core::agent::{AgentEvent, ContentBlock, Message, Role, StreamOptions};

#[tokio::test]
#[ignore]
async fn assistive_lane_answers_one_single_shot_turn() {
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
    assert!(!text.trim().is_empty(), "reply must be non-empty");
    eprintln!("agent replied: {text}");
}
