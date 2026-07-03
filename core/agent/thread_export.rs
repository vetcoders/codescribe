//! Render a persisted agent `Thread` to a Markdown transcript.
//!
//! Pure formatting — no filesystem. The bridge (`bridge/src/threads.rs`) owns
//! saving the returned string under `~/.codescribe/transcriptions/YYYY-MM-DD/`.
//! The shape mirrors the legacy voice-chat `chat_markdown_from_messages`
//! (`app/ui/voice_chat/api/export.rs`, removed in 37efe51): a thread heading,
//! export metadata, then one `## Role · timestamp` section per turn.

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

use super::thread_store::Thread;

/// Build a Markdown transcript for `thread`.
///
/// `assistant_only = true` keeps only assistant turns (mirrors the legacy
/// "assistant replies only" export variant); otherwise every turn carrying
/// visible text is included. Turns whose content flattens to nothing (e.g.
/// pure tool-use / image payloads) are skipped either way.
pub fn thread_to_markdown(thread: &Thread, assistant_only: bool) -> String {
    let exported_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let title = thread.title.trim();
    let heading = if title.is_empty() {
        "Untitled thread"
    } else {
        title
    };

    let mut out = String::new();
    out.push_str(&format!("# {heading}\n\n"));
    out.push_str(&format!("- exported_at: {exported_at}\n"));
    out.push_str(&format!(
        "- scope: {}\n\n",
        if assistant_only {
            "assistant_only"
        } else {
            "all"
        }
    ));

    for message in &thread.messages {
        if assistant_only && !is_assistant(&message.role) {
            continue;
        }
        let text = flatten_content(&message.content);
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!(
            "## {} · {}\n\n",
            role_label(&message.role),
            local_timestamp(message.timestamp)
        ));
        out.push_str(text.trim_end());
        out.push_str("\n\n");
    }

    out.trim_end().to_string() + "\n"
}

fn is_assistant(role: &str) -> bool {
    role.eq_ignore_ascii_case("assistant")
}

/// Human-readable section label for a message role. Known roles get a fixed
/// capitalization; anything else is capitalized best-effort.
fn role_label(role: &str) -> String {
    match role.to_ascii_lowercase().as_str() {
        "user" => "User".to_string(),
        "assistant" => "Assistant".to_string(),
        "system" => "System".to_string(),
        "tool" => "Tool".to_string(),
        "" => "Message".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => other.to_string(),
            }
        }
    }
}

fn local_timestamp(ts: DateTime<Utc>) -> String {
    ts.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Flatten a message's structured content into a display string. Type-aware (see
/// `collect_text`) so it yields clean prose without leaking structural field
/// values. Interior whitespace — newlines, fenced code blocks, list breaks — is
/// preserved so the exported Markdown keeps its structure; only each block's
/// outer edges are trimmed and distinct blocks are separated by a blank line
/// (mirrors `thread_store` / bridge `threads::flatten_message_text`). The
/// previous `split_whitespace().join(" ")` collapsed every newline and code
/// fence into one paragraph.
fn flatten_content(content: &[Value]) -> String {
    let mut chunks = Vec::new();
    for value in content {
        collect_text(value, &mut chunks);
    }
    chunks
        .iter()
        .map(|chunk| chunk.trim())
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Collect human-readable prose from a stored content `Value`, keyed on the
/// canonical `content_block_to_value` shapes (`thread_store.rs`): `text` blocks
/// contribute their text, `tool_result` recurses into its nested content, and
/// `tool_use` / `image` / `image_asset` blocks carry no display prose (skipped).
/// A field-only extractor (as opposed to a blind key walk) avoids emitting
/// structural values like the literal `"text"` / `"tool_use"` type tags.
fn collect_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            out.push(text.trim().to_string());
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        Value::Object(map) => match map.get("type").and_then(Value::as_str) {
            Some("text") | None => {
                if let Some(text) = map.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    out.push(text.trim().to_string());
                }
            }
            Some("tool_result") => {
                if let Some(nested) = map.get("content") {
                    collect_text(nested, out);
                }
            }
            Some(_) => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn message(role: &str, text: &str) -> super::super::thread_store::ThreadMessage {
        super::super::thread_store::ThreadMessage {
            role: role.to_string(),
            content: vec![json!({ "type": "text", "text": text })],
            timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 30, 0).unwrap(),
            metadata: None,
        }
    }

    fn thread(messages: Vec<super::super::thread_store::ThreadMessage>) -> Thread {
        Thread {
            id: "t_test".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 7, 2, 9, 30, 0).unwrap(),
            title: "auth-refactor".to_string(),
            title_is_custom: true,
            mode: "assistive".to_string(),
            tags: vec![],
            notes: vec![],
            messages,
            summary: None,
            total_tokens: None,
            provider: "anthropic".to_string(),
            model: "claude".to_string(),
        }
    }

    #[test]
    fn renders_heading_scope_roles_and_body() {
        let md = thread_to_markdown(
            &thread(vec![
                message("user", "where do we double-dispatch events?"),
                message("assistant", "Two spots: bus.ts and store.ts."),
            ]),
            false,
        );
        assert!(
            md.starts_with("# auth-refactor\n"),
            "heading is the thread title:\n{md}"
        );
        assert!(md.contains("- scope: all\n"));
        assert!(md.contains("## User · "));
        assert!(md.contains("## Assistant · "));
        assert!(md.contains("where do we double-dispatch events?"));
        assert!(md.contains("Two spots: bus.ts and store.ts."));
        assert!(md.ends_with('\n'));
    }

    #[test]
    fn assistant_only_drops_non_assistant_turns() {
        let md = thread_to_markdown(
            &thread(vec![
                message("user", "please summarize"),
                message("assistant", "Done — here is the summary."),
            ]),
            true,
        );
        assert!(md.contains("- scope: assistant_only\n"));
        assert!(!md.contains("## User"), "user turn must be excluded:\n{md}");
        assert!(!md.contains("please summarize"));
        assert!(md.contains("## Assistant · "));
        assert!(md.contains("Done — here is the summary."));
    }

    #[test]
    fn preserves_code_fences_and_line_breaks_in_export() {
        // Regression: the .md export must keep fenced code blocks and list line
        // breaks intact. `split_whitespace().join(" ")` collapsed them into a
        // single paragraph; the fix in threads.rs never reached this exporter.
        let raw = "Here:\n\n```rust\nfn main() {}\n```\n\n- a\n- b";
        let md = thread_to_markdown(&thread(vec![message("assistant", raw)]), false);
        assert!(
            md.contains("```rust\nfn main() {}\n```"),
            "fenced code block must survive export:\n{md}"
        );
        assert!(
            md.contains("- a\n- b"),
            "list line breaks must survive export:\n{md}"
        );
    }

    #[test]
    fn skips_turns_with_no_visible_text() {
        // A tool-use-only turn (no plain text) should not emit a section.
        let mut msgs = vec![message("assistant", "Result ready.")];
        msgs.insert(
            0,
            super::super::thread_store::ThreadMessage {
                role: "assistant".to_string(),
                content: vec![
                    json!({ "type": "tool_use", "id": "abc", "name": "grep", "input": {} }),
                ],
                timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 29, 0).unwrap(),
                metadata: None,
            },
        );
        let md = thread_to_markdown(&thread(msgs), false);
        // Only one Assistant section (the tool-use-only turn is skipped).
        assert_eq!(md.matches("## Assistant").count(), 1, "\n{md}");
        assert!(md.contains("Result ready."));
    }
}
