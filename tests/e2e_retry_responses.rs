use codescribe::ai_formatting;

use serial_test::serial;

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

    // Speed up test execution (production defaults remain 5s/2.5s).
    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_RETRY_DELAY_MS", "10");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");

        // Ensure AI formatting is enabled (other tests may have disabled it)
        std::env::set_var("AI_FORMATTING_ENABLED", "1");

        std::env::set_var("LLM_ENDPOINT", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
        // Mock returns plain JSON, not SSE — use sync mode
        std::env::set_var("LLM_USE_STREAMING", "0");
    }

    // First attempt fails with 500, second succeeds.
    // Mockito serves mocks in LIFO order, so we create success mock first.
    let _m2 = server
        .mock("POST", "/v1/responses")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"resp_test_1","output":[{"type":"message","content":[{"type":"output_text","text":"Hello world."}]}]}"#,
        )
        .create_async()
        .await;

    let _m1 = server
        .mock("POST", "/v1/responses")
        .with_status(500)
        .with_body("boom")
        .create_async()
        .await;

    let out = ai_formatting::format_text("hello world", Some("en"), false).await;
    assert_eq!(out.trim(), "Hello world.");
}
