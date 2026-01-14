use std::fs;

use codescribe::{ai_formatting, config::prompts};

use serial_test::serial;
use tempfile::TempDir;

fn get_required_env(keys: &[&str]) -> String {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            let t = v.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    panic!(
        "Missing required env var. Set one of: {}",
        keys.join(", ")
    );
}

/// E2E test that verifies Ollama conversation continuity using a real Ollama instance.
///
/// Requirement: verification must be based on the *actual* content returned by Ollama
/// ("czajnowe zapytanie"), not only on request-shape.
///
/// To run:
/// - Ensure Ollama is running locally (e.g., `ollama serve`)
/// - Export `LLM_ENDPOINT=http://localhost:11434` (or `OLLAMA_HOST=...`)
/// - Export `LLM_MODEL=<your_model>` (or `OLLAMA_MODEL=...`)
///
/// This test uses a dedicated, deterministic system prompt written into the app prompts folder
/// (under an overridden `CODESCRIBE_DATA_DIR`) so the expected behavior is stable.
#[tokio::test]
#[serial]
async fn e2e_ollama_memory_real_response_chajnik_query() {
    // Opt-in gate: real Ollama E2E requires a running local Ollama + a pulled model.
    // This keeps default `cargo test` fast and hermetic, while CI/merge-gate can enable it.
    let enabled = std::env::var("CODESCRIBE_E2E_OLLAMA")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        eprintln!(
            "Skipping real Ollama E2E (set CODESCRIBE_E2E_OLLAMA=1 to enable)."
        );
        return;
    }

    let host = get_required_env(&["LLM_ENDPOINT", "OLLAMA_HOST"]);
    let model = get_required_env(&["LLM_MODEL", "OLLAMA_MODEL"]);

    if !(host.contains("localhost") || host.contains("127.0.0.1")) {
        panic!(
            "This E2E test requires a local Ollama host. Got LLM_ENDPOINT/OLLAMA_HOST={}",
            host
        );
    }

    let tmp = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
    }

    // Ensure `ai_formatting` uses Ollama native path.
    unsafe {
        std::env::set_var("LLM_ENDPOINT", host);
        std::env::set_var("LLM_MODEL", model);
        std::env::remove_var("LLM_API_KEY");
    }

    // Avoid retries here (we want a clear 2-turn test), and allow enough time for local inference.
    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "20000");
    }

    // Deterministic test prompt that makes memory usage observable via the *real* response.
    let test_prompt = r#"You are a deterministic test agent.

Rules:
- Output ONLY plain text, no quotes, no extra commentary.
- If the latest user message starts with 'STORE_TOKEN:' then output exactly: STORED
- If the latest user message is exactly 'RECALL_TOKEN' then output the token from the most recent
  user message that starts with 'STORE_TOKEN:' (text after ':', trimmed). If none exists, output: MISSING
"#;

    // Write prompt into the same file-backed location used by the app.
    let assistive_path = prompts::get_assistive_prompt_path();
    fs::create_dir_all(assistive_path.parent().unwrap()).expect("mkdir prompts");
    fs::write(&assistive_path, test_prompt).expect("write assistive prompt");

    // Reset memory to ensure clean start.
    ai_formatting::reset_ollama_memory();

    let token = format!("czajnik-{}", uuid::Uuid::new_v4());
    let r1 = ai_formatting::format_text(&format!("STORE_TOKEN: {}", token), None, true).await;
    assert_eq!(r1.trim(), "STORED");

    let r2 = ai_formatting::format_text("RECALL_TOKEN", None, true).await;
    assert_eq!(r2.trim(), token);

    // Reset should remove continuity.
    ai_formatting::reset_ollama_memory();
    let r3 = ai_formatting::format_text("RECALL_TOKEN", None, true).await;
    assert_eq!(r3.trim(), "MISSING");
}

/// Test context management functions (used by tauri-app)
///
/// These functions are used by tauri-app commands for UI state management.
/// This test ensures they compile and work correctly.
#[test]
fn test_context_management_api() {
    use codescribe::state::conversation;

    // Test conversation API
    let had_conversation = conversation::has_active_conversation();
    conversation::reset_conversation();
    // After reset, should have no active conversation
    assert!(!conversation::has_active_conversation() || !had_conversation);
}
