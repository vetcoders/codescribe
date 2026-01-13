use codescribe::ai_formatting;

use mockito::Matcher;
use serial_test::serial;

/// E2E-ish test for the retry loop in `ai_formatting::format_text` when using the
/// Responses API path (`/v1/responses`).
///
/// This validates:
/// - first attempt can fail (HTTP 500)
/// - a single retry is performed
/// - retry request includes strengthened instructions (contains `CRITICAL`)
/// - second attempt succeeds and returns formatted output
#[tokio::test]
#[serial]
async fn e2e_retry_on_failure_responses_api() {
    let mut server = mockito::Server::new();
    let endpoint = format!("{}/v1/responses", server.url());

    // Speed up test execution (production defaults remain 5s/2.5s).
    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_RETRY_DELAY_MS", "10");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "500");

        std::env::set_var("LLM_HOST", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
    }

    // 1) First attempt: any request body that does NOT contain CRITICAL => fail.
    let m1 = server
        .mock("POST", "/v1/responses")
        .match_body(Matcher::Regex(r"(?s)^(?!.*CRITICAL).*$".to_string()))
        .with_status(500)
        .with_body("boom")
        .create();

    // 2) Retry attempt: request body contains CRITICAL => succeed.
    let m2 = server
        .mock("POST", "/v1/responses")
        .match_body(Matcher::Regex(r"CRITICAL".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"resp_test_1","output":[{"type":"message","content":[{"type":"output_text","text":"Hello world."}]}]}"#,
        )
        .create();

    let out = ai_formatting::format_text("hello world", Some("en"), false).await;
    assert_eq!(out.trim(), "Hello world.");

    m1.assert();
    m2.assert();
}
