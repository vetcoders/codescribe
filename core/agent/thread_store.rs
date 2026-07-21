use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use directories::BaseDirs;
use rand::distributions::Alphanumeric;
use rand::{Rng, thread_rng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, warn};

use super::thread_index::ThreadIndex;
use super::types::{ContentBlock, Message, Role};

const THREADS_DIR_NAME: &str = "threads";
const BLOBS_DIR_NAME: &str = "blobs";
const THREAD_FILE_EXT: &str = "json";
const DEFAULT_THREAD_TITLE: &str = "Codescribe Agent Chat";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: String,
    /// True when the user renamed the thread by hand. Auto-titling (deriving a
    /// title from the first message) must never overwrite a custom title.
    /// `#[serde(default)]` keeps threads saved before this field deserializable
    /// (they default to auto-titled).
    #[serde(default)]
    pub title_is_custom: bool,
    /// True when the title came from the isolated one-shot title provider.
    /// Older thread JSON predates this ownership state and therefore migrates
    /// safely into the heuristic state (`false/false`).
    #[serde(default)]
    pub title_is_generated: bool,
    pub mode: String,
    pub tags: Vec<String>,
    pub notes: Vec<ThreadNote>,
    pub messages: Vec<ThreadMessage>,
    pub summary: Option<String>,
    pub total_tokens: Option<TokenUsage>,
    pub provider: String,
    pub model: String,
}

impl Thread {
    /// A title is heuristic only while neither durable owner has claimed it.
    /// Both persistence paths use this predicate before deriving a slug.
    pub fn title_is_heuristic(&self) -> bool {
        !self.title_is_custom && !self.title_is_generated
    }

    pub fn add_note(
        &mut self,
        text: impl Into<String>,
        anchored_to_message: Option<usize>,
    ) -> ThreadNote {
        let note = ThreadNote {
            id: generate_note_id(),
            created_at: Utc::now(),
            text: text.into(),
            anchored_to_message,
        };
        self.notes.push(note.clone());
        self.updated_at = Utc::now();
        note
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadMessage {
    pub role: String,
    pub content: Vec<Value>,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl From<&Message> for ThreadMessage {
    fn from(message: &Message) -> Self {
        let content = message
            .content
            .iter()
            .map(content_block_to_value)
            .collect::<Vec<_>>();

        Self {
            role: role_to_string(message.role).to_string(),
            content,
            timestamp: message.timestamp.unwrap_or_else(Utc::now),
            metadata: None,
        }
    }
}

impl ThreadMessage {
    pub fn to_message(&self) -> Message {
        let content = self
            .content
            .iter()
            .map(value_to_content_block)
            .collect::<Vec<_>>();

        Message {
            role: role_from_string(&self.role),
            content,
            timestamp: Some(self.timestamp),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadNote {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchored_to_message: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone)]
pub struct ThreadStore {
    threads_dir: PathBuf,
    blobs_dir: PathBuf,
}

impl ThreadStore {
    pub fn new() -> Result<Self> {
        let app_data = app_data_dir();
        Self::new_in(app_data.join(THREADS_DIR_NAME))
    }

    pub fn new_in<P: AsRef<Path>>(threads_dir: P) -> Result<Self> {
        let threads_dir = threads_dir.as_ref().to_path_buf();
        let blobs_dir = threads_dir.join(BLOBS_DIR_NAME);

        fs::create_dir_all(&threads_dir)
            .with_context(|| format!("Failed to create threads dir: {}", threads_dir.display()))?;
        fs::create_dir_all(&blobs_dir)
            .with_context(|| format!("Failed to create blobs dir: {}", blobs_dir.display()))?;

        Ok(Self {
            threads_dir,
            blobs_dir,
        })
    }

    pub fn save_thread(&self, thread: &Thread) -> Result<()> {
        // ThreadStore is the durable title boundary. Source-specific delivery
        // paths may hand us an assistive heredoc delimiter (`<<<`) as their
        // heuristic title; normalize it from the first user message before
        // either the thread JSON or its index projection can persist it.
        let normalized = if thread.title_is_heuristic() && !title_is_meaningful(&thread.title) {
            let mut normalized = thread.clone();
            normalize_heuristic_title(&mut normalized);
            Some(normalized)
        } else {
            None
        };
        let thread = normalized.as_ref().unwrap_or(thread);
        let path = self.thread_path(&thread.id)?;
        let json = serde_json::to_vec_pretty(thread).context("Failed to serialize thread JSON")?;
        atomic_write(&path, &json)?;

        let mut index = ThreadIndex::load_or_create(&self.threads_dir)?;
        index.add(thread)?;
        Ok(())
    }

    pub fn load_thread(&self, id: &str) -> Result<Thread> {
        let mut thread = self.load_thread_raw(id)?;
        if normalize_heuristic_title(&mut thread) {
            // Lazy migration for pre-0.13.0 files. A read remains usable even
            // on a read-only volume; the warning preserves the persistence
            // failure while the caller still receives a safe in-memory title.
            if let Err(error) = self.save_thread(&thread) {
                warn!(thread_id = %id, %error, "Failed to persist healed thread title");
            }
        }
        Ok(thread)
    }

    /// Heal legacy index rows whose heuristic title has no readable content.
    /// Returns the number of thread JSON + index entries repaired. Re-running
    /// is idempotent: once a row is healed it is no longer a candidate.
    pub fn heal_degenerate_titles(&self) -> Result<usize> {
        let index = ThreadIndex::load_or_create(&self.threads_dir)?;
        let candidate_ids = index
            .data()
            .threads
            .iter()
            .filter(|summary| !title_is_meaningful(&summary.title))
            .map(|summary| summary.id.clone())
            .collect::<Vec<_>>();

        let mut healed = 0;
        for id in candidate_ids {
            let result = (|| -> Result<bool> {
                let mut thread = self.load_thread_raw(&id)?;
                if !normalize_heuristic_title(&mut thread) {
                    return Ok(false);
                }
                self.save_thread(&thread)?;
                Ok(true)
            })();
            match result {
                Ok(true) => healed += 1,
                Ok(false) => {}
                Err(error) => {
                    // One corrupt/stale row must not make the entire thread
                    // rail disappear. Keep the row for the Swift fallback and
                    // preserve a diagnostic for repair.
                    warn!(thread_id = %id, %error, "Failed to heal legacy thread title");
                }
            }
        }
        Ok(healed)
    }

    fn load_thread_raw(&self, id: &str) -> Result<Thread> {
        let path = self.thread_path(id)?;
        let path = canonical_existing_child(&self.threads_dir, &path)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read thread file: {}", path.display()))?;
        serde_json::from_str::<Thread>(&raw)
            .with_context(|| format!("Failed to parse thread file: {}", path.display()))
    }

    pub fn delete_thread(&self, id: &str) -> Result<()> {
        let path = self.thread_path(id)?;
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove thread file: {}", path.display()))?;
            debug!("Removed thread file {}", path.display());
        }

        let mut index = ThreadIndex::load_or_create(&self.threads_dir)?;
        index.remove(id)?;
        Ok(())
    }

    pub fn set_thread_favorite(&self, id: &str, is_favorite: bool) -> Result<bool> {
        validate_thread_id(id)?;
        let mut index = ThreadIndex::load_or_create(&self.threads_dir)?;
        index.set_favorite(id, is_favorite)
    }

    /// Rename a thread and mark the title as user-custom so later auto-titling
    /// won't clobber it. Returns `false` when no such thread exists on disk.
    /// `updated_at` is left untouched so a rename does not reorder the rail.
    pub fn set_thread_title(&self, id: &str, title: &str) -> Result<bool> {
        let trimmed = title.trim();
        if !title_is_meaningful(trimmed) {
            bail!("Thread title must contain readable text");
        }
        let path = self.thread_path(id)?;
        if !path.exists() {
            return Ok(false);
        }
        let mut thread = self.load_thread(id)?;
        thread.title = trimmed.to_string();
        thread.title_is_custom = true;
        thread.title_is_generated = false;
        self.save_thread(&thread)?;
        Ok(true)
    }

    /// Persist an AI-generated title without stealing ownership from a manual
    /// rename. Returns `false` for a missing thread or a custom-owned title.
    /// `updated_at` stays unchanged so title completion cannot reorder the rail.
    pub fn set_generated_title(&self, id: &str, title: &str) -> Result<bool> {
        let trimmed = title.trim();
        if !title_is_meaningful(trimmed) {
            bail!("Generated thread title must contain readable text");
        }
        let path = self.thread_path(id)?;
        if !path.exists() {
            return Ok(false);
        }
        let mut thread = self.load_thread(id)?;
        if thread.title_is_custom {
            return Ok(false);
        }
        thread.title = trimmed.to_string();
        thread.title_is_custom = false;
        thread.title_is_generated = true;
        self.save_thread(&thread)?;
        Ok(true)
    }

    pub fn thread_file_path(&self, id: &str) -> Result<PathBuf> {
        self.thread_path(id)
    }

    pub fn save_blob(&self, data: &[u8], name: &str) -> Result<PathBuf> {
        let sanitized = sanitize_filename(name);
        let path = self.unique_blob_path(&sanitized);
        atomic_write(&path, data)?;
        Ok(path)
    }

    pub fn generate_id() -> String {
        format!("t_{}_{}", Utc::now().format("%Y-%m-%d"), random_suffix(6))
    }

    pub fn threads_dir(&self) -> &Path {
        &self.threads_dir
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    /// Build the on-disk path for a thread id.
    ///
    /// Validation lives here — at path *construction* — so every caller
    /// (current or future) that turns an id into a filesystem path is forced
    /// through `validate_thread_id`. This keeps the path-traversal guard
    /// adjacent to the join that produces the path, instead of relying on each
    /// API entry point to remember to validate first.
    fn thread_path(&self, id: &str) -> Result<PathBuf> {
        validate_thread_id(id)?;
        Ok(self.threads_dir.join(format!("{id}.{THREAD_FILE_EXT}")))
    }

    fn unique_blob_path(&self, file_name: &str) -> PathBuf {
        let candidate = self.blobs_dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }

        let path = Path::new(file_name);
        let stem = path
            .file_stem()
            .and_then(|part| part.to_str())
            .filter(|part| !part.is_empty())
            .unwrap_or("blob");
        let extension = path.extension().and_then(|part| part.to_str());

        for _ in 0..1024 {
            let suffix = random_suffix(4);
            let name = if let Some(ext) = extension {
                format!("{stem}_{suffix}.{ext}")
            } else {
                format!("{stem}_{suffix}")
            };
            let next = self.blobs_dir.join(name);
            if !next.exists() {
                return next;
            }
        }

        self.blobs_dir
            .join(format!("{}_{}.bin", stem, Utc::now().timestamp_millis()))
    }
}

/// A persisted title must carry at least one Unicode letter or number. This
/// rejects punctuation-only transport markers such as `<<<` while preserving
/// ordinary multilingual titles.
pub fn title_is_meaningful(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.starts_with("<<<") && trimmed.chars().any(char::is_alphanumeric)
}

fn normalize_heuristic_title(thread: &mut Thread) -> bool {
    if !thread.title_is_heuristic() || title_is_meaningful(&thread.title) {
        return false;
    }
    thread.title = derive_title_from_messages(&thread.messages);
    true
}

fn derive_title_from_messages(messages: &[ThreadMessage]) -> String {
    let Some(first_user) = messages
        .iter()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
    else {
        return DEFAULT_THREAD_TITLE.to_string();
    };

    let mut chunks = Vec::new();
    for value in &first_user.content {
        collect_title_text(value, &mut chunks);
    }
    for chunk in chunks {
        for line in chunk.lines() {
            if let Some(title) = normalize_title_line(line) {
                return title;
            }
        }
    }
    DEFAULT_THREAD_TITLE.to_string()
}

fn collect_title_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.trim().is_empty() => out.push(text.to_string()),
        Value::Array(items) if !items.iter().all(Value::is_number) => {
            for item in items {
                collect_title_text(item, out);
            }
        }
        Value::Object(map) => match map.get("type").and_then(Value::as_str) {
            Some("text") | Some("input_text") | Some("output_text") | None => {
                if let Some(text) = map.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    out.push(text.to_string());
                }
            }
            Some("tool_result") => {
                if let Some(content) = map.get("content") {
                    collect_title_text(content, out);
                }
            }
            Some(_) => {}
        },
        _ => {}
    }
}

fn normalize_title_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !title_is_meaningful(trimmed) || is_assistive_wire_label(trimmed) {
        return None;
    }
    let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped = normalized.chars().take(72).collect::<String>();
    title_is_meaningful(&clipped).then_some(clipped)
}

fn is_assistive_wire_label(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        // Canonical English skeleton labels (build_assistive_input).
        "user_instruction:",
        "user instruction:",
        "selected_text:",
        "selected text:",
        "context:",
        // Legacy Polish labels — threads persisted before the EN rename.
        "instrukcja_użytkownika:",
        "instrukcja użytkownika:",
        "zaznaczony_tekst:",
        "zaznaczony tekst:",
        "kontekst:",
        "kontekst_aplikacji:",
        "kontekst aplikacji:",
        "aplikacja:",
        "okno:",
        "system prompt",
        "you are an agent",
        "jesteś agentem",
        "jestes agentem",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn app_data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
        return PathBuf::from(shellexpand::tilde(&custom).into_owned());
    }

    BaseDirs::new()
        .map(|dirs| dirs.data_dir().join("Codescribe"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join("Library/Application Support/Codescribe")
        })
}

fn sanitize_filename(name: &str) -> String {
    let raw = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("blob.bin");

    let mut out = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if out.trim_matches('_').is_empty() {
        out = "blob.bin".to_string();
    }

    if out.len() > 180 {
        out.truncate(180);
    }

    out
}

fn validate_thread_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("Thread id cannot be empty");
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        bail!("Thread id contains invalid path characters: {id}");
    }
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
            "Thread path escaped threads dir: {} outside {}",
            path.display(),
            base.display()
        );
    }
    Ok(path)
}

fn role_to_string(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

fn role_from_string(value: &str) -> Role {
    match value.to_ascii_lowercase().as_str() {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        _ => Role::User,
    }
}

fn content_block_to_value(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(text) => json!({
            "type": "text",
            "text": text,
        }),
        // Inline images (composer attachments) are spilled to the shared
        // agent asset store so they survive the persist/restore roundtrip —
        // the JSON thread file itself never carries image bytes. Empty blocks
        // (restored from pre-asset thread files) and failed spills keep the
        // explicit `data_omitted` marker instead of minting an empty asset.
        ContentBlock::Image { data, media_type } => {
            let data_omitted = || {
                json!({
                    "type": "image",
                    "media_type": media_type,
                    "size_bytes": data.len(),
                    "data_omitted": true,
                })
            };
            if data.is_empty() {
                return data_omitted();
            }
            match crate::agent::AgentAssetStore::save_inline_image(data, media_type) {
                Ok(asset) => json!({
                    "type": "image_asset",
                    "asset_id": asset.asset_id,
                    "path": asset.path,
                    "media_type": asset.media_type,
                    "size_bytes": asset.size_bytes,
                }),
                Err(error) => {
                    warn!("Failed to persist inline image as disk-backed asset: {error}");
                    data_omitted()
                }
            }
        }
        ContentBlock::ImageAsset(asset) => json!({
            "type": "image_asset",
            "asset_id": asset.asset_id,
            "path": asset.path,
            "media_type": asset.media_type,
            "size_bytes": asset.size_bytes,
        }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content.iter().map(content_block_to_value).collect::<Vec<_>>(),
            "is_error": is_error,
        }),
    }
}

fn value_to_content_block(value: &Value) -> ContentBlock {
    let Some(value_type) = value.get("type").and_then(Value::as_str) else {
        return ContentBlock::Text(value.to_string());
    };

    match value_type {
        "text" | "input_text" | "output_text" => ContentBlock::Text(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "image" => {
            let media_type = value
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            ContentBlock::Image {
                data: Vec::new(),
                media_type,
            }
        }
        "image_asset" => {
            let asset = crate::agent::ImageAsset {
                asset_id: value
                    .get("asset_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                path: value
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                media_type: value
                    .get("media_type")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream")
                    .to_string(),
                size_bytes: value
                    .get("size_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            };
            ContentBlock::ImageAsset(asset)
        }
        "tool_use" => ContentBlock::ToolUse {
            id: value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_tool")
                .to_string(),
            input: value.get("input").cloned().unwrap_or_else(|| json!({})),
        },
        "tool_result" => {
            let nested = value
                .get("content")
                .and_then(Value::as_array)
                .map(|items| items.iter().map(value_to_content_block).collect::<Vec<_>>())
                .unwrap_or_default();
            ContentBlock::ToolResult {
                tool_use_id: value
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                content: nested,
                is_error: value
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }
        }
        _ => ContentBlock::Text(value.to_string()),
    }
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

fn random_suffix(len: usize) -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn generate_note_id() -> String {
    format!("n_{}_{}", Utc::now().format("%Y-%m-%d"), random_suffix(6))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use chrono::Duration;
    use serde_json::json;
    use std::collections::HashSet;
    use tempfile::TempDir;

    use super::*;

    fn sample_thread(id: String, updated_at: DateTime<Utc>) -> Thread {
        Thread {
            id,
            created_at: updated_at - Duration::minutes(10),
            updated_at,
            title: "Parvo patient follow-up".to_string(),
            title_is_custom: false,
            title_is_generated: false,
            mode: "assistive".to_string(),
            tags: vec!["urgent".to_string(), "canine".to_string()],
            notes: vec![ThreadNote {
                id: "n_2026-02-23_abcd12".to_string(),
                created_at: updated_at,
                text: "Call owner after bloodwork".to_string(),
                anchored_to_message: Some(0),
            }],
            messages: vec![
                ThreadMessage {
                    role: "user".to_string(),
                    content: vec![json!({"type":"input_text","text":"Summarize latest labs"})],
                    timestamp: updated_at - Duration::minutes(1),
                    metadata: Some(json!({"source":"voice"})),
                },
                ThreadMessage {
                    role: "assistant".to_string(),
                    content: vec![json!({"type":"output_text","text":"WBC improved."})],
                    timestamp: updated_at,
                    metadata: None,
                },
            ],
            summary: Some("Dog improving after IV fluids.".to_string()),
            total_tokens: Some(TokenUsage {
                input: 234,
                output: 145,
            }),
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        }
    }

    #[test]
    fn round_trip_save_and_load_thread() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());

        store.save_thread(&thread)?;
        let loaded = store.load_thread(&thread.id)?;
        assert_eq!(loaded, thread);

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        assert_eq!(index.data().threads.len(), 1);
        assert_eq!(index.data().threads[0].id, thread.id);

        Ok(())
    }

    #[test]
    fn save_replaces_assistive_delimiter_title_before_json_and_index_persist() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let mut thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        thread.title = "<<<".to_string();
        thread.messages[0].content = vec![json!({
            "type": "input_text",
            "text": "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\nPrzygotuj plan wypisu pacjenta\n>\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"
        })];

        store.save_thread(&thread)?;

        let persisted: Thread =
            serde_json::from_str(&fs::read_to_string(store.thread_file_path(&thread.id)?)?)?;
        assert_eq!(persisted.title, "Przygotuj plan wypisu pacjenta");
        assert!(persisted.title_is_heuristic());

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        assert_eq!(index.data().threads[0].title, persisted.title);
        Ok(())
    }

    #[test]
    fn assistive_wire_labels_are_skipped_in_both_label_languages() {
        // Canonical EN labels (current wires) and legacy PL labels (threads
        // already on disk) must BOTH stay out of derived titles.
        for label in [
            "USER_INSTRUCTION:",
            "SELECTED_TEXT: no selection available.",
            "SELECTED_TEXT: carried in <codescribe_context>.",
            "CONTEXT:",
            "INSTRUKCJA_UŻYTKOWNIKA:",
            "ZAZNACZONY_TEKST: brak dostępnego zaznaczenia.",
            "KONTEKST:",
        ] {
            assert!(
                is_assistive_wire_label(label),
                "label must be skip-listed: {label}"
            );
            assert_eq!(
                normalize_title_line(label),
                None,
                "label must never become a title: {label}"
            );
        }
        assert!(!is_assistive_wire_label("Plan wypisu pacjenta"));
        assert_eq!(
            normalize_title_line("Plan wypisu pacjenta").as_deref(),
            Some("Plan wypisu pacjenta")
        );
    }

    #[test]
    fn save_replaces_assistive_delimiter_title_for_english_wire() -> Result<()> {
        // EN mirror of the PL fixture above: the canonical wire produced after
        // the label rename must yield the instruction as the derived title.
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let mut thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        thread.title = "<<<".to_string();
        thread.messages[0].content = vec![json!({
            "type": "input_text",
            "text": "USER_INSTRUCTION:\n<<<\nPrzygotuj plan wypisu pacjenta\n>\n\nSELECTED_TEXT: no selection available.\n"
        })];

        store.save_thread(&thread)?;

        let persisted: Thread =
            serde_json::from_str(&fs::read_to_string(store.thread_file_path(&thread.id)?)?)?;
        assert_eq!(persisted.title, "Przygotuj plan wypisu pacjenta");
        assert!(persisted.title_is_heuristic());
        Ok(())
    }

    #[test]
    fn legacy_delimiter_fixture_heals_file_index_and_export_idempotently() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let mut thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        thread.messages[0].content = vec![json!({
            "type": "text",
            "text": "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\nCompare insulin protocols\n>\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"
        })];
        store.save_thread(&thread)?;

        // Emulate a pre-fix 0.13.0 artifact: both the source JSON and its
        // denormalized index row carry the heredoc delimiter as title.
        let thread_path = store.thread_file_path(&thread.id)?;
        let mut thread_json: Value = serde_json::from_str(&fs::read_to_string(&thread_path)?)?;
        thread_json["title"] = json!("<<<");
        atomic_write(&thread_path, &serde_json::to_vec_pretty(&thread_json)?)?;

        let index_path = store.threads_dir().join("index.json");
        let mut index_json: Value = serde_json::from_str(&fs::read_to_string(&index_path)?)?;
        index_json["threads"][0]["title"] = json!("<<<");
        atomic_write(&index_path, &serde_json::to_vec_pretty(&index_json)?)?;

        assert_eq!(store.heal_degenerate_titles()?, 1);
        let healed = store.load_thread(&thread.id)?;
        assert_eq!(healed.title, "Compare insulin protocols");
        let healed_index = ThreadIndex::load_or_create(store.threads_dir())?;
        assert_eq!(healed_index.data().threads[0].title, healed.title);

        let markdown = crate::agent::thread_export::thread_to_markdown(&healed, false);
        assert!(markdown.starts_with("# Compare insulin protocols"));
        assert!(!markdown.contains("# <<<"));
        assert_eq!(store.heal_degenerate_titles()?, 0);
        Ok(())
    }

    #[test]
    fn legacy_openai_text_aliases_restore_as_plain_text() {
        let message = ThreadMessage {
            role: "assistant".to_string(),
            content: vec![
                json!({"type":"input_text","text":"Owner asked about appetite"}),
                json!({"type":"output_text","text":"Appetite improved overnight"}),
            ],
            timestamp: Utc::now(),
            metadata: None,
        }
        .to_message();

        assert_eq!(message.content.len(), 2);
        match &message.content[0] {
            ContentBlock::Text(text) => assert_eq!(text, "Owner asked about appetite"),
            other => panic!("input_text restored as unexpected block: {other:?}"),
        }
        match &message.content[1] {
            ContentBlock::Text(text) => assert_eq!(text, "Appetite improved overnight"),
            other => panic!("output_text restored as unexpected block: {other:?}"),
        }
    }

    #[test]
    fn delete_removes_thread_and_index_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());

        store.save_thread(&thread)?;
        store.delete_thread(&thread.id)?;

        let path = store.thread_path(&thread.id)?;
        assert!(!path.exists());

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        assert!(index.data().threads.is_empty());
        Ok(())
    }

    #[test]
    fn save_blob_writes_binary_data() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let png_header = [137, 80, 78, 71, 13, 10, 26, 10];

        let path = store.save_blob(&png_header, "../screenshot?.png")?;
        assert!(path.starts_with(store.blobs_dir()));
        assert_eq!(fs::read(&path)?, png_header);
        Ok(())
    }

    #[test]
    fn generated_ids_are_unique() {
        let mut seen = HashSet::new();
        for _ in 0..512 {
            let id = ThreadStore::generate_id();
            assert!(seen.insert(id), "duplicate thread id generated");
        }
    }

    #[test]
    fn add_note_supports_optional_message_anchor() {
        let mut thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let note = thread.add_note("Verify appetite tomorrow", Some(1));

        assert_eq!(note.anchored_to_message, Some(1));
        assert!(thread.notes.iter().any(|value| value.id == note.id));
    }

    #[test]
    fn save_thread_updates_index_search_results() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let id = ThreadStore::generate_id();

        let mut thread = sample_thread(id.clone(), Utc::now() - Duration::minutes(30));
        thread.title = "Dermatology intake".to_string();
        thread.summary = Some("initial allergy note".to_string());
        store.save_thread(&thread)?;

        thread.updated_at = Utc::now();
        thread.title = "Dermatology urgent handoff".to_string();
        thread.summary = Some("cat urgent follow-up tomorrow".to_string());
        store.save_thread(&thread)?;

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        assert_eq!(
            index.data().threads.len(),
            1,
            "save should upsert by thread id"
        );

        let urgent_results = index.search("cat urgent");
        assert_eq!(urgent_results.len(), 1);
        assert_eq!(urgent_results[0].id, id);

        let stale_results = index.search("initial allergy");
        assert!(
            stale_results.is_empty(),
            "stale pre-update summary should not remain searchable"
        );

        Ok(())
    }

    #[test]
    fn set_thread_favorite_updates_index_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let id = thread.id.clone();
        store.save_thread(&thread)?;

        let updated = store.set_thread_favorite(&id, true)?;
        assert!(updated);

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        let summary = index
            .data()
            .threads
            .iter()
            .find(|value| value.id == id)
            .expect("summary should exist");
        assert!(summary.is_favorite);

        Ok(())
    }

    #[test]
    fn set_thread_title_marks_custom_and_persists() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let id = thread.id.clone();
        store.save_thread(&thread)?;

        let renamed = store.set_thread_title(&id, "  Custom name  ")?;
        assert!(renamed);

        let loaded = store.load_thread(&id)?;
        assert_eq!(loaded.title, "Custom name");
        assert!(
            loaded.title_is_custom,
            "rename marks the title as user-custom"
        );
        assert!(
            !loaded.title_is_generated,
            "manual rename clears generated ownership"
        );

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        let summary = index
            .data()
            .threads
            .iter()
            .find(|value| value.id == id)
            .expect("summary should exist");
        assert_eq!(summary.title, "Custom name", "index reflects renamed title");

        assert!(
            store.set_thread_title(&id, "   ").is_err(),
            "empty title is rejected"
        );
        assert!(
            !store.set_thread_title("t_2026-01-01_missing", "x")?,
            "renaming an absent thread returns false"
        );
        Ok(())
    }

    #[test]
    fn legacy_thread_json_defaults_generated_ownership_to_false() -> Result<()> {
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let mut value = serde_json::to_value(&thread)?;
        value
            .as_object_mut()
            .expect("thread serializes as an object")
            .remove("title_is_generated");

        let loaded: Thread = serde_json::from_value(value)?;
        assert!(!loaded.title_is_generated);
        assert!(loaded.title_is_heuristic());
        Ok(())
    }

    #[test]
    fn generated_and_custom_title_transitions_are_exclusive() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let id = thread.id.clone();
        let original_updated_at = thread.updated_at;
        assert!(!thread.title_is_custom);
        assert!(!thread.title_is_generated);
        assert!(thread.title_is_heuristic());
        store.save_thread(&thread)?;

        assert!(store.set_generated_title(&id, "  Analiza wyników Łatki  ")?);
        let generated = store.load_thread(&id)?;
        assert_eq!(generated.title, "Analiza wyników Łatki");
        assert!(!generated.title_is_custom);
        assert!(generated.title_is_generated);
        assert!(!generated.title_is_heuristic());
        assert_eq!(generated.updated_at, original_updated_at);
        let generated_index = ThreadIndex::load_or_create(store.threads_dir())?;
        let generated_summary = generated_index
            .data()
            .threads
            .iter()
            .find(|value| value.id == id)
            .expect("generated summary should exist");
        assert_eq!(generated_summary.title, "Analiza wyników Łatki");

        assert!(store.set_thread_title(&id, "Plan właściciela")?);
        let custom = store.load_thread(&id)?;
        assert_eq!(custom.title, "Plan właściciela");
        assert!(custom.title_is_custom);
        assert!(!custom.title_is_generated);
        assert!(!custom.title_is_heuristic());
        assert_eq!(custom.updated_at, original_updated_at);
        Ok(())
    }

    #[test]
    fn generated_title_rejects_blank_missing_and_custom_without_mutation() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        let id = thread.id.clone();
        store.save_thread(&thread)?;

        assert!(store.set_generated_title(&id, " \n\t ").is_err());
        assert!(store.set_generated_title(&id, "<<<").is_err());
        assert!(store.set_generated_title(&id, "<<< 2026-07-20").is_err());
        assert!(store.set_thread_title(&id, "<<<").is_err());
        assert!(!store.set_generated_title("t_2026-01-01_missing", "Generated")?);

        assert!(store.set_thread_title(&id, "Custom authority")?);
        let before = store.load_thread(&id)?;
        assert!(!store.set_generated_title(&id, "Generated overwrite")?);
        let after = store.load_thread(&id)?;
        assert_eq!(
            after, before,
            "custom content and ownership must be preserved"
        );

        let index = ThreadIndex::load_or_create(store.threads_dir())?;
        let summary = index
            .data()
            .threads
            .iter()
            .find(|value| value.id == id)
            .expect("summary should exist");
        assert_eq!(summary.title, "Custom authority");
        Ok(())
    }

    #[test]
    fn inline_image_roundtrips_through_disk_backed_asset() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let image_bytes = format!("w5a-inline-roundtrip-bytes-{}", std::process::id()).into_bytes();

        let message = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text("look at this".to_string()),
                ContentBlock::Image {
                    data: image_bytes.clone(),
                    media_type: "image/png".to_string(),
                },
            ],
            timestamp: Some(Utc::now()),
        };

        let mut thread = sample_thread(ThreadStore::generate_id(), Utc::now());
        thread.messages = vec![ThreadMessage::from(&message)];
        store.save_thread(&thread)?;

        let restored = store.load_thread(&thread.id)?.messages[0].to_message();
        let ContentBlock::ImageAsset(asset) = &restored.content[1] else {
            panic!(
                "inline image should restore as a disk-backed asset, got: {:?}",
                restored.content[1]
            );
        };
        let data = crate::agent::AgentAssetStore::read_image(&asset.path)?;
        assert_eq!(
            data, image_bytes,
            "restored asset bytes must match the original image"
        );
        assert_eq!(asset.size_bytes, image_bytes.len() as u64);
        assert_eq!(asset.media_type, "image/png");

        // The persisted thread JSON must not carry raw image bytes.
        let raw = fs::read_to_string(store.thread_file_path(&thread.id)?)?;
        assert!(raw.contains("image_asset"));
        assert!(!raw.contains("data_omitted"));

        fs::remove_file(&asset.path).ok();
        Ok(())
    }

    #[test]
    fn inline_image_asset_is_written_once_across_saves() -> Result<()> {
        let block = ContentBlock::Image {
            data: b"w5a-dedup-bytes".to_vec(),
            media_type: "image/png".to_string(),
        };

        let first = content_block_to_value(&block);
        let path = PathBuf::from(
            first
                .get("path")
                .and_then(Value::as_str)
                .expect("persisted inline image should carry an asset path"),
        );
        assert!(path.exists(), "first persist must write the asset file");

        // Simulate a pre-existing asset: if the second persist rewrote the
        // file, the sentinel would be clobbered.
        fs::write(&path, b"sentinel")?;
        let second = content_block_to_value(&block);
        assert_eq!(
            first, second,
            "same bytes must map to the same asset across saves"
        );
        assert_eq!(
            fs::read(&path)?,
            b"sentinel",
            "existing asset must be referenced, not rewritten"
        );

        fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn legacy_data_omitted_image_restores_without_bytes_and_repersists_safely() {
        let legacy = json!({
            "type": "image",
            "media_type": "image/png",
            "size_bytes": 123,
            "data_omitted": true,
        });

        let block = value_to_content_block(&legacy);
        let ContentBlock::Image { data, media_type } = &block else {
            panic!("legacy image value should restore as an image block: {block:?}");
        };
        assert!(data.is_empty(), "legacy blocks carry no bytes by design");
        assert_eq!(media_type, "image/png");

        // Re-persisting a byteless block keeps the explicit degraded marker
        // instead of minting an empty asset.
        let repersisted = content_block_to_value(&block);
        assert_eq!(
            repersisted.get("data_omitted").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            repersisted.get("type").and_then(Value::as_str),
            Some("image")
        );
    }

    #[test]
    fn thread_file_path_validates_id() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let id = ThreadStore::generate_id();
        let path = store.thread_file_path(&id)?;
        assert!(path.ends_with(format!("{id}.json")));
        assert!(store.thread_file_path("../bad").is_err());
        Ok(())
    }
}
