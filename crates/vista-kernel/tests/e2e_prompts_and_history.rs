use std::fs;

use codescribe::{ai_formatting, config::prompts, state::history};

use mockito::Matcher;
use serial_test::serial;
use tempfile::TempDir;

#[test]
#[serial]
fn e2e_prompts_are_file_backed_and_history_uses_config_dir() {
    let tmp = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
    }

    // --- Prompts: load-or-create ---
    let formatting = prompts::get_formatting_prompt();
    assert!(formatting.contains("TRANSCRIPTION FORMATTER"));

    let assistive = prompts::get_assistive_prompt();
    assert!(assistive.contains("Jesteś asystentem tekstowym"));

    // Files should exist under CODESCRIBE_DATA_DIR/prompts/...
    let formatting_path = prompts::get_formatting_prompt_path();
    let assistive_path = prompts::get_assistive_prompt_path();
    // Canonicalize tmp.path() to handle macOS /var → /private/var symlink
    let tmp_canon = tmp
        .path()
        .canonicalize()
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    assert!(formatting_path.starts_with(&tmp_canon));
    assert!(assistive_path.starts_with(&tmp_canon));
    assert!(formatting_path.exists());
    assert!(assistive_path.exists());

    // Overwrite formatting prompt and re-load
    fs::write(&formatting_path, "CUSTOM_FORMATTING_PROMPT").expect("write prompt");
    assert_eq!(
        prompts::get_formatting_prompt().trim(),
        "CUSTOM_FORMATTING_PROMPT"
    );

    // Reset to defaults
    prompts::reset_to_defaults().expect("reset prompts");
    assert!(prompts::get_formatting_prompt().contains("TRANSCRIPTION FORMATTER"));

    // --- History: should respect config_dir override ---
    let e1 = history::save_entry("raw one two");
    assert!(e1.path.starts_with(&tmp_canon));
    assert!(
        fs::read_to_string(&e1.path)
            .unwrap()
            .contains("raw one two")
    );

    // Mimic tray behavior: read last, format, save as new entry
    let mut server = mockito::Server::new();
    let endpoint = format!("{}/v1/responses", server.url());

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
        // Keep some slack for local CI variability.
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
        std::env::set_var("LLM_ENDPOINT", &endpoint);
        std::env::set_var("LLM_FORMATTING_ENDPOINT", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_FORMATTING_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
        std::env::set_var("LLM_FORMATTING_API_KEY", "test-key");
    }

    let m = server
        .mock("POST", "/v1/responses")
        .match_body(Matcher::Regex(r"raw one two".to_string()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(
            r#"data: {"type":"response.output_text.delta","delta":"RAW ONE TWO."}

data: {"type":"response.completed","response":{"id":"resp_test_2"}}

data: [DONE]

"#,
        )
        .create();

    let last = history::latest_entry().expect("latest_entry");
    let raw = fs::read_to_string(last.path).expect("read last");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let formatted = rt.block_on(ai_formatting::format_text(&raw, None, false));

    // Ensure the LLM endpoint was actually called (i.e., we didn't silently fall back).
    m.assert();
    assert_eq!(formatted.trim(), "RAW ONE TWO.");
    let e2 = history::save_entry(&formatted);

    // New entry created, raw entry kept.
    assert_ne!(e1.path, e2.path);
    assert!(
        fs::read_to_string(&e1.path)
            .unwrap()
            .contains("raw one two")
    );
    assert!(
        fs::read_to_string(&e2.path)
            .unwrap()
            .contains("RAW ONE TWO.")
    );
}
