//! Canonical persistence boundary for completed agent turns.
//!
//! Source-specific send paths construct [`ThreadDeliveryInput`]. This gateway
//! alone owns the durable load/create, title/summary projection, timestamped
//! upsert, and receipt contract over [`ThreadStore`].

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{ContentBlock, Message, Role, Thread, ThreadMessage, ThreadStore};

const DEFAULT_THREAD_TITLE: &str = "Codescribe Agent Chat";

/// Completed-turn origin. It is intentionally a core delivery concept rather
/// than UI state: callers use it for lifecycle evidence without logging content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDeliverySource {
    VoiceAssistive,
    Composer,
    LegacyFallback,
}

impl ThreadDeliverySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VoiceAssistive => "voice-assistive",
            Self::Composer => "composer",
            Self::LegacyFallback => "legacy-fallback",
        }
    }
}

/// Provider-agnostic durable state for one completed agent thread delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDeliveryInput {
    pub backend_id: String,
    pub messages: Vec<ThreadMessage>,
    pub provider: String,
    pub model: String,
    pub source: ThreadDeliverySource,
    pub mode: String,
    pub tags: Vec<String>,
    pub timestamp: DateTime<Utc>,
}

/// Durable proof returned only after the thread JSON and index upsert succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDeliveryReceipt {
    pub backend_id: String,
    pub created: bool,
    pub message_count: usize,
    pub updated_at: DateTime<Utc>,
    /// This delivery introduced the first completed user/assistant exchange.
    pub first_exchange: bool,
    /// The first exchange can launch isolated title generation. Custom and
    /// already-generated titles are never eligible.
    pub title_eligible: bool,
}

#[derive(Debug, Clone)]
pub struct ThreadDeliveryGateway {
    store: ThreadStore,
}

impl ThreadDeliveryGateway {
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: ThreadStore::new().context("Failed to initialize ThreadStore")?,
        })
    }

    pub fn new_in<P: AsRef<Path>>(threads_dir: P) -> Result<Self> {
        Ok(Self {
            store: ThreadStore::new_in(threads_dir)?,
        })
    }

    pub fn deliver(&self, input: ThreadDeliveryInput) -> Result<ThreadDeliveryReceipt> {
        let ThreadDeliveryInput {
            backend_id,
            messages,
            provider,
            model,
            source,
            mode,
            tags,
            timestamp,
        } = input;

        let path = self.store.thread_file_path(&backend_id)?;
        let existing =
            if path.exists() {
                Some(self.store.load_thread(&backend_id).with_context(|| {
                    format!("Failed to load existing agent thread {backend_id}")
                })?)
            } else {
                None
            };
        let created = existing.is_none();
        let previous_had_exchange = existing
            .as_ref()
            .is_some_and(|thread| has_completed_exchange(&thread.messages));
        let current_has_exchange = has_completed_exchange(&messages);
        let first_exchange = current_has_exchange && !previous_had_exchange;

        let canonical_messages = messages
            .iter()
            .map(ThreadMessage::to_message)
            .collect::<Vec<_>>();
        let mut thread = existing.unwrap_or_else(|| Thread {
            id: backend_id.clone(),
            created_at: timestamp,
            updated_at: timestamp,
            title: DEFAULT_THREAD_TITLE.to_string(),
            title_is_custom: false,
            title_is_generated: false,
            mode: mode.clone(),
            tags: tags.clone(),
            notes: Vec::new(),
            messages: Vec::new(),
            summary: None,
            total_tokens: None,
            provider: provider.clone(),
            model: model.clone(),
        });

        thread.updated_at = timestamp;
        if thread.title_is_heuristic() {
            thread.title = derive_thread_title(&canonical_messages);
        }
        thread.summary = derive_thread_summary(&canonical_messages);
        thread.messages = messages;
        thread.provider = provider;
        thread.model = model;
        thread.mode = mode;
        thread.tags = tags;

        let title_eligible = first_exchange && thread.title_is_heuristic();
        let message_count = thread.messages.len();
        self.store
            .save_thread(&thread)
            .with_context(|| format!("Failed to deliver agent thread {backend_id}"))?;

        tracing::debug!(
            backend_thread_id = %backend_id,
            source = source.as_str(),
            created,
            message_count,
            first_exchange,
            title_eligible,
            "Agent thread delivery persisted"
        );

        Ok(ThreadDeliveryReceipt {
            backend_id,
            created,
            message_count,
            updated_at: timestamp,
            first_exchange,
            title_eligible,
        })
    }
}

fn has_completed_exchange(messages: &[ThreadMessage]) -> bool {
    let Some(first_user) = messages
        .iter()
        .position(|message| message.role.eq_ignore_ascii_case("user"))
    else {
        return false;
    };

    messages[first_user + 1..]
        .iter()
        .any(|message| message.role.eq_ignore_ascii_case("assistant"))
}

/// First user message, boilerplate-stripped and clipped to the rail title cap.
fn derive_thread_title(messages: &[Message]) -> String {
    let first_user = messages.iter().find(|message| message.role == Role::User);
    let candidate = first_user
        .and_then(raw_text_from_message)
        .and_then(|raw| strip_boilerplate_title(&raw))
        .or_else(|| first_user.and_then(extract_text_from_message))
        .unwrap_or_else(|| DEFAULT_THREAD_TITLE.to_string());

    let mut title = candidate.chars().take(72).collect::<String>();
    if title.trim().is_empty() {
        title = DEFAULT_THREAD_TITLE.to_string();
    }
    title
}

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

fn raw_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let text = out.join("\n");
    (!text.trim().is_empty()).then_some(text)
}

fn extract_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let normalized = out
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn extract_text_from_block(block: &ContentBlock, out: &mut Vec<String>) {
    match block {
        ContentBlock::Text(text) if !text.trim().is_empty() => out.push(text.to_string()),
        ContentBlock::ToolResult { content, .. } => {
            for nested in content {
                extract_text_from_block(nested, out);
            }
        }
        _ => {}
    }
}

const BOILERPLATE_LINE_PREFIXES: &[&str] = &[
    "instrukcja",
    "instruction",
    "jesteś agentem",
    "jestes agentem",
    "you are an agent",
    "system prompt",
    "system:",
];

fn strip_boilerplate_title(raw: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_boilerplate_line(trimmed) {
            return None;
        }
        let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        (!normalized.is_empty()).then_some(normalized)
    })
}

fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    BOILERPLATE_LINE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || is_all_caps_header(line)
}

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

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::agent::{ThreadIndex, ThreadIndexData, ThreadNote, TokenUsage};

    fn timestamp(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 19, hour, 0, 0)
            .single()
            .expect("fixed timestamp should be valid")
    }

    fn message(role: &str, text: &str, at: DateTime<Utc>) -> ThreadMessage {
        ThreadMessage {
            role: role.to_string(),
            content: vec![json!({"type":"text","text":text})],
            timestamp: at,
            metadata: None,
        }
    }

    fn input(
        backend_id: &str,
        source: ThreadDeliverySource,
        messages: Vec<ThreadMessage>,
        at: DateTime<Utc>,
    ) -> ThreadDeliveryInput {
        ThreadDeliveryInput {
            backend_id: backend_id.to_string(),
            messages,
            provider: "openai-responses".to_string(),
            model: "gpt-test".to_string(),
            source,
            mode: "assistive".to_string(),
            tags: vec!["agent".to_string(), "overlay".to_string()],
            timestamp: at,
        }
    }

    fn exchange(at: DateTime<Utc>, user: &str, assistant: &str) -> Vec<ThreadMessage> {
        vec![
            message("user", user, at),
            message("assistant", assistant, at),
        ]
    }

    fn assert_single_thread_artifacts(threads_dir: &Path) -> Result<()> {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- test-only directory is created by TempDir and initialized by ThreadDeliveryGateway/ThreadStore before inspection.
        let thread_files = fs::read_dir(threads_dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
                    && path.file_name().is_some_and(|name| name != "index.json")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thread_files.len(),
            1,
            "same backend id must leave exactly one thread JSON artifact"
        );

        let index = ThreadIndex::load_or_create(threads_dir)?;
        assert_eq!(
            index.data().threads.len(),
            1,
            "same backend id must leave exactly one index row"
        );
        Ok(())
    }

    #[test]
    fn thread_delivery_input_constructs_voice_and_composer_sources() {
        let at = timestamp(8);
        let voice = input(
            "t_2026-07-19_shared",
            ThreadDeliverySource::VoiceAssistive,
            exchange(at, "voice question", "voice answer"),
            at,
        );
        let composer = input(
            "t_2026-07-19_shared",
            ThreadDeliverySource::Composer,
            exchange(at, "typed question", "typed answer"),
            at,
        );

        assert_eq!(voice.backend_id, composer.backend_id);
        assert_eq!(voice.source.as_str(), "voice-assistive");
        assert_eq!(composer.source.as_str(), "composer");
        assert_eq!(voice.mode, "assistive");
        assert_eq!(composer.tags, vec!["agent", "overlay"]);
    }

    #[test]
    fn thread_delivery_upserts_voice_and_composer_into_one_json_and_index_row() -> Result<()> {
        let tmp = TempDir::new()?;
        let threads_dir = tmp.path().join("threads");
        let gateway = ThreadDeliveryGateway::new_in(&threads_dir)?;
        let backend_id = "t_2026-07-19_shared";
        let first_at = timestamp(8);
        let second_at = timestamp(9);

        let first = gateway.deliver(input(
            backend_id,
            ThreadDeliverySource::VoiceAssistive,
            exchange(first_at, "First question", "First answer"),
            first_at,
        ))?;
        assert!(first.created);
        assert_eq!(first.message_count, 2);
        assert_eq!(first.updated_at, first_at);
        assert!(first.first_exchange);
        assert!(first.title_eligible);

        let mut growing = exchange(first_at, "First question", "First answer");
        growing.extend(exchange(second_at, "Second question", "Second answer"));
        let second = gateway.deliver(input(
            backend_id,
            ThreadDeliverySource::Composer,
            growing,
            second_at,
        ))?;
        assert!(!second.created);
        assert_eq!(second.message_count, 4);
        assert_eq!(second.updated_at, second_at);
        assert!(!second.first_exchange);
        assert!(!second.title_eligible);

        assert_single_thread_artifacts(&threads_dir)?;
        let thread_path = threads_dir.join(format!("{backend_id}.json"));
        let thread_json = fs::read_to_string(&thread_path)?;
        let persisted: Thread = serde_json::from_str(&thread_json)?;
        let index_json = fs::read_to_string(threads_dir.join("index.json"))?;
        let index: ThreadIndexData = serde_json::from_str(&index_json)?;
        assert_eq!(persisted.id, backend_id);
        assert_eq!(persisted.messages.len(), 4);
        assert_eq!(persisted.updated_at, second_at);
        assert_eq!(index.threads[0].id, backend_id);
        assert_eq!(index.threads[0].message_count, 4);
        assert_eq!(index.threads[0].updated_at, second_at);
        println!(
            "thread_delivery_artifacts thread={} index_rows={} messages={} updated_at={}",
            thread_path.display(),
            index.threads.len(),
            persisted.messages.len(),
            persisted.updated_at
        );
        Ok(())
    }

    #[test]
    #[should_panic(expected = "same backend id must leave exactly one thread JSON artifact")]
    fn thread_delivery_verifier_rejects_a_different_second_backend_id() {
        let tmp = TempDir::new().expect("temp dir should initialize");
        let threads_dir = tmp.path().join("threads");
        let gateway =
            ThreadDeliveryGateway::new_in(&threads_dir).expect("gateway should initialize");
        let at = timestamp(10);
        gateway
            .deliver(input(
                "t_2026-07-19_first",
                ThreadDeliverySource::VoiceAssistive,
                exchange(at, "one", "one reply"),
                at,
            ))
            .expect("first delivery should succeed");
        gateway
            .deliver(input(
                "t_2026-07-19_wrong-second-id",
                ThreadDeliverySource::Composer,
                exchange(at, "two", "two reply"),
                at,
            ))
            .expect("second delivery should succeed independently");

        assert_single_thread_artifacts(&threads_dir)
            .expect("artifact verifier should reject the identity split");
    }

    #[test]
    fn thread_delivery_preserves_custom_title_and_disables_title_eligibility() -> Result<()> {
        let tmp = TempDir::new()?;
        let threads_dir = tmp.path().join("threads");
        let store = ThreadStore::new_in(&threads_dir)?;
        let gateway = ThreadDeliveryGateway::new_in(&threads_dir)?;
        let backend_id = "t_2026-07-19_custom";
        let at = timestamp(11);
        store.save_thread(&Thread {
            id: backend_id.to_string(),
            created_at: at,
            updated_at: at,
            title: "Manual authority".to_string(),
            title_is_custom: true,
            title_is_generated: false,
            mode: "assistive".to_string(),
            tags: vec!["agent".to_string()],
            notes: Vec::<ThreadNote>::new(),
            messages: Vec::new(),
            summary: None,
            total_tokens: None::<TokenUsage>,
            provider: "old-provider".to_string(),
            model: "old-model".to_string(),
        })?;

        let receipt = gateway.deliver(input(
            backend_id,
            ThreadDeliverySource::Composer,
            exchange(at, "A title candidate", "An answer"),
            at,
        ))?;
        let persisted = store.load_thread(backend_id)?;
        assert!(!receipt.created);
        assert!(receipt.first_exchange);
        assert!(!receipt.title_eligible);
        assert_eq!(persisted.title, "Manual authority");
        assert!(persisted.title_is_custom);
        assert!(!persisted.title_is_generated);
        Ok(())
    }

    #[test]
    fn thread_delivery_title_skips_boilerplate_preamble() {
        let message = Message {
            role: Role::User,
            content: vec![ContentBlock::Text(
                "INSTRUKCJA UŻYTKOWNIKA: JESTEŚ AGENTEM\n\nNapraw hang na starcie sesji"
                    .to_string(),
            )],
            timestamp: None,
        };
        assert_eq!(
            derive_thread_title(&[message]),
            "Napraw hang na starcie sesji"
        );
    }

    #[test]
    fn thread_delivery_title_keeps_plain_first_line() {
        let message = Message {
            role: Role::User,
            content: vec![ContentBlock::Text(
                "Fix the rate limiter double-fire".to_string(),
            )],
            timestamp: None,
        };
        assert_eq!(
            derive_thread_title(&[message]),
            "Fix the rate limiter double-fire"
        );
    }

    #[test]
    fn thread_delivery_title_falls_back_when_all_boilerplate() {
        let message = Message {
            role: Role::User,
            content: vec![ContentBlock::Text("INSTRUKCJA: zrób coś".to_string())],
            timestamp: None,
        };
        assert_eq!(derive_thread_title(&[message]), "INSTRUKCJA: zrób coś");
    }
}
