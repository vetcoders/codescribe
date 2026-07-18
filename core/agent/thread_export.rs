//! Render a persisted agent `Thread` to a Markdown transcript.
//!
//! Pure formatting — no filesystem. The bridge (`bridge/src/threads.rs`) owns
//! saving the returned string under `~/.codescribe/transcriptions/YYYY-MM-DD/`.
//! The shape mirrors the legacy voice-chat `chat_markdown_from_messages`
//! (`app/ui/voice_chat/api/export.rs`, removed in 37efe51): a thread heading,
//! export metadata, then one `## Role · timestamp` section per turn.
//!
//! Tool results are the exception. In the Anthropic/OpenAI protocol a tool
//! result is a `tool_result` content block carried inside a **user-role**
//! message; naively keying the section header off `message.role` mis-attributes
//! every tool payload (health JSON, MCP errors, search output) as if the user
//! had typed it. So tool-result blocks are lifted out of the role-based prose
//! flow and rendered under their own `## Tool · <name> · timestamp` sections
//! (`## Tool result · timestamp` when the invoking tool's name can't be
//! resolved). Genuine user prose still renders as `## User`.

use std::collections::HashMap;

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

use super::thread_store::{Thread, ThreadMessage};

/// A tool-result content block lifted out of its carrier message for rendering.
struct ToolResultEntry {
    /// The `tool_use` id this result answers (used to resolve the tool name).
    tool_use_id: String,
    /// Flattened, human-readable body of the result.
    body: String,
    is_error: bool,
}

/// Build a Markdown transcript for `thread`.
///
/// `assistant_only = true` keeps only assistant turns (mirrors the legacy
/// "assistant replies only" export variant); otherwise every turn carrying
/// visible text is included. Turns whose content flattens to nothing (e.g.
/// pure tool-use / image payloads) are skipped either way. Tool results are
/// attributed to the tool, not the user, regardless of the carrier role.
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

    // tool_use_id -> tool name, harvested from the assistant `tool_use` blocks
    // so a `tool_result` (which only carries the id) can name its invoker.
    let tool_names = build_tool_name_index(&thread.messages);

    for message in &thread.messages {
        if assistant_only && !is_assistant(&message.role) {
            continue;
        }

        // Prose (real text the role authored) keeps the role-based header. This
        // deliberately does not dive into `tool_result` blocks, so a user turn
        // that merely relays tool output produces no phantom `## User` section.
        let prose = flatten_prose(&message.content);
        if !prose.trim().is_empty() {
            out.push_str(&format!(
                "## {} · {}\n\n",
                role_label(&message.role),
                local_timestamp(message.timestamp)
            ));
            out.push_str(prose.trim_end());
            out.push_str("\n\n");
        }

        // Tool results are attributed to the tool, not the carrier role.
        for entry in collect_tool_results(&message.content) {
            if entry.body.trim().is_empty() {
                continue;
            }
            let mut header = match tool_names.get(&entry.tool_use_id) {
                Some(name) => format!("Tool · {name}"),
                None => "Tool result".to_string(),
            };
            if entry.is_error {
                header.push_str(" (error)");
            }
            out.push_str(&format!(
                "## {} · {}\n\n",
                header,
                local_timestamp(message.timestamp)
            ));
            out.push_str(entry.body.trim_end());
            out.push_str("\n\n");
        }
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

/// Map every `tool_use` block's id to its tool name across the whole thread.
/// The name lives on the assistant's `tool_use` block; a `tool_result` only
/// stores the referencing `tool_use_id`, so this index lets us name the tool
/// without persisting anything new alongside the result.
fn build_tool_name_index(messages: &[ThreadMessage]) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for message in messages {
        for value in &message.content {
            gather_tool_names(value, &mut index);
        }
    }
    index
}

fn gather_tool_names(value: &Value, index: &mut HashMap<String, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                gather_tool_names(item, index);
            }
        }
        Value::Object(map) if map.get("type").and_then(Value::as_str) == Some("tool_use") => {
            if let (Some(id), Some(name)) = (
                map.get("id").and_then(Value::as_str),
                map.get("name").and_then(Value::as_str),
            ) {
                index.insert(id.to_string(), name.to_string());
            }
        }
        _ => {}
    }
}

/// Lift every `tool_result` block out of a message's content, flattening each
/// result's nested body for standalone rendering.
fn collect_tool_results(content: &[Value]) -> Vec<ToolResultEntry> {
    let mut entries = Vec::new();
    for value in content {
        gather_tool_results(value, &mut entries);
    }
    entries
}

fn gather_tool_results(value: &Value, entries: &mut Vec<ToolResultEntry>) {
    match value {
        Value::Array(items) => {
            for item in items {
                gather_tool_results(item, entries);
            }
        }
        Value::Object(map) if map.get("type").and_then(Value::as_str) == Some("tool_result") => {
            let body = map
                .get("content")
                .map(flatten_tool_body)
                .unwrap_or_default();
            entries.push(ToolResultEntry {
                tool_use_id: map
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                body,
                is_error: map
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        _ => {}
    }
}

/// Flatten a message's prose content into a display string, skipping
/// `tool_result` blocks (rendered separately) so tool output never leaks into a
/// role-attributed section. See `collect_text` for block handling.
fn flatten_prose(content: &[Value]) -> String {
    let mut chunks = Vec::new();
    for value in content {
        collect_text(value, &mut chunks, false);
    }
    join_chunks(chunks)
}

/// Flatten the nested content of a single `tool_result` block into its body,
/// recursing through the wrapped `text` blocks.
fn flatten_tool_body(content: &Value) -> String {
    let mut chunks = Vec::new();
    collect_text(content, &mut chunks, true);
    join_chunks(chunks)
}

fn join_chunks(chunks: Vec<String>) -> String {
    chunks
        .iter()
        .map(|chunk| chunk.trim())
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Collect human-readable prose from a stored content `Value`, keyed on the
/// canonical `content_block_to_value` shapes (`thread_store.rs`): `text` blocks
/// contribute their text, with legacy `input_text` / `output_text` aliases read
/// the same way for already-persisted OpenAI-shaped files. `tool_use` / `image`
/// / `image_asset` blocks carry no display prose (skipped). `tool_result` blocks
/// are only descended into when `dive_tool_result` is set (i.e. when rendering a
/// result body via `flatten_tool_body`); prose flattening leaves them for
/// `collect_tool_results` so tool output is never attributed to the carrier
/// role. A field-only extractor (as opposed to a blind key walk) avoids emitting
/// structural values like the literal `"text"` / `"tool_use"` type tags.
/// Interior whitespace — newlines, fenced code blocks, list breaks — is
/// preserved; only each block's outer edges are trimmed.
fn collect_text(value: &Value, out: &mut Vec<String>, dive_tool_result: bool) {
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            out.push(text.trim().to_string());
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, out, dive_tool_result);
            }
        }
        Value::Object(map) => match map.get("type").and_then(Value::as_str) {
            Some("text") | Some("input_text") | Some("output_text") | None => {
                if let Some(text) = map.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    out.push(text.trim().to_string());
                }
            }
            Some("tool_result") => {
                if dive_tool_result && let Some(nested) = map.get("content") {
                    collect_text(nested, out, dive_tool_result);
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
    fn renders_legacy_openai_text_alias_blocks() {
        let timestamp = Utc.with_ymd_and_hms(2026, 7, 2, 9, 31, 0).unwrap();
        let md = thread_to_markdown(
            &thread(vec![
                super::super::thread_store::ThreadMessage {
                    role: "user".to_string(),
                    content: vec![json!({ "type": "input_text", "text": "legacy prompt" })],
                    timestamp,
                    metadata: None,
                },
                super::super::thread_store::ThreadMessage {
                    role: "assistant".to_string(),
                    content: vec![json!({ "type": "output_text", "text": "legacy reply" })],
                    timestamp,
                    metadata: None,
                },
            ]),
            false,
        );

        assert!(md.contains("legacy prompt"), "\n{md}");
        assert!(md.contains("legacy reply"), "\n{md}");
        assert!(!md.contains("input_text"), "\n{md}");
        assert!(!md.contains("output_text"), "\n{md}");
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

    #[test]
    fn tool_result_is_attributed_to_tool_not_user() {
        // A user-role message that merely carries a tool_result must render
        // under a Tool section, never `## User`. The tool name is resolved from
        // the matching assistant `tool_use` block.
        let tool_use = super::super::thread_store::ThreadMessage {
            role: "assistant".to_string(),
            content: vec![json!({
                "type": "tool_use",
                "id": "call_1",
                "name": "health",
                "input": {},
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 28, 0).unwrap(),
            metadata: None,
        };
        let tool_result = super::super::thread_store::ThreadMessage {
            role: "user".to_string(),
            content: vec![json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": [{ "type": "text", "text": "{\"status\":\"ok\"}" }],
                "is_error": false,
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 29, 0).unwrap(),
            metadata: None,
        };
        let md = thread_to_markdown(
            &thread(vec![tool_use, tool_result, message("user", "thanks!")]),
            false,
        );
        assert!(
            md.contains("## Tool · health · "),
            "tool result must render under a named Tool section:\n{md}"
        );
        assert!(md.contains("{\"status\":\"ok\"}"), "\n{md}");
        // Exactly one User section — the genuine prose turn, not the tool relay.
        assert_eq!(
            md.matches("## User").count(),
            1,
            "only the real user prose is a User section:\n{md}"
        );
        assert!(md.contains("thanks!"));
    }

    #[test]
    fn tool_result_without_matching_tool_use_falls_back_and_flags_errors() {
        // No assistant tool_use to resolve the name -> "Tool result"; is_error
        // is surfaced in the header.
        let tool_result = super::super::thread_store::ThreadMessage {
            role: "user".to_string(),
            content: vec![json!({
                "type": "tool_result",
                "tool_use_id": "orphan",
                "content": [{ "type": "text", "text": "connection refused" }],
                "is_error": true,
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 29, 0).unwrap(),
            metadata: None,
        };
        let md = thread_to_markdown(&thread(vec![tool_result]), false);
        assert!(
            md.contains("## Tool result (error) · "),
            "unresolved erroring tool result must be flagged:\n{md}"
        );
        assert!(md.contains("connection refused"), "\n{md}");
        assert!(!md.contains("## User"), "no user attribution:\n{md}");
    }

    #[test]
    fn assistant_only_drops_tool_results_carried_by_user() {
        // Tool results live in user-role messages, so assistant_only export
        // (legacy "assistant replies only") must not include them.
        let tool_result = super::super::thread_store::ThreadMessage {
            role: "user".to_string(),
            content: vec![json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": [{ "type": "text", "text": "secret tool payload" }],
                "is_error": false,
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 7, 2, 9, 29, 0).unwrap(),
            metadata: None,
        };
        let md = thread_to_markdown(
            &thread(vec![tool_result, message("assistant", "All set.")]),
            true,
        );
        assert!(!md.contains("## Tool"), "\n{md}");
        assert!(!md.contains("secret tool payload"), "\n{md}");
        assert!(md.contains("## Assistant · "));
        assert!(md.contains("All set."));
    }
}
