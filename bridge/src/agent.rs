//! Agent streaming surface — thin UniFFI wrapper over the live codescribe
//! `AgentSession` (token/reasoning/tool-call streaming). Moved out of `lib.rs`
//! in W3 cut #0 so each bridge slice owns a disjoint file.

use std::sync::Arc;

use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ContentBlock, ImageAttachment, Message, Role, StreamOptions,
    Thread, ThreadMessage, ThreadStore, ToolRegistry,
};
use codescribe_core::attachment::{MAX_VISION_IMAGE_BYTES, load_image_for_vision};
use codescribe_core::llm::provider::{LlmMode, provider_supports_vision, resolve_provider};

use crate::CsError;

/// Maximum number of image attachments the composer may forward in one message.
/// Matches the live app controller's `MAX_AGENT_VISION_IMAGES` so both send paths
/// behave alike; exceeding it is surfaced as a readable error rather than a silent
/// truncation.
const MAX_COMPOSER_VISION_IMAGES: usize = 16;

/// One outgoing composer attachment. Path-based on purpose: the bridge reads and
/// validates the file on the Rust side (via `load_image_for_vision`), which is
/// cheaper than marshalling raw image bytes across FFI and reuses core's single
/// vision-loading path. Swift persists clipboard images to disk before handing a
/// path here, so every attachment reduces to a filesystem path.
#[derive(uniffi::Record)]
pub struct CsAttachment {
    /// Absolute filesystem path to the attached image.
    pub path: String,
}

/// Foreign callback trait — agent streaming events forwarded to Swift.
/// Mirrors `AgentUiEvent`; the Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsAgentListener: Send + Sync {
    fn on_text_delta(&self, delta: String);
    fn on_text_done(&self, text: String);
    fn on_reasoning_delta(&self, delta: String);
    fn on_tool_executing(&self, name: String, id: String);
    fn on_tool_result(&self, name: String, id: String, summary: String, is_error: bool);
    fn on_done(&self);
    fn on_error(&self, message: String);
}

/// Thin handle to the codescribe agent engine.
#[derive(uniffi::Object, Default)]
pub struct CodescribeAgent {}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeAgent {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// True when the assistive LLM provider can be built from the environment
    /// (LLM_ASSISTIVE_ENDPOINT / _MODEL / _API_KEY present). Same gate the live
    /// app uses before agent replies are possible.
    pub fn is_available(&self) -> bool {
        // Warm settings + Keychain only when the agent surface is actually used.
        // Constructing the Swift app model must not trigger a keychain prompt.
        let _ = codescribe_core::config::Config::load();
        codescribe::agent::create_default_provider().is_ok()
    }

    /// Stream one agent reply for `text` on the conversation identified by
    /// `thread_id`, forwarding token/reasoning/tool events to `listener` as they
    /// arrive. Returns the final assembled assistant text.
    ///
    /// Memory + persistence: prior turns stored under `thread_id` are restored
    /// into the session before sending (so the model sees the conversation
    /// history), and the updated thread is written back after a successful reply
    /// so the SwiftUI app's conversations survive restart. Persistence is
    /// best-effort: a load/save failure never fails the reply the user already
    /// saw.
    ///
    /// Full native tool set + MCP are registered, so the agent can actually act
    /// (clipboard, selection, screenshot, filesystem, typing, github, search,
    /// transcribe). Tools execute on demand when the model calls them.
    pub async fn stream_reply(
        &self,
        text: String,
        thread_id: String,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        self.run_stream(text, thread_id, Vec::new(), listener).await
    }

    /// Stream one agent reply for `text` with `attachments` forwarded as real
    /// vision input (the composer 📎 path). Attachments are path-based; the bridge
    /// loads + validates each one via core's single `load_image_for_vision` path
    /// (PNG/JPEG/GIF/WebP/BMP/TIFF, ≤ 8 MB each) so the send never routes raw
    /// bytes through FFI and never produces a second attachment pipeline.
    ///
    /// Degradation is explicit, never a silent drop:
    /// - the selected model is not vision-capable ⇒ readable error, nothing sent;
    /// - any attachment is missing / unsupported / too large / empty ⇒ readable
    ///   error naming the offending file(s), nothing sent;
    /// - more than 16 images ⇒ readable error.
    pub async fn stream_reply_with_attachments(
        &self,
        text: String,
        thread_id: String,
        attachments: Vec<CsAttachment>,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        let images = validate_composer_attachments(&attachments)?;
        self.run_stream(text, thread_id, images, listener).await
    }
}

impl CodescribeAgent {
    /// Shared streaming core behind [`stream_reply`] and
    /// [`stream_reply_with_attachments`]. `attachments` are already loaded +
    /// validated `ImageAttachment`s (empty for the text-only path).
    async fn run_stream(
        &self,
        text: String,
        thread_id: String,
        attachments: Vec<ImageAttachment>,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        // Keep provider construction behavior identical to the old eager
        // constructor path, but delay it until the user sends a message.
        let config = codescribe_core::config::Config::load();
        let provider = codescribe::agent::create_default_provider()?;
        let mut registry = ToolRegistry::new();
        codescribe::agent::tools::register_all_tools(&mut registry);
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AgentUiEvent>(64);
        let mut session = AgentSession::new(provider, Arc::new(registry), ui_tx);

        // Restore prior turns for cross-turn memory. ThreadStore does blocking
        // fs I/O, so the load runs on a blocking pool thread and is awaited
        // before the agent loop starts. A missing/corrupt thread yields an empty
        // history (best-effort: a first turn simply has nothing to restore).
        let thread_id_for_load = thread_id.clone();
        let restored: Vec<Message> = tokio::task::spawn_blocking(move || {
            ThreadStore::new()
                .ok()
                .and_then(|store| store.load_thread(&thread_id_for_load).ok())
                .map(|thread| {
                    thread
                        .messages
                        .iter()
                        .map(ThreadMessage::to_message)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !restored.is_empty() {
            // Seeds the conversation history; resets the provider chain id to
            // None (the persistence id is `thread_id`, separate from the
            // provider's response-chain id).
            session.restore_messages(restored);
        }

        // Honor the same assistive system prompt + token cap the in-app
        // controller path uses (build_agent_stream_options), so a Swift chat send
        // is not stripped of the WORKSPACE-augmented assistive prompt and the
        // configured `ai_assistive_max_tokens`.
        let options = build_bridge_stream_options(config.ai_assistive_max_tokens);

        // Drive the agent loop on a task so the channel closes when it finishes,
        // letting the drain loop below terminate cleanly. The task hands back the
        // session's final message log so the caller can persist the thread.
        let send_handle = tokio::spawn(async move {
            let mut session = session;
            let attachments = attachments;
            session.send(text, attachments, &options).await?;
            Ok::<Vec<Message>, anyhow::Error>(session.messages().to_vec())
        });

        let mut final_text = String::new();
        while let Some(event) = ui_rx.recv().await {
            match event {
                AgentUiEvent::TextDelta(delta) => listener.on_text_delta(delta),
                AgentUiEvent::TextDone(t) => {
                    final_text = t.clone();
                    listener.on_text_done(t);
                }
                AgentUiEvent::ReasoningDelta(delta) => listener.on_reasoning_delta(delta),
                AgentUiEvent::ToolExecuting { name, id } => listener.on_tool_executing(name, id),
                AgentUiEvent::ToolResult {
                    name,
                    id,
                    summary,
                    is_error,
                } => listener.on_tool_result(name, id, summary, is_error),
                AgentUiEvent::Done => listener.on_done(),
                AgentUiEvent::Error(message) => listener.on_error(message),
            }
        }

        match send_handle.await {
            Ok(Ok(messages)) => {
                // Persist the updated thread (best-effort). The reply already
                // streamed to the user, so a save failure is logged-and-ignored
                // rather than surfaced as an error.
                persist_thread(thread_id, messages).await;
                Ok(final_text)
            }
            Ok(Err(error)) => Err(CsError::Agent {
                msg: error.to_string(),
            }),
            Err(join_error) => Err(CsError::Agent {
                msg: format!("agent task join error: {join_error}"),
            }),
        }
    }
}

/// Build the assistive stream options for a bridge chat send, honoring the same
/// assistive system prompt and token cap the in-app controller path uses
/// (`app/controller/helpers.rs::build_agent_stream_options`). Model is left empty
/// so the provider resolves it from `LLM_ASSISTIVE_MODEL` (identical default to
/// the controller), keeping both send paths behaviorally aligned.
fn build_bridge_stream_options(ai_assistive_max_tokens: i32) -> StreamOptions {
    let max_tokens = u32::try_from(ai_assistive_max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0);
    StreamOptions {
        model: String::new(),
        system_prompt: Some(compose_agent_system_prompt()),
        max_tokens,
        temperature: None,
        reset_chain: false,
    }
}

/// Compose the agent system prompt exactly like the controller path
/// (`app/controller/helpers.rs::compose_agent_system_prompt`): the base assistive
/// prompt plus the WORKSPACE section (6238ca1) that pins project roots and tells
/// the model to resolve names via `list_projects` instead of guessing paths.
fn compose_agent_system_prompt() -> String {
    let base = codescribe_core::config::prompts::get_assistive_prompt();
    let workspace = codescribe::agent::tools::workspace::workspace_prompt_section();
    format!("{base}\n\n{workspace}")
}

/// Load + validate composer attachments into vision `ImageAttachment`s.
///
/// All-or-nothing on purpose: a partial success would silently drop images the
/// user chose to attach. Any failure returns a readable [`CsError`] naming the
/// offending files so the composer surfaces it instead of sending a quietly
/// degraded message. Also gates on the selected model's vision capability.
fn validate_composer_attachments(
    attachments: &[CsAttachment],
) -> Result<Vec<ImageAttachment>, CsError> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    if attachments.len() > MAX_COMPOSER_VISION_IMAGES {
        return Err(CsError::Agent {
            msg: format!(
                "Too many images ({}). Attach at most {} per message.",
                attachments.len(),
                MAX_COMPOSER_VISION_IMAGES
            ),
        });
    }

    // Vision gate: refuse (readable error) rather than silently drop the images
    // when the configured assistive model cannot read them.
    let provider = resolve_provider(LlmMode::Assistive);
    let model = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_default();
    if !provider_supports_vision(provider, &model) {
        return Err(CsError::Agent {
            msg: "The selected model can't read images. Switch to a vision-capable \
                  model in Settings, or remove the attachment before sending."
                .to_string(),
        });
    }

    let mut images = Vec::with_capacity(attachments.len());
    let mut failed: Vec<String> = Vec::new();
    for attachment in attachments {
        let path = std::path::Path::new(&attachment.path);
        match load_image_for_vision(path, MAX_VISION_IMAGE_BYTES) {
            Some((data, media_type)) => images.push(ImageAttachment { data, media_type }),
            None => failed.push(attachment_label(&attachment.path)),
        }
    }

    if !failed.is_empty() {
        return Err(CsError::Agent {
            msg: format!(
                "Couldn't attach {}: image must be PNG, JPEG, GIF, WebP, BMP, or \
                 TIFF and 8 MB or smaller.",
                failed.join(", ")
            ),
        });
    }

    Ok(images)
}

/// Short, user-facing label (file name, path fallback) for an attachment path.
fn attachment_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Persist (create or update) the thread identified by `thread_id` from the
/// session's final `messages`. Mirrors the live app's `persist_runtime_thread`
/// (app/controller/helpers.rs): load-or-build a `Thread`, refresh title/summary/
/// messages, and save. Runs the blocking fs work on a blocking pool thread and
/// swallows any error — persistence is best-effort.
async fn persist_thread(thread_id: String, messages: Vec<Message>) {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let store = ThreadStore::new()?;

        // `now` is sourced from the freshest message timestamp the session
        // stamped (`Some(Utc::now())` per turn), avoiding a direct `chrono`
        // dependency in the bridge crate. With nothing to anchor the thread to,
        // skip the write.
        let Some(now) = messages.iter().rev().find_map(|message| message.timestamp) else {
            return Ok(());
        };

        let model = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_default();
        // Reflect the resolved assistive provider so persisted thread metadata is
        // accurate when Anthropic is active (was hardcoded "openai-responses").
        let provider = resolve_provider(LlmMode::Assistive).as_str().to_string();

        let mut thread = store.load_thread(&thread_id).unwrap_or_else(|_| Thread {
            id: thread_id.clone(),
            created_at: now,
            updated_at: now,
            title: "Codescribe Agent Chat".to_string(),
            title_is_custom: false,
            mode: "assistive".to_string(),
            tags: vec!["agent".to_string(), "overlay".to_string()],
            notes: Vec::new(),
            messages: Vec::new(),
            summary: None,
            total_tokens: None,
            provider: provider.clone(),
            model: model.clone(),
        });

        thread.updated_at = now;
        // Never clobber a title the user set by hand from the rail.
        if !thread.title_is_custom {
            thread.title = derive_thread_title(&messages);
        }
        thread.summary = derive_thread_summary(&messages);
        thread.messages = messages.iter().map(ThreadMessage::from).collect();
        thread.provider = provider;
        thread.model = model;

        store.save_thread(&thread)?;
        Ok(())
    })
    .await;

    if let Ok(Err(error)) = result {
        // Bridge crate has no logging dep; stderr keeps the best-effort failure
        // visible without taking the reply down.
        eprintln!("Failed to persist agent thread (best-effort): {error}");
    }
}

/// First user message, boilerplate-stripped and trimmed to a title-length slice.
///
/// Every agent conversation is seeded with a pasted instruction preamble
/// ("INSTRUKCJA UŻYTKOWNIKA: JESTEŚ AGENTEM…"), so a naive first-line title makes
/// every thread read identically. We first try the newline-preserving raw text
/// and skip leading instruction/header lines; only if the whole message looks
/// like boilerplate do we fall back to the collapsed full text.
fn derive_thread_title(messages: &[Message]) -> String {
    let first_user = messages.iter().find(|message| message.role == Role::User);

    let candidate = first_user
        .and_then(raw_text_from_message)
        .and_then(|raw| strip_boilerplate_title(&raw))
        .or_else(|| first_user.and_then(extract_text_from_message))
        .unwrap_or_else(|| "Codescribe Agent Chat".to_string());

    let mut title = candidate.chars().take(72).collect::<String>();
    if title.trim().is_empty() {
        title = "Codescribe Agent Chat".to_string();
    }
    title
}

/// Newline-preserving flatten of a message's textual content (unlike
/// `extract_text_from_message`, which collapses all whitespace). Lets the title
/// heuristic reason about the first "real" line.
fn raw_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let text = out.join("\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Known leading-boilerplate line prefixes, matched case-insensitively against
/// the trimmed line. Pasted agent preambles open with one of these.
const BOILERPLATE_LINE_PREFIXES: &[&str] = &[
    "instrukcja",
    "instruction",
    "jesteś agentem",
    "jestes agentem",
    "you are an agent",
    "system prompt",
    "system:",
];

/// Drop leading instruction/header lines and return the first meaningful line,
/// whitespace-normalized. Returns `None` when every line looks like boilerplate
/// (the caller then falls back to the collapsed full text).
fn strip_boilerplate_title(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_boilerplate_line(trimmed) {
            continue;
        }
        let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        if !normalized.is_empty() {
            return Some(normalized);
        }
    }
    None
}

/// A line is boilerplate when it opens with a known preamble prefix or reads as
/// an all-caps header (letters present, none lowercase — e.g.
/// "INSTRUKCJA UŻYTKOWNIKA:").
fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    if BOILERPLATE_LINE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    is_all_caps_header(line)
}

/// True when the line has alphabetic characters and none of them are lowercase.
fn is_all_caps_header(line: &str) -> bool {
    let mut has_alpha = false;
    for ch in line.chars() {
        if ch.is_alphabetic() {
            has_alpha = true;
            if ch.is_lowercase() {
                return false;
            }
        }
    }
    has_alpha
}

/// Latest assistant message, trimmed to a summary-length slice. Replica of
/// `derive_thread_summary` in app/controller/helpers.rs.
fn derive_thread_summary(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .and_then(extract_text_from_message)
        .map(|text| {
            let mut clipped = text.chars().take(240).collect::<String>();
            if clipped.is_empty() {
                clipped = "Assistant response".to_string();
            }
            clipped
        })
}

/// Flatten a message's textual content into a single normalized string. Replica
/// of `extract_text_from_message` in app/controller/helpers.rs.
fn extract_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let text = out.join(" ");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Collect text from a content block (recursing into tool results). Replica of
/// `extract_text_from_block` in app/controller/helpers.rs.
fn extract_text_from_block(block: &ContentBlock, out: &mut Vec<String>) {
    match block {
        ContentBlock::Text(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        ContentBlock::ToolResult { content, .. } => {
            for nested in content {
                extract_text_from_block(nested, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cs_bridge_attach_{}_{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn cs(path: &std::path::Path) -> CsAttachment {
        CsAttachment {
            path: path.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn empty_attachments_yield_no_images() {
        let images = validate_composer_attachments(&[]).unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn valid_image_loads_as_vision_attachment() {
        let dir = tmp_dir("valid");
        let png = dir.join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nfake").unwrap();

        let images = validate_composer_attachments(&[cs(&png)]).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        assert!(!images[0].data.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_or_nonimage_is_a_readable_error_not_a_silent_drop() {
        let dir = tmp_dir("bad");
        let txt = dir.join("note.txt");
        std::fs::write(&txt, b"hello").unwrap();
        let missing = dir.join("gone.png");

        let err = validate_composer_attachments(&[cs(&txt), cs(&missing)]).unwrap_err();
        let CsError::Agent { msg } = err else {
            panic!("expected a readable agent error");
        };
        assert!(
            msg.contains("note.txt"),
            "names the unsupported file: {msg}"
        );
        assert!(msg.contains("gone.png"), "names the missing file: {msg}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn too_many_images_is_rejected() {
        let attachments: Vec<CsAttachment> = (0..=MAX_COMPOSER_VISION_IMAGES)
            .map(|i| CsAttachment {
                path: format!("/tmp/x{i}.png"),
            })
            .collect();
        let err = validate_composer_attachments(&attachments).unwrap_err();
        let CsError::Agent { msg } = err else {
            panic!("expected a readable agent error");
        };
        assert!(msg.contains("Too many"), "explains the cap: {msg}");
    }

    fn user_message(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text(text.to_string())],
            timestamp: None,
        }
    }

    #[test]
    fn title_skips_boilerplate_preamble() {
        let text = "INSTRUKCJA UŻYTKOWNIKA: JESTEŚ AGENTEM\n\nNapraw hang na starcie sesji";
        let title = derive_thread_title(&[user_message(text)]);
        assert_eq!(title, "Napraw hang na starcie sesji");
    }

    #[test]
    fn title_keeps_plain_first_line() {
        let title = derive_thread_title(&[user_message("Fix the rate limiter double-fire")]);
        assert_eq!(title, "Fix the rate limiter double-fire");
    }

    #[test]
    fn title_falls_back_when_all_boilerplate() {
        // Single merged line: prefix-flagged, so stripping yields nothing and we
        // fall back to the collapsed full text (never worse than before).
        let text = "INSTRUKCJA: zrób coś";
        let title = derive_thread_title(&[user_message(text)]);
        assert_eq!(title, "INSTRUKCJA: zrób coś");
    }
}
