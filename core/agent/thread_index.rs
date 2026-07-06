use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use super::thread_store::{Thread, ThreadMessage};

const INDEX_FILE_NAME: &str = "index.json";
const INDEX_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadIndexData {
    pub version: u32,
    pub threads: Vec<ThreadSummary>,
}

impl Default for ThreadIndexData {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            threads: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    pub mode: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub has_notes: bool,
    #[serde(default)]
    pub latest_message: Option<String>,
    #[serde(default)]
    pub latest_note: Option<String>,
    #[serde(default)]
    pub search_text: String,
    pub is_favorite: bool,
}

impl ThreadSummary {
    fn from_thread(thread: &Thread, is_favorite: bool) -> Self {
        let latest_message = thread
            .messages
            .iter()
            .rev()
            .find_map(thread_message_preview_text);
        let latest_note = thread
            .notes
            .iter()
            .rev()
            .map(|note| normalize_snippet(&note.text))
            .find(|note| !note.is_empty());
        let search_text =
            build_search_text(thread, latest_note.as_deref(), latest_message.as_deref());

        Self {
            id: thread.id.clone(),
            title: thread.title.clone(),
            created_at: thread.created_at,
            updated_at: thread.updated_at,
            message_count: thread.messages.len(),
            mode: thread.mode.clone(),
            tags: thread.tags.clone(),
            summary: thread.summary.clone(),
            has_notes: !thread.notes.is_empty(),
            latest_message,
            latest_note,
            search_text,
            is_favorite,
        }
    }

    fn searchable_text(&self) -> Cow<'_, str> {
        if !self.search_text.is_empty() {
            return Cow::Borrowed(&self.search_text);
        }

        let mut out = String::with_capacity(
            self.title.len()
                + self.mode.len()
                + self.tags.iter().map(String::len).sum::<usize>()
                + self.summary.as_ref().map_or(0, String::len)
                + 8,
        );

        out.push_str(&self.title.to_ascii_lowercase());
        out.push(' ');
        out.push_str(&self.mode.to_ascii_lowercase());
        out.push(' ');
        for tag in &self.tags {
            out.push_str(&tag.to_ascii_lowercase());
            out.push(' ');
        }
        if let Some(summary) = &self.summary {
            out.push_str(&summary.to_ascii_lowercase());
        }
        Cow::Owned(out)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    pub mode: Option<String>,
    pub favorites_only: bool,
    pub has_notes: bool,
    pub tag: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThreadIndex {
    path: PathBuf,
    data: ThreadIndexData,
}

impl ThreadIndex {
    pub fn load_or_create(threads_dir: &Path) -> Result<Self> {
        fs::create_dir_all(threads_dir).with_context(|| {
            format!(
                "Failed to create threads directory: {}",
                threads_dir.display()
            )
        })?;

        // `path` joins a compile-time constant filename (`INDEX_FILE_NAME`) onto
        // the store-owned `threads_dir`. No caller-supplied component reaches it,
        // so there is no path-traversal source to taint.
        let path = threads_dir.join(INDEX_FILE_NAME);
        if path.exists() {
            let path = canonical_existing_child(threads_dir, &path)?;
            let mut raw = String::new();
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- path is canonicalized and checked to stay under threads_dir immediately above.
            fs::File::open(&path)
                .with_context(|| format!("Failed to open thread index: {}", path.display()))?
                .read_to_string(&mut raw)
                .with_context(|| format!("Failed to read thread index: {}", path.display()))?;
            let mut data = serde_json::from_str::<ThreadIndexData>(&raw)
                .with_context(|| format!("Failed to parse thread index: {}", path.display()))?;
            if data.version < INDEX_VERSION {
                let rebuild_dir = path.parent().unwrap_or(threads_dir);
                data = rebuild_index_from_threads(rebuild_dir, &data)?;
                let index = Self { path, data };
                index.save()?;
                return Ok(index);
            }
            return Ok(Self { path, data });
        }

        let index = Self {
            path,
            data: ThreadIndexData::default(),
        };
        index.save()?;
        Ok(index)
    }

    pub fn add(&mut self, thread: &Thread) -> Result<()> {
        match self
            .data
            .threads
            .iter_mut()
            .find(|summary| summary.id == thread.id)
        {
            Some(existing) => {
                let is_favorite = existing.is_favorite;
                *existing = ThreadSummary::from_thread(thread, is_favorite);
            }
            None => self
                .data
                .threads
                .push(ThreadSummary::from_thread(thread, false)),
        }
        sort_by_updated_desc(&mut self.data.threads);
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.data.threads.retain(|summary| summary.id != id);
        self.save()
    }

    pub fn set_favorite(&mut self, id: &str, is_favorite: bool) -> Result<bool> {
        let Some(entry) = self
            .data
            .threads
            .iter_mut()
            .find(|summary| summary.id == id)
        else {
            return Ok(false);
        };

        if entry.is_favorite == is_favorite {
            return Ok(true);
        }

        entry.is_favorite = is_favorite;
        self.save()?;
        Ok(true)
    }

    pub fn list(&self, filter: Option<&ThreadFilter>) -> Vec<&ThreadSummary> {
        let mut entries = self
            .data
            .threads
            .iter()
            .filter(|summary| filter.is_none_or(|f| matches_filter(summary, f)))
            .collect::<Vec<_>>();
        entries.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        entries
    }

    pub fn search(&self, query: &str) -> Vec<&ThreadSummary> {
        let terms = normalize_terms(query);
        if terms.is_empty() {
            return self.list(None);
        }

        let mut entries = self
            .data
            .threads
            .iter()
            .filter(|summary| {
                let haystack = summary.searchable_text();
                terms.iter().all(|term| haystack.contains(term))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        entries
    }

    pub fn save(&self) -> Result<()> {
        let data =
            serde_json::to_vec_pretty(&self.data).context("Failed to serialize thread index")?;
        atomic_write(&self.path, &data)
    }

    pub fn data(&self) -> &ThreadIndexData {
        &self.data
    }
}

fn sort_by_updated_desc(entries: &mut [ThreadSummary]) {
    entries.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
}

fn normalize_terms(query: &str) -> Vec<String> {
    query
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn build_search_text(
    thread: &Thread,
    latest_note: Option<&str>,
    latest_message: Option<&str>,
) -> String {
    const MAX_SEARCH_TEXT_BYTES: usize = 16_384;
    let mut out = String::with_capacity(1024);
    append_search_chunk(&mut out, &thread.title, MAX_SEARCH_TEXT_BYTES);
    append_search_chunk(&mut out, &thread.mode, MAX_SEARCH_TEXT_BYTES);
    append_search_chunk(&mut out, &thread.tags.join(" "), MAX_SEARCH_TEXT_BYTES);

    if let Some(summary) = &thread.summary {
        append_search_chunk(&mut out, summary, MAX_SEARCH_TEXT_BYTES);
    }

    if let Some(note) = latest_note {
        append_search_chunk(&mut out, note, MAX_SEARCH_TEXT_BYTES);
    }
    if let Some(message) = latest_message {
        append_search_chunk(&mut out, message, MAX_SEARCH_TEXT_BYTES);
    }

    for note in &thread.notes {
        append_search_chunk(&mut out, &note.text, MAX_SEARCH_TEXT_BYTES);
        if out.len() >= MAX_SEARCH_TEXT_BYTES {
            break;
        }
    }
    if out.len() < MAX_SEARCH_TEXT_BYTES {
        for message in &thread.messages {
            if let Some(text) = thread_message_preview_text(message) {
                append_search_chunk(&mut out, &text, MAX_SEARCH_TEXT_BYTES);
            }
            if out.len() >= MAX_SEARCH_TEXT_BYTES {
                break;
            }
        }
    }

    out
}

fn rebuild_index_from_threads(
    threads_dir: &Path,
    existing: &ThreadIndexData,
) -> Result<ThreadIndexData> {
    let favorites_by_id = existing
        .threads
        .iter()
        .map(|summary| (summary.id.clone(), summary.is_favorite))
        .collect::<HashMap<_, _>>();
    let mut threads = Vec::new();

    // `threads_dir` is store-owned: the only caller passes `path.parent()` of the
    // index file, which itself is `threads_dir.join(INDEX_FILE_NAME)`. No
    // caller-supplied component reaches it, so there is no path-traversal source to taint.
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- threads_dir is store-owned (derived from the store-owned index path), not caller-supplied.
    for entry in fs::read_dir(threads_dir).with_context(|| {
        format!(
            "Failed to read threads directory: {}",
            threads_dir.display()
        )
    })? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !is_thread_json_file(&path) {
            continue;
        }

        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(thread) = serde_json::from_str::<Thread>(&raw) else {
            continue;
        };
        let is_favorite = favorites_by_id.get(&thread.id).copied().unwrap_or(false);
        threads.push(ThreadSummary::from_thread(&thread, is_favorite));
    }

    sort_by_updated_desc(&mut threads);
    Ok(ThreadIndexData {
        version: INDEX_VERSION,
        threads,
    })
}

fn is_thread_json_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("t_"))
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn append_search_chunk(out: &mut String, value: &str, max_len: usize) {
    if value.is_empty() || out.len() >= max_len {
        return;
    }
    let normalized = normalize_snippet(value);
    if normalized.is_empty() {
        return;
    }
    let separator_len = usize::from(!out.is_empty());
    let remaining = max_len.saturating_sub(out.len() + separator_len);
    let prefix_len = grapheme_prefix_len(&normalized, remaining);
    if prefix_len == 0 {
        return;
    }
    if separator_len > 0 {
        out.push(' ');
    }
    out.push_str(&normalized[..prefix_len]);
}

fn normalize_snippet(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn grapheme_prefix_len(value: &str, max_len: usize) -> usize {
    if value.len() <= max_len {
        return value.len();
    }
    let mut boundary = 0;
    for (idx, grapheme) in value.grapheme_indices(true) {
        let end = idx + grapheme.len();
        if end > max_len {
            break;
        }
        boundary = end;
    }
    boundary
}

fn thread_message_preview_text(message: &ThreadMessage) -> Option<String> {
    let mut chunks = Vec::new();
    for value in &message.content {
        collect_message_text(value, &mut chunks);
    }
    let joined = chunks.join(" ");
    let normalized = normalize_snippet(&joined);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Collect preview/search prose from stored thread content. Only text-like
/// blocks (`text`, plus legacy `input_text` / `output_text`) contribute their
/// `text`; `tool_result` recurses into nested content, and structural fields are
/// never walked blindly.
fn collect_message_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        serde_json::Value::Array(items) => {
            // Skip binary-like arrays (e.g., image bytes).
            if items.iter().all(serde_json::Value::is_number) {
                return;
            }
            for item in items {
                collect_message_text(item, out);
            }
        }
        serde_json::Value::Object(map) => match map.get("type").and_then(serde_json::Value::as_str)
        {
            Some("text") | Some("input_text") | Some("output_text") | None => {
                if let Some(text) = map.get("text").and_then(serde_json::Value::as_str)
                    && !text.trim().is_empty()
                {
                    out.push(text.to_string());
                }
            }
            Some("tool_result") => {
                if let Some(content) = map.get("content") {
                    collect_message_text(content, out);
                }
            }
            Some(_) => {}
        },
        _ => {}
    }
}

fn matches_filter(summary: &ThreadSummary, filter: &ThreadFilter) -> bool {
    if let Some(mode) = &filter.mode
        && !summary.mode.eq_ignore_ascii_case(mode)
    {
        return false;
    }

    if filter.favorites_only && !summary.is_favorite {
        return false;
    }

    if filter.has_notes && !summary.has_notes {
        return false;
    }

    if let Some(tag) = &filter.tag
        && !summary
            .tags
            .iter()
            .any(|value| value.eq_ignore_ascii_case(tag))
    {
        return false;
    }

    true
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory for {}", path.display()))?;
    }

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)
        .with_context(|| format!("Failed to write temporary file {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to atomically rename {} -> {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn canonical_existing_child(base: &Path, path: &Path) -> Result<PathBuf> {
    let base = base
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize base dir: {}", base.display()))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize file path: {}", path.display()))?;
    if !path.starts_with(&base) {
        bail!(
            "Thread index path escaped threads dir: {} outside {}",
            path.display(),
            base.display()
        );
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use chrono::Duration;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::agent::thread_store::{Thread, ThreadMessage, TokenUsage};

    fn sample_thread(
        id: &str,
        title: &str,
        summary: Option<&str>,
        mode: &str,
        minutes_ago: i64,
    ) -> Thread {
        let updated_at = Utc::now() - Duration::minutes(minutes_ago);
        Thread {
            id: id.to_string(),
            created_at: updated_at - Duration::minutes(5),
            updated_at,
            title: title.to_string(),
            title_is_custom: false,
            mode: mode.to_string(),
            tags: vec!["vet".to_string(), "urgent".to_string()],
            notes: Vec::new(),
            messages: vec![ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"input_text","text":"hello"})],
                timestamp: updated_at,
                metadata: None,
            }],
            summary: summary.map(ToOwned::to_owned),
            total_tokens: Some(TokenUsage {
                input: 10,
                output: 20,
            }),
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        }
    }

    #[test]
    fn search_matches_all_words_and_sorts_by_latest() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let first = sample_thread(
            "t_2026-02-22_aaaaaa",
            "Cat urgent follow-up",
            Some("Prednisone taper plan"),
            "assistive",
            30,
        );
        let second = sample_thread(
            "t_2026-02-23_bbbbbb",
            "Urgent dermatology handoff",
            Some("Cat allergy response"),
            "assistive",
            5,
        );
        let third = sample_thread(
            "t_2026-02-23_cccccc",
            "Billing question",
            Some("No clinical content"),
            "toggle",
            1,
        );

        index.add(&first)?;
        index.add(&second)?;
        index.add(&third)?;

        let results = index.search("cat urgent");
        let ids = results
            .iter()
            .map(|summary| summary.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["t_2026-02-23_bbbbbb", "t_2026-02-22_aaaaaa"]);

        Ok(())
    }

    #[test]
    fn list_applies_filters() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let mut a = sample_thread(
            "t_2026-02-23_filter1",
            "Case A",
            Some("alpha"),
            "assistive",
            10,
        );
        a.notes.push(crate::agent::thread_store::ThreadNote {
            id: "n_1".to_string(),
            created_at: Utc::now(),
            text: "Pinned".to_string(),
            anchored_to_message: Some(0),
        });

        let b = sample_thread("t_2026-02-23_filter2", "Case B", Some("beta"), "toggle", 2);

        index.add(&a)?;
        index.add(&b)?;
        index.set_favorite("t_2026-02-23_filter1", true)?;

        let filter = ThreadFilter {
            mode: Some("assistive".to_string()),
            favorites_only: true,
            has_notes: true,
            tag: Some("urgent".to_string()),
        };
        let results = index.list(Some(&filter));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "t_2026-02-23_filter1");

        Ok(())
    }

    #[test]
    fn search_includes_message_and_note_text() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let mut thread = sample_thread(
            "t_2026-02-23_note_search",
            "Canine follow-up",
            Some("Clinical recap"),
            "assistive",
            3,
        );
        thread.messages.push(ThreadMessage {
            role: "assistant".to_string(),
            content: vec![json!({"type":"output_text","text":"Kidney panel improved"})],
            timestamp: Utc::now(),
            metadata: None,
        });
        thread.notes.push(crate::agent::thread_store::ThreadNote {
            id: "n_2".to_string(),
            created_at: Utc::now(),
            text: "Call owner about kidney values".to_string(),
            anchored_to_message: Some(1),
        });
        index.add(&thread)?;

        let message_results = index.search("kidney panel");
        assert_eq!(message_results.len(), 1);
        assert_eq!(message_results[0].id, "t_2026-02-23_note_search");

        let note_results = index.search("call owner kidney");
        assert_eq!(note_results.len(), 1);
        assert_eq!(note_results[0].id, "t_2026-02-23_note_search");

        Ok(())
    }

    #[test]
    fn legacy_openai_text_aliases_feed_preview_and_search_without_type_leak() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;

        let mut thread = sample_thread(
            "t_2026-07-05_legacy_aliases",
            "Legacy aliases",
            None,
            "assistive",
            1,
        );
        thread.messages = vec![
            ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"input_text","text":"Owner asked about appetite"})],
                timestamp: Utc::now(),
                metadata: None,
            },
            ThreadMessage {
                role: "assistant".to_string(),
                content: vec![
                    json!({"type":"tool_use","id":"toolu_1","name":"search_threads","input":{"query":"output_text"}}),
                    json!({"type":"output_text","text":"Appetite improved overnight"}),
                ],
                timestamp: Utc::now(),
                metadata: None,
            },
        ];
        index.add(&thread)?;

        let summary = index
            .list(None)
            .into_iter()
            .find(|summary| summary.id == "t_2026-07-05_legacy_aliases")
            .expect("thread summary should be indexed");
        assert_eq!(
            summary.latest_message.as_deref(),
            Some("appetite improved overnight")
        );
        assert!(summary.search_text.contains("owner asked about appetite"));
        assert!(summary.search_text.contains("appetite improved overnight"));
        assert!(!summary.search_text.contains("input_text"));
        assert!(!summary.search_text.contains("output_text"));

        let results = index.search("appetite improved");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "t_2026-07-05_legacy_aliases");

        Ok(())
    }

    #[test]
    fn load_migrates_legacy_index_summaries_from_thread_files() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut thread = sample_thread(
            "t_2026-07-05_legacy_rebuild",
            "Legacy rebuild",
            None,
            "assistive",
            1,
        );
        thread.messages = vec![
            ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"input_text","text":"Owner reports appetite"})],
                timestamp: Utc::now(),
                metadata: None,
            },
            ThreadMessage {
                role: "assistant".to_string(),
                content: vec![json!({"type":"output_text","text":"Appetite improved overnight"})],
                timestamp: Utc::now(),
                metadata: None,
            },
        ];
        fs::write(
            tmp.path().join(format!("{}.json", thread.id)),
            serde_json::to_vec_pretty(&thread)?,
        )?;
        fs::write(tmp.path().join("t_2026-07-05_broken.json"), "{not-json")?;

        let stale_index = ThreadIndexData {
            version: 1,
            threads: vec![ThreadSummary {
                id: thread.id.clone(),
                title: thread.title.clone(),
                created_at: thread.created_at,
                updated_at: thread.updated_at,
                message_count: thread.messages.len(),
                mode: thread.mode.clone(),
                tags: thread.tags.clone(),
                summary: None,
                has_notes: false,
                latest_message: Some("ai failed output_text".to_string()),
                latest_note: None,
                search_text: "ai failed output_text".to_string(),
                is_favorite: true,
            }],
        };
        fs::write(
            tmp.path().join(INDEX_FILE_NAME),
            serde_json::to_vec_pretty(&stale_index)?,
        )?;

        let index = ThreadIndex::load_or_create(tmp.path())?;
        assert_eq!(index.data().version, INDEX_VERSION);
        assert_eq!(index.data().threads.len(), 1);
        let summary = &index.data().threads[0];
        assert_eq!(summary.id, thread.id);
        assert!(summary.is_favorite);
        assert_eq!(
            summary.latest_message.as_deref(),
            Some("appetite improved overnight")
        );
        assert!(summary.search_text.contains("owner reports appetite"));
        assert!(summary.search_text.contains("appetite improved overnight"));
        assert!(!summary.search_text.contains("input_text"));
        assert!(!summary.search_text.contains("output_text"));
        assert!(!summary.search_text.contains("ai failed output_text"));

        let reloaded = ThreadIndex::load_or_create(tmp.path())?;
        assert_eq!(reloaded.data().version, INDEX_VERSION);
        assert!(reloaded.data().threads[0].is_favorite);

        Ok(())
    }

    #[test]
    fn append_search_chunk_respects_grapheme_boundary_at_limit() {
        let mut out = "x".repeat(16_382);
        append_search_chunk(&mut out, "ęcho", 16_384);
        assert_eq!(out.len(), 16_382);

        let mut out = "x".repeat(16_381);
        append_search_chunk(&mut out, "ęcho", 16_384);
        assert_eq!(out.len(), 16_384);
        assert!(out.ends_with(" ę"));

        let mut out = "x".repeat(16_364);
        append_search_chunk(&mut out, "zażółć gęślą jaźń też", 16_384);
        assert!(out.ends_with(" zażółć gęślą"));

        let zalgo = "A͙̒̍͢l̠̗̅ͩ͜t̷̝͖̐ͤ͜h͓̉͠o̵̯ͨͭů̷͈͚ͤg̸̺͚ͯͩ͡ȟ̶̩ 𝙻̥͐͏͖̓𝚘̸̙̗ͥͮ͝𝚌͍̈͢𝚝̴̱͑ͤ𝚛̸͍͔ͣ́𝚎̳́҉̙̎͢𝚎̥̄͏";
        let first_grapheme_len = zalgo.graphemes(true).next().expect("grapheme").len();

        let mut out = String::new();
        append_search_chunk(&mut out, zalgo, first_grapheme_len - 1);
        assert!(out.is_empty());

        append_search_chunk(&mut out, zalgo, first_grapheme_len);
        assert_eq!(out, "a͙̒̍͢");

        let zero_width_payload = format!(
            "{}{}",
            "gаdz𝒊оlоrt",
            "\u{200b}\u{200c}\u{200d}".repeat(10_000)
        );
        let mut out = String::new();
        append_search_chunk(&mut out, &zero_width_payload, 16_384);
        assert!(out.starts_with("gаdz𝒊оlоrt"));
        assert!(out.len() <= 16_384);
    }

    #[test]
    fn set_favorite_persists_to_disk() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut index = ThreadIndex::load_or_create(tmp.path())?;
        let thread = sample_thread("t_2026-02-23_fav", "Case", Some("alpha"), "assistive", 1);
        index.add(&thread)?;

        let updated = index.set_favorite("t_2026-02-23_fav", true)?;
        assert!(updated);

        let reloaded = ThreadIndex::load_or_create(tmp.path())?;
        let reloaded_entry = reloaded
            .data()
            .threads
            .iter()
            .find(|summary| summary.id == "t_2026-02-23_fav")
            .expect("entry should exist");
        assert!(reloaded_entry.is_favorite);

        Ok(())
    }
}
