use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codescribe::{ai_formatting, state};
use serde_json::Value;
use serial_test::serial;

type Shared<T> = Arc<Mutex<T>>;
type PreviousResponseLog = Vec<Option<String>>;
type SseRequestLog = Vec<(Option<String>, Option<bool>)>;

fn request_json(req: &mockito::Request) -> Value {
    req.utf8_lossy_body()
        .ok()
        .and_then(|body| serde_json::from_str(body.as_ref()).ok())
        .unwrap_or(Value::Null)
}

/// E2E-ish test for the retry loop in `ai_formatting::format_text` when using the
/// Responses API path (`/v1/responses`).
///
/// This validates:
/// - first attempt can fail (HTTP 500)
/// - a single retry is performed
/// - second attempt succeeds and returns formatted output
#[tokio::test]
#[serial]
async fn e2e_retry_on_failure_responses_api() {
    let mut server = mockito::Server::new_async().await;
    let endpoint = format!("{}/v1/responses", server.url());

    state::reset_conversation();

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_RETRY_DELAY_MS", "10");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
        std::env::set_var("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS", "2000");
        std::env::set_var("AI_FORMATTING_ENABLED", "1");
        std::env::set_var("LLM_ENDPOINT", &endpoint);
        std::env::set_var("LLM_FORMATTING_ENDPOINT", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_FORMATTING_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
        std::env::set_var("LLM_FORMATTING_API_KEY", "test-key");
        std::env::set_var("LLM_USE_STREAMING", "0");
    }

    // Mockito matches mocks in declaration order (FIFO).
    let fail_then_retry = server
        .mock("POST", "/v1/responses")
        .with_status(500)
        .with_body("boom")
        .expect(1)
        .create_async()
        .await;

    let success_after_retry = server
        .mock("POST", "/v1/responses")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"resp_test_1","output":[{"type":"message","content":[{"type":"output_text","text":"Hello world."}]}]}"#,
        )
        .expect(1)
        .create_async()
        .await;

    let out = ai_formatting::format_text("hello world", Some("en"), false).await;
    fail_then_retry.assert_async().await;
    success_after_retry.assert_async().await;
    assert_eq!(out.trim(), "Hello world.");

    state::reset_conversation();
}

#[tokio::test]
#[serial]
async fn e2e_retry_on_non_streaming_timeout_keeps_previous_response_id() {
    let mut server = mockito::Server::new_async().await;
    let endpoint = format!("{}/v1/responses", server.url());

    state::reset_conversation();
    state::set_response_id_for_mode(state::AiMode::Formatting, "prev_nonstream_1".to_string());

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_RETRY_DELAY_MS", "750");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "200");
        std::env::set_var("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS", "200");
        std::env::set_var("AI_FORMATTING_ENABLED", "1");
        std::env::set_var("LLM_ENDPOINT", &endpoint);
        std::env::set_var("LLM_FORMATTING_ENDPOINT", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_FORMATTING_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
        std::env::set_var("LLM_FORMATTING_API_KEY", "test-key");
        std::env::set_var("LLM_USE_STREAMING", "0");
    }
    assert_eq!(
        std::env::var("LLM_FORMATTING_ENDPOINT").expect("LLM_FORMATTING_ENDPOINT"),
        endpoint
    );
    assert!(
        ai_formatting::has_api_key(),
        "formatting config should be valid"
    );

    let seen_prev_ids: Shared<PreviousResponseLog> = Arc::new(Mutex::new(Vec::new()));
    let seen_prev_ids_match = Arc::clone(&seen_prev_ids);
    let response_sequence = Arc::new(AtomicUsize::new(0));
    let response_sequence_body = Arc::clone(&response_sequence);

    let _timeout_then_retry = server
        .mock("POST", "/v1/responses")
        .match_request(move |req| {
            let parsed = request_json(req);
            let previous = parsed
                .get("previous_response_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            seen_prev_ids_match
                .lock()
                .expect("seen_prev_ids lock")
                .push(previous.clone());
            previous.as_deref() == Some("prev_nonstream_1")
        })
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let seq = response_sequence_body.fetch_add(1, Ordering::SeqCst);
            if seq == 0 {
                std::thread::sleep(Duration::from_millis(500));
                br#"{"id":"resp_nonstream_slow","output":[{"type":"message","content":[{"type":"output_text","text":"too late"}]}]}"#.to_vec()
            } else {
                br#"{"id":"resp_nonstream_final","output":[{"type":"message","content":[{"type":"output_text","text":"Hello world."}]}]}"#.to_vec()
            }
        })
        .create_async()
        .await;

    let out = ai_formatting::format_text("hello world", Some("en"), false).await;
    let seen = seen_prev_ids.lock().expect("seen_prev_ids lock");
    assert!(
        seen.len() >= 2,
        "Expected retry request sequence, got {:?}",
        *seen
    );
    assert!(
        seen.iter()
            .all(|id| id.as_deref() == Some("prev_nonstream_1")),
        "Expected previous_response_id preserved across retries, got {:?}",
        *seen
    );
    assert!(
        response_sequence.load(Ordering::SeqCst) >= 2,
        "Expected at least two non-streaming attempts"
    );
    assert_eq!(out.trim(), "Hello world.");

    assert_eq!(
        state::get_previous_response_id_for_mode(state::AiMode::Formatting),
        Some("resp_nonstream_final".to_string())
    );

    state::reset_conversation();
}

#[tokio::test]
#[serial]
async fn e2e_retry_on_sse_inter_chunk_timeout_keeps_previous_response_id() {
    let mut server = mockito::Server::new_async().await;
    let endpoint = format!("{}/v1/responses", server.url());

    state::reset_conversation();
    state::set_response_id_for_mode(state::AiMode::Formatting, "prev_sse_1".to_string());

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_RETRY_DELAY_MS", "10");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
        std::env::set_var("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS", "120");
        std::env::set_var("AI_FORMATTING_ENABLED", "1");
        std::env::set_var("LLM_ENDPOINT", &endpoint);
        std::env::set_var("LLM_FORMATTING_ENDPOINT", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_FORMATTING_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
        std::env::set_var("LLM_FORMATTING_API_KEY", "test-key");
        std::env::set_var("LLM_USE_STREAMING", "1");
    }

    let seen_sse_flags: Shared<SseRequestLog> = Arc::new(Mutex::new(Vec::new()));
    let seen_sse_flags_success = Arc::clone(&seen_sse_flags);
    let seen_sse_flags_timeout = Arc::clone(&seen_sse_flags);

    let _stall_then_retry = server
        .mock("POST", "/v1/responses")
        .match_request(move |req| {
            let parsed = request_json(req);
            let previous = parsed
                .get("previous_response_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let stream = parsed.get("stream").and_then(|v| v.as_bool());
            seen_sse_flags_timeout
                .lock()
                .expect("seen_sse_flags lock")
                .push((previous.clone(), stream));
            previous.as_deref() == Some("prev_sse_1") && stream == Some(true)
        })
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_chunked_body(|w| {
            w.write_all(
                br#"data: {"type":"response.output_text.delta","delta":"Hello"}

"#,
            )?;
            w.flush()?;
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        })
        .expect(1)
        .create_async()
        .await;

    let success = server
        .mock("POST", "/v1/responses")
        .match_request(move |req| {
            let parsed = request_json(req);
            let previous = parsed
                .get("previous_response_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let stream = parsed.get("stream").and_then(|v| v.as_bool());
            seen_sse_flags_success
                .lock()
                .expect("seen_sse_flags lock")
                .push((previous.clone(), stream));
            previous.as_deref() == Some("prev_sse_1") && stream == Some(true)
        })
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_chunked_body(|w| {
            w.write_all(
                br#"data: {"type":"response.output_text.delta","delta":"Hello world."}

"#,
            )?;
            w.write_all(
                br#"data: {"type":"response.completed","response":{"id":"resp_sse_final"}}

"#,
            )?;
            w.write_all(b"data: [DONE]\n\n")
        })
        .expect(1)
        .create_async()
        .await;

    let out = ai_formatting::format_text("hello world", Some("en"), false).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    success.assert_async().await;
    assert_eq!(out.trim(), "Hello world.");

    let seen = seen_sse_flags.lock().expect("seen_sse_flags lock");
    assert!(seen.len() >= 2, "Expected 2 requests, got {:?}", *seen);
    assert!(
        seen.iter()
            .all(|(id, stream)| id.as_deref() == Some("prev_sse_1") && *stream == Some(true)),
        "Expected previous_response_id + stream=true on retries, got {:?}",
        *seen
    );

    assert_eq!(
        state::get_previous_response_id_for_mode(state::AiMode::Formatting),
        Some("resp_sse_final".to_string())
    );

    state::reset_conversation();
}
