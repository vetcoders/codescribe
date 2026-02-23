use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use directories::BaseDirs;
use rand::distributions::Alphanumeric;
use rand::{Rng, thread_rng};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use super::thread_index::ThreadIndex;
use super::types::{ContentBlock, Message, Role};

const THREADS_DIR_NAME: &str = "threads";
const BLOBS_DIR_NAME: &str = "blobs";
const THREAD_FILE_EXT: &str = "json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: String,
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
            .map(|block| match serde_json::to_value(block) {
                Ok(value) => value,
                Err(err) => Value::String(format!("[content_serialize_error:{err}]")),
            })
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
            .map(|value| {
                serde_json::from_value::<ContentBlock>(value.clone())
                    .unwrap_or_else(|_| ContentBlock::Text(value.to_string()))
            })
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
        validate_thread_id(&thread.id)?;
        let path = self.thread_path(&thread.id);
        let json = serde_json::to_vec_pretty(thread).context("Failed to serialize thread JSON")?;
        atomic_write(&path, &json)?;

        let mut index = ThreadIndex::load_or_create(&self.threads_dir)?;
        index.add(thread)?;
        Ok(())
    }

    pub fn load_thread(&self, id: &str) -> Result<Thread> {
        validate_thread_id(id)?;
        let path = self.thread_path(id);
        let raw = fs::read_to_string(&path) // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
            .with_context(|| format!("Failed to read thread file: {}", path.display()))?;
        let thread = serde_json::from_str::<Thread>(&raw)
            .with_context(|| format!("Failed to parse thread file: {}", path.display()))?;
        Ok(thread)
    }

    pub fn delete_thread(&self, id: &str) -> Result<()> {
        validate_thread_id(id)?;
        let path = self.thread_path(id);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove thread file: {}", path.display()))?;
            debug!("Removed thread file {}", path.display());
        }

        let mut index = ThreadIndex::load_or_create(&self.threads_dir)?;
        index.remove(id)?;
        Ok(())
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

    fn thread_path(&self, id: &str) -> PathBuf {
        self.threads_dir.join(format!("{id}.{THREAD_FILE_EXT}"))
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

fn app_data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
        return PathBuf::from(shellexpand::tilde(&custom).into_owned());
    }

    BaseDirs::new()
        .map(|dirs| dirs.data_dir().join("CodeScribe"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join("Library/Application Support/CodeScribe")
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
    fn delete_removes_thread_and_index_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let store = ThreadStore::new_in(tmp.path().join("threads"))?;
        let thread = sample_thread(ThreadStore::generate_id(), Utc::now());

        store.save_thread(&thread)?;
        store.delete_thread(&thread.id)?;

        let path = store.thread_path(&thread.id);
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
}
