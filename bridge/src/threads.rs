//! Thread persistence + transcript history surface — a thin, synchronous UniFFI
//! wrapper over the live codescribe `ThreadStore` / `ThreadIndex` (saved agent
//! conversations) and `state::history` (on-disk transcript artifacts). Split out
//! of `lib.rs` in W3 cut #5 so each bridge slice owns a disjoint file.
//!
//! All cross-FFI types are UniFFI-able: `DateTime<Utc>` / `DateTime<Local>` are
//! flattened to `i64` epoch-millis, `serde_json::Value` to a `raw_json` String,
//! `PathBuf` to String, and `usize` to `u64`. No secret values cross the boundary.

use std::fs;

use chrono::Local;
use codescribe_core::agent::thread_export::thread_to_markdown;
use codescribe_core::agent::thread_index::{ThreadFilter, ThreadIndex, ThreadSummary};
use codescribe_core::agent::thread_store::{
    Thread, ThreadMessage, ThreadNote, ThreadStore, TokenUsage,
};
use codescribe_core::state::history::{self, HistoryEntry, TranscriptKind};
use serde_json::Value;

use crate::CsError;

/// Cumulative token accounting for a thread. Mirrors `TokenUsage`
/// (`thread_store.rs:105`).
#[derive(uniffi::Record)]
pub struct CsTokenUsage {
    pub input: u64,
    pub output: u64,
}

impl From<&TokenUsage> for CsTokenUsage {
    fn from(usage: &TokenUsage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
        }
    }
}

/// Lightweight thread index entry used to render the thread list/search.
/// Mirrors `ThreadSummary` (`thread_index.rs:30`); the internal `search_text`
/// field is intentionally omitted (index-only, not display-facing).
#[derive(uniffi::Record)]
pub struct CsThreadSummary {
    pub id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub message_count: u64,
    pub mode: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub has_notes: bool,
    pub latest_message: Option<String>,
    pub latest_note: Option<String>,
    pub is_favorite: bool,
}

impl From<&ThreadSummary> for CsThreadSummary {
    fn from(summary: &ThreadSummary) -> Self {
        Self {
            id: summary.id.clone(),
            title: summary.title.clone(),
            created_at_ms: summary.created_at.timestamp_millis(),
            updated_at_ms: summary.updated_at.timestamp_millis(),
            message_count: summary.message_count as u64,
            mode: summary.mode.clone(),
            tags: summary.tags.clone(),
            summary: summary.summary.clone(),
            has_notes: summary.has_notes,
            latest_message: summary.latest_message.clone(),
            latest_note: summary.latest_note.clone(),
            is_favorite: summary.is_favorite,
        }
    }
}

/// One message inside a thread. `text` is the flattened, human-readable content
/// (replicating the private preview logic at `thread_index.rs:334/348`, without
/// the search-side lowercasing). `raw_json` carries the full structured content
/// array so callers can recover tool calls / images. Mirrors `ThreadMessage`
/// (`thread_store.rs:54`).
#[derive(uniffi::Record)]
pub struct CsThreadMessage {
    pub role: String,
    pub text: String,
    pub raw_json: String,
    pub timestamp_ms: i64,
}

impl From<&ThreadMessage> for CsThreadMessage {
    fn from(message: &ThreadMessage) -> Self {
        Self {
            role: message.role.clone(),
            text: flatten_message_text(&message.content),
            raw_json: serde_json::to_string(&message.content).unwrap_or_default(),
            timestamp_ms: message.timestamp.timestamp_millis(),
        }
    }
}

/// A pinned note attached to a thread. Mirrors `ThreadNote`
/// (`thread_store.rs:96`).
#[derive(uniffi::Record)]
pub struct CsThreadNote {
    pub id: String,
    pub created_at_ms: i64,
    pub text: String,
    pub anchored_to_message: Option<u64>,
}

impl From<&ThreadNote> for CsThreadNote {
    fn from(note: &ThreadNote) -> Self {
        Self {
            id: note.id.clone(),
            created_at_ms: note.created_at.timestamp_millis(),
            text: note.text.clone(),
            anchored_to_message: note.anchored_to_message.map(|index| index as u64),
        }
    }
}

/// A fully loaded conversation thread. Mirrors `Thread` (`thread_store.rs:20`).
#[derive(uniffi::Record)]
pub struct CsThread {
    pub id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub title: String,
    pub mode: String,
    pub tags: Vec<String>,
    pub notes: Vec<CsThreadNote>,
    pub messages: Vec<CsThreadMessage>,
    pub summary: Option<String>,
    pub total_tokens: Option<CsTokenUsage>,
    pub provider: String,
    pub model: String,
}

impl From<&Thread> for CsThread {
    fn from(thread: &Thread) -> Self {
        Self {
            id: thread.id.clone(),
            created_at_ms: thread.created_at.timestamp_millis(),
            updated_at_ms: thread.updated_at.timestamp_millis(),
            title: thread.title.clone(),
            mode: thread.mode.clone(),
            tags: thread.tags.clone(),
            notes: thread.notes.iter().map(CsThreadNote::from).collect(),
            messages: thread.messages.iter().map(CsThreadMessage::from).collect(),
            summary: thread.summary.clone(),
            total_tokens: thread.total_tokens.as_ref().map(CsTokenUsage::from),
            provider: thread.provider.clone(),
            model: thread.model.clone(),
        }
    }
}

/// Filter applied to `list_threads`. Mirrors `ThreadFilter`
/// (`thread_index.rs:111`).
#[derive(uniffi::Record)]
pub struct CsThreadFilter {
    pub mode: Option<String>,
    pub favorites_only: bool,
    pub has_notes: bool,
    pub tag: Option<String>,
}

impl From<CsThreadFilter> for ThreadFilter {
    fn from(filter: CsThreadFilter) -> Self {
        Self {
            mode: filter.mode,
            favorites_only: filter.favorites_only,
            has_notes: filter.has_notes,
            tag: filter.tag,
        }
    }
}

/// What kind of transcript artifact a history entry holds. Mirrors
/// `TranscriptKind` (`state/history.rs:27`).
#[derive(uniffi::Enum)]
pub enum CsTranscriptKind {
    Raw,
    Cloud,
    FormattedTranscript,
    AssistantInterpretation,
    FormattingFailed,
    Failed,
}

impl From<TranscriptKind> for CsTranscriptKind {
    fn from(kind: TranscriptKind) -> Self {
        match kind {
            TranscriptKind::Raw => CsTranscriptKind::Raw,
            TranscriptKind::Cloud => CsTranscriptKind::Cloud,
            TranscriptKind::FormattedTranscript => CsTranscriptKind::FormattedTranscript,
            TranscriptKind::AssistantInterpretation => CsTranscriptKind::AssistantInterpretation,
            TranscriptKind::FormattingFailed => CsTranscriptKind::FormattingFailed,
            TranscriptKind::Failed => CsTranscriptKind::Failed,
        }
    }
}

/// One on-disk transcript artifact. Mirrors `HistoryEntry`
/// (`state/history.rs:20`); `path` is the absolute file path as a String and
/// `timestamp_ms` is epoch-millis from the entry's local timestamp.
#[derive(uniffi::Record)]
pub struct CsHistoryEntry {
    pub path: String,
    pub timestamp_ms: i64,
    pub preview: String,
    pub kind: CsTranscriptKind,
}

impl From<HistoryEntry> for CsHistoryEntry {
    fn from(entry: HistoryEntry) -> Self {
        Self {
            path: entry.path.to_string_lossy().into_owned(),
            timestamp_ms: entry.timestamp.timestamp_millis(),
            preview: entry.preview,
            kind: entry.kind.into(),
        }
    }
}

/// Thin handle to the codescribe thread store + transcript history.
///
/// Stateless: every call constructs a fresh `ThreadStore` / `ThreadIndex` over
/// the live on-disk data dir, so reads always reflect what the engine wrote.
#[derive(uniffi::Object)]
pub struct CodescribeThreads {}

#[uniffi::export]
impl CodescribeThreads {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self {}
    }

    /// List indexed thread summaries, newest first, optionally filtered.
    /// Wraps `ThreadIndex::list` (`thread_index.rs:195`).
    pub fn list_threads(
        &self,
        filter: Option<CsThreadFilter>,
    ) -> Result<Vec<CsThreadSummary>, CsError> {
        let index = open_index()?;
        let core_filter = filter.map(ThreadFilter::from);
        Ok(index
            .list(core_filter.as_ref())
            .into_iter()
            .map(CsThreadSummary::from)
            .collect())
    }

    /// Full-text search over indexed threads (all query words must match),
    /// newest first. Wraps `ThreadIndex::search` (`thread_index.rs:206`).
    pub fn search_threads(&self, query: String) -> Result<Vec<CsThreadSummary>, CsError> {
        let index = open_index()?;
        Ok(index
            .search(&query)
            .into_iter()
            .map(CsThreadSummary::from)
            .collect())
    }

    /// Load a full thread by id. Wraps `ThreadStore::load_thread`
    /// (`thread_store.rs:149`).
    pub fn load_thread(&self, id: String) -> Result<CsThread, CsError> {
        let store = ThreadStore::new()?;
        let thread = store.load_thread(&id)?;
        Ok(CsThread::from(&thread))
    }

    /// Delete a thread (file + index entry) by id. Wraps
    /// `ThreadStore::delete_thread` (`thread_store.rs:159`).
    pub fn delete_thread(&self, id: String) -> Result<(), CsError> {
        let store = ThreadStore::new()?;
        store.delete_thread(&id)?;
        Ok(())
    }

    /// Set a thread's favorite flag; returns `false` when no such thread exists.
    /// Wraps `ThreadStore::set_thread_favorite` (`thread_store.rs:173`).
    pub fn set_thread_favorite(&self, id: String, is_favorite: bool) -> Result<bool, CsError> {
        let store = ThreadStore::new()?;
        Ok(store.set_thread_favorite(&id, is_favorite)?)
    }

    /// Rename a thread. Marks the title as user-custom so auto-titling won't
    /// overwrite it on the next turn; returns `false` when no such thread exists.
    /// Wraps `ThreadStore::set_thread_title` (`thread_store.rs`).
    pub fn rename_thread(&self, id: String, title: String) -> Result<bool, CsError> {
        let store = ThreadStore::new()?;
        Ok(store.set_thread_title(&id, &title)?)
    }

    /// Generate a fresh, collision-resistant thread id. Wraps
    /// `ThreadStore::generate_id` (`thread_store.rs:191`).
    pub fn generate_thread_id(&self) -> String {
        ThreadStore::generate_id()
    }

    /// Export a thread as a Markdown transcript saved under
    /// `~/.codescribe/transcriptions/YYYY-MM-DD/`. Returns the absolute path of
    /// the written file. `assistant_only = true` keeps only assistant turns.
    /// Formatting lives in `codescribe_core::agent::thread_export` (unit-tested);
    /// this wrapper owns the on-disk placement + collision-avoidance, mirroring
    /// the legacy `save_chat_markdown_to_history` (removed in 37efe51).
    pub fn export_thread_markdown(
        &self,
        id: String,
        assistant_only: bool,
    ) -> Result<String, CsError> {
        let store = ThreadStore::new()?;
        let thread = store.load_thread(&id)?;
        let markdown = thread_to_markdown(&thread, assistant_only);

        let now = Local::now();
        let dir = history::transcriptions_dir(&now);
        let time_base = now.format("%H%M%S").to_string();
        let kind = if assistant_only {
            "chat-assistant"
        } else {
            "chat"
        };

        let mut candidate = dir.join(format!("{time_base}_{kind}.md"));
        for i in 1..=10_000 {
            if !candidate.exists() {
                break;
            }
            candidate = dir.join(format!("{time_base}_{kind}_{i}.md"));
        }

        fs::write(&candidate, markdown)?;
        Ok(candidate.to_string_lossy().into_owned())
    }

    /// Recent transcript history entries, newest first, capped at `limit`.
    /// Wraps `history::recent_entries` (`state/history.rs:404`).
    pub fn recent_history(&self, limit: u32) -> Vec<CsHistoryEntry> {
        history::recent_entries(limit as usize)
            .into_iter()
            .map(CsHistoryEntry::from)
            .collect()
    }

    /// Read the full text of a transcript artifact at `path`. Wraps
    /// `std::fs::read_to_string`.
    pub fn read_history_text(&self, path: String) -> Result<String, CsError> {
        Ok(fs::read_to_string(&path)?)
    }
}

/// Open the live thread index over the default on-disk data dir.
fn open_index() -> Result<ThreadIndex, CsError> {
    let store = ThreadStore::new()?;
    let index = ThreadIndex::load_or_create(store.threads_dir())?;
    Ok(index)
}

/// Flatten a message's structured content into a display string, mirroring the
/// private `collect_message_text` walk (`thread_index.rs:348`) but for the
/// *display* surface rather than the search index. Unlike the search preview we
/// must NOT collapse interior whitespace: the live stream hands the renderer raw
/// markdown, so a restored message has to keep its newlines or the block parser
/// sees one giant paragraph. We therefore preserve intra-block newlines and only
/// trim each block's outer edges, separating distinct content blocks with a
/// blank line so their markdown structure stays intact.
fn flatten_message_text(content: &[Value]) -> String {
    let mut chunks = Vec::new();
    for value in content {
        collect_message_text(value, &mut chunks);
    }
    chunks
        .iter()
        .map(|chunk| chunk.trim())
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Recursively collect human-readable text from a content `Value`, keyed on the
/// canonical content-block `type` (like `core::agent::thread_export::collect_text`)
/// rather than a blind key walk. A blind walk recurses into structural fields, so
/// a restored `image` block leaks its `media_type` ("image/png") and a `tool_use`
/// block leaks its `id`/`name` into the displayed transcript. A type-aware
/// whitelist emits only real prose: canonical `text` blocks, legacy
/// `input_text` / `output_text` aliases, plus recursed `tool_result` content.
/// Interior newlines are preserved (unlike the search-index twin) so restored
/// markdown keeps its structure.
fn collect_message_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        Value::Array(items) => {
            // Skip binary-like arrays (e.g., raw image bytes).
            if items.iter().all(Value::is_number) {
                return;
            }
            for item in items {
                collect_message_text(item, out);
            }
        }
        Value::Object(map) => match map.get("type").and_then(Value::as_str) {
            // A text block (or an untyped object treated as one) contributes its
            // `text`; structural fields (media_type, id, name, …) are ignored.
            Some("text") | Some("input_text") | Some("output_text") | None => {
                if let Some(text) = map.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    out.push(text.to_string());
                }
            }
            // Tool results carry nested display prose; recurse into it.
            Some("tool_result") => {
                if let Some(nested) = map.get("content") {
                    collect_message_text(nested, out);
                }
            }
            // tool_use / image / image_asset / anything else: no display prose.
            Some(_) => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: a message that streams as rich markdown must survive the
    /// structured-content -> display-text flatten with its newlines intact.
    /// Before the fix `split_whitespace().join(" ")` collapsed every `\n` into a
    /// space, so a restored thread rendered one giant paragraph.
    #[test]
    fn flatten_preserves_newlines_within_a_text_block() {
        let raw = "# Nagłówek H1\n\n## Nagłówek H2\n\n- punkt 1\n- punkt 2";
        let content = vec![json!({ "type": "text", "text": raw })];
        assert_eq!(flatten_message_text(&content), raw);
    }

    #[test]
    fn flatten_separates_distinct_text_blocks_with_a_blank_line() {
        let content = vec![
            json!({ "type": "text", "text": "Pierwszy akapit." }),
            json!({ "type": "text", "text": "Drugi akapit\nz nową linią." }),
        ];
        assert_eq!(
            flatten_message_text(&content),
            "Pierwszy akapit.\n\nDrugi akapit\nz nową linią."
        );
    }

    #[test]
    fn flatten_surfaces_openai_text_alias_blocks() {
        let content = vec![
            json!({ "type": "input_text", "text": "legacy prompt" }),
            json!({ "type": "output_text", "text": "legacy reply" }),
        ];
        assert_eq!(
            flatten_message_text(&content),
            "legacy prompt\n\nlegacy reply"
        );
    }

    #[test]
    fn flatten_skips_binary_arrays_and_blank_blocks() {
        let content = vec![
            json!({ "type": "text", "text": "widoczny" }),
            json!([1, 2, 3, 4]),
            json!({ "type": "text", "text": "   " }),
        ];
        assert_eq!(flatten_message_text(&content), "widoczny");
    }

    /// Regression: a restored `image` block must not leak its `media_type`
    /// ("image/png") and a `tool_use` block must not leak its `id`/`name` into
    /// the displayed transcript. The type-aware whitelist emits only real prose.
    #[test]
    fn flatten_skips_restored_image_and_tool_use_structural_fields() {
        let content = vec![
            json!({ "type": "text", "text": "opis zdjęcia" }),
            json!({
                "type": "image",
                "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" }
            }),
            json!({
                "type": "tool_use",
                "id": "toolu_42",
                "name": "grep",
                "input": { "pattern": "x" }
            }),
        ];
        assert_eq!(flatten_message_text(&content), "opis zdjęcia");
    }

    /// Nested `tool_result` prose still surfaces on the display surface.
    #[test]
    fn flatten_surfaces_tool_result_prose() {
        let content = vec![json!({
            "type": "tool_result",
            "tool_use_id": "toolu_42",
            "content": [ { "type": "text", "text": "wynik grep" } ]
        })];
        assert_eq!(flatten_message_text(&content), "wynik grep");
    }
}
