//! Native thread-search tool (canonical: `search_threads`).
//!
//! Read-only lookup over the local thread index — titles, tags, summaries,
//! notes, and message text. Every match carries a snippet anchored on the first
//! query term, so the agent can judge relevance without opening the thread.

use anyhow::{Context, Result};
use codescribe_core::agent::{
    ThreadIndex, ThreadStore, ThreadSummary, ToolDefinition, ToolRegistry, ToolResultContent,
    ToolRisk,
};
use serde::Serialize;
use serde_json::{Value, json};

/// Matches returned when the caller omits `limit`.
const DEFAULT_LIMIT: usize = 5;
/// Upper bound on `limit`, enforced by clamping rather than rejection.
const MAX_LIMIT: usize = 20;
/// Characters of context carried in each match snippet.
const SNIPPET_CHARS: usize = 320;

/// Register the read-only `search_threads` tool on the shared [`ToolRegistry`].
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            search_threads_definition(),
            Box::new(|input| Box::pin(handle_search_threads(input))),
            ToolRisk::ReadOnly,
        )
        .expect("register search_threads tool");
}

/// Tool schema for `search_threads`.
fn search_threads_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_threads".to_string(),
        description: "Search the local Codescribe thread index by title, tags, summary, notes, and message text. Read-only; returns top matching thread ids and snippets.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. All words must match the indexed thread corpus."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "default": DEFAULT_LIMIT,
                    "description": "Maximum number of matches to return."
                }
            },
            "required": ["query"]
        }),
    }
}

/// Dispatch adapter for `search_threads`: turns a [`Result`] into tool content.
async fn handle_search_threads(input: Value) -> Vec<ToolResultContent> {
    match search_threads_from_input(&input) {
        Ok(output) => vec![ToolResultContent::Text(output)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Validate the request, run it against the live index, and render pretty JSON.
///
/// `query` must be present and non-blank; `limit` is clamped into
/// `1..=MAX_LIMIT` rather than rejected.
fn search_threads_from_input(input: &Value) -> Result<String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("Missing required non-empty string field 'query'")?;
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|raw| usize::try_from(raw).ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    let store = ThreadStore::new().context("Failed to initialize ThreadStore")?;
    let index =
        ThreadIndex::load_or_create(store.threads_dir()).context("Failed to load thread index")?;
    let matches = search_index(&index, query, limit);
    serde_json::to_string_pretty(&SearchThreadsOutput {
        query: query.to_string(),
        count: matches.len(),
        matches,
    })
    .context("Failed to serialize search_threads output")
}

/// Query an index and project the top `limit` summaries into tool output shape.
fn search_index(index: &ThreadIndex, query: &str, limit: usize) -> Vec<SearchThreadMatch> {
    index
        .search(query)
        .into_iter()
        .take(limit)
        .map(|summary| SearchThreadMatch::from_summary(summary, query))
        .collect()
}

/// Serialized envelope returned to the agent.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct SearchThreadsOutput {
    /// Query as it was actually run (trimmed).
    query: String,
    /// Number of matches in `matches` — already limit-bounded.
    count: usize,
    /// Matches in index-ranked order.
    matches: Vec<SearchThreadMatch>,
}

/// One matching thread, flattened for the agent.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct SearchThreadMatch {
    /// Thread id, usable to open the thread.
    id: String,
    /// Display title.
    title: String,
    /// Last update, RFC 3339.
    updated_at: String,
    /// Thread mode (e.g. `assistive`).
    mode: String,
    /// Thread tags.
    tags: Vec<String>,
    /// Query-anchored excerpt of the indexed corpus.
    snippet: String,
    /// Stored summary, when one exists.
    summary: Option<String>,
    /// Most recent message text, when one exists.
    latest_message: Option<String>,
    /// Most recent note text, when one exists.
    latest_note: Option<String>,
}

impl SearchThreadMatch {
    /// Project an indexed summary into output shape, computing its snippet.
    fn from_summary(summary: &ThreadSummary, query: &str) -> Self {
        Self {
            id: summary.id.clone(),
            title: summary.title.clone(),
            updated_at: summary.updated_at.to_rfc3339(),
            mode: summary.mode.clone(),
            tags: summary.tags.clone(),
            snippet: snippet_for_summary(summary, query),
            summary: summary.summary.clone(),
            latest_message: summary.latest_message.clone(),
            latest_note: summary.latest_note.clone(),
        }
    }
}

/// Build a whitespace-normalized excerpt centred on the query.
///
/// Concatenates title, summary, latest note, latest message, and the indexed
/// search text, then starts ~24 characters before the first occurrence of the
/// query's first term (or at the beginning when it is absent).
fn snippet_for_summary(summary: &ThreadSummary, query: &str) -> String {
    let haystack = [
        summary.title.as_str(),
        summary.summary.as_deref().unwrap_or(""),
        summary.latest_note.as_deref().unwrap_or(""),
        summary.latest_message.as_deref().unwrap_or(""),
        summary.search_text.as_str(),
    ]
    .join(" ");
    let normalized = haystack.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }

    let query_anchor = query
        .split_whitespace()
        .next()
        .map(|term| term.to_ascii_lowercase())
        .unwrap_or_default();
    let lower = normalized.to_ascii_lowercase();
    let start = if query_anchor.is_empty() {
        0
    } else {
        lower.find(&query_anchor).unwrap_or(0)
    };
    let prefix = start.saturating_sub(24);
    normalized
        .chars()
        .skip(prefix)
        .take(SNIPPET_CHARS)
        .collect::<String>()
}

/// Covers ranking, the limit, and snippet anchoring over a temp index.
#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use codescribe_core::agent::{Thread, ThreadMessage, TokenUsage};

    use super::*;

    /// Build an indexable thread with one assistant message.
    fn sample_thread(
        id: &str,
        title: &str,
        summary: &str,
        message_text: &str,
        minutes_ago: i64,
    ) -> Thread {
        let updated_at = Utc::now() - Duration::minutes(minutes_ago);
        Thread {
            id: id.to_string(),
            created_at: updated_at - Duration::minutes(5),
            updated_at,
            title: title.to_string(),
            title_is_custom: false,
            title_is_generated: false,
            mode: "assistive".to_string(),
            tags: vec!["clinic".to_string()],
            notes: Vec::new(),
            messages: vec![ThreadMessage {
                role: "assistant".to_string(),
                content: vec![json!({"type":"output_text","text": message_text})],
                timestamp: updated_at,
                metadata: None,
            }],
            summary: Some(summary.to_string()),
            total_tokens: Some(TokenUsage {
                input: 10,
                output: 20,
            }),
            provider: "openai-responses".to_string(),
            model: "gpt-5".to_string(),
        }
    }

    /// Search index returns a capped, JSON-serializable match list for the agent tool.
    #[test]
    fn search_index_returns_limited_json_ready_matches() {
        let tmp = tempfile::TempDir::new().expect("temp dir should be created");
        let mut index =
            ThreadIndex::load_or_create(tmp.path()).expect("thread index should initialize");
        index
            .add(&sample_thread(
                "t_2026-06-11_a",
                "Renal follow-up",
                "Creatinine improved",
                "owner asked about renal diet",
                1,
            ))
            .expect("first thread should index");
        index
            .add(&sample_thread(
                "t_2026-06-11_b",
                "Billing",
                "Invoice question",
                "client needs emailed invoice",
                2,
            ))
            .expect("second thread should index");

        let matches = search_index(&index, "renal", 3);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "t_2026-06-11_a");
        assert!(matches[0].snippet.contains("Renal"));
    }
}
