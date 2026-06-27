//! Chat-to-Markdown export and history persistence.

use super::*;

/// Export the current Agent chat thread as Markdown.
///
/// - `assistant_only=false` → include User + Assistant messages
/// - `assistant_only=true` → include only Assistant messages
pub fn export_chat_markdown(assistant_only: bool) -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    chat_markdown_from_messages(&state.messages, assistant_only)
}

/// Save the current Agent chat thread as a `.md` file in `~/.codescribe/transcriptions/YYYY-MM-DD/`.
///
/// Returns the created path on success.
pub fn save_chat_markdown_to_history(assistant_only: bool) -> Option<PathBuf> {
    let md = export_chat_markdown(assistant_only);
    if md.trim().is_empty() {
        return None;
    }

    let now = Local::now();
    let dir = crate::state::history::transcriptions_dir(&now);
    let time_base = now.format("%H%M%S").to_string();
    let kind = if assistant_only {
        "chat-assistant"
    } else {
        "chat"
    };

    let mut candidate = dir.join(format!("{}_{}.md", time_base, kind));
    for i in 1..=10_000 {
        if !candidate.exists() {
            break;
        }
        candidate = dir.join(format!("{}_{}_{}.md", time_base, kind, i));
    }

    if std::fs::write(&candidate, md).is_ok() {
        Some(candidate)
    } else {
        None
    }
}

pub fn chat_markdown_from_messages(messages: &[ChatMessage], assistant_only: bool) -> String {
    let exported_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut out = String::new();
    out.push_str("# CodeScribe Chat Export\n\n");
    out.push_str(&format!("- exported_at: {}\n", exported_at));
    out.push_str(&format!(
        "- scope: {}\n\n",
        if assistant_only {
            "assistant_only"
        } else {
            "all"
        }
    ));

    for msg in messages {
        if assistant_only && msg.role != ChatRole::Assistant {
            continue;
        }
        let role = match msg.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            ChatRole::System => "System",
            ChatRole::Reasoning => "Reasoning",
            ChatRole::ToolActivity => "Tool activity",
        };
        out.push_str(&format!("## {}\n\n", role));
        out.push_str(msg.text.trim_end());
        out.push_str("\n\n");
    }

    out.trim_end().to_string() + "\n"
}
