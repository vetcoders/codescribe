//! Canonical persistence boundary for completed agent turns.
//!
//! Source-specific send paths construct [`ThreadDeliveryInput`]. This gateway
//! alone owns the durable load/create, title/summary projection, timestamped
//! upsert, and receipt contract over [`ThreadStore`].

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{ContentBlock, Message, Role, Thread, ThreadMessage, ThreadStore};

/// Placeholder title for a thread with nothing usable to derive one from.
///
/// Doubles as the "still heuristic" marker: a thread carrying this title has
/// neither a custom nor a generated one, so it stays title-eligible.
const DEFAULT_THREAD_TITLE: &str = "Codescribe Agent Chat";

/// Completed-turn origin. It is intentionally a core delivery concept rather
/// than UI state: callers use it for lifecycle evidence without logging content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDeliverySource {
    /// Hotkey-driven voice turn delivered through the assistive lane.
    VoiceAssistive,
    /// Typed turn sent from the chat composer.
    Composer,
    /// A connected instruction-following Max formatting consultation.
    MaxConsultation,
    /// Pre-gateway send path still routed here so nothing bypasses persistence.
    LegacyFallback,
    /// Resident run-monitor heartbeat re-entering an existing thread.
    Monitor,
}

impl ThreadDeliverySource {
    /// Stable kebab-case identifier for logs and lifecycle evidence.
    ///
    /// These strings are matched by log analysis, so they are part of the
    /// contract and are deliberately not derived from the variant names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VoiceAssistive => "voice-assistive",
            Self::Composer => "composer",
            Self::MaxConsultation => "max-consultation",
            Self::LegacyFallback => "legacy-fallback",
            Self::Monitor => "run-monitor",
        }
    }
}

/// Provider-agnostic durable state for one completed agent thread delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDeliveryInput {
    /// Thread identity. Voice and composer turns of one conversation must share
    /// it — a differing id splits the conversation into two threads.
    pub backend_id: String,
    /// The **full** turn history to persist, not a delta: `deliver` replaces the
    /// stored messages with this vector rather than appending to them.
    pub messages: Vec<ThreadMessage>,
    /// Provider that produced this turn, recorded for provenance.
    pub provider: String,
    /// Model that produced this turn.
    pub model: String,
    /// Which send path delivered it; logged, never persisted on the thread.
    pub source: ThreadDeliverySource,
    /// Conversation mode (for example `assistive`).
    pub mode: String,
    /// Thread tags, replaced wholesale on every delivery.
    pub tags: Vec<String>,
    /// Delivery time, used verbatim as the thread's `updated_at`.
    pub timestamp: DateTime<Utc>,
}

/// Durable proof returned only after the thread JSON and index upsert succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDeliveryReceipt {
    /// Thread the delivery landed on, echoed back for correlation.
    pub backend_id: String,
    /// This delivery created the thread rather than updating an existing one.
    pub created: bool,
    /// Message count now persisted on disk.
    pub message_count: usize,
    /// Timestamp written to the thread.
    pub updated_at: DateTime<Utc>,
    /// This delivery introduced the first completed user/assistant exchange.
    pub first_exchange: bool,
    /// The first exchange can launch isolated title generation. Custom and
    /// already-generated titles are never eligible.
    pub title_eligible: bool,
}

/// Read-only retained input for a recovery UI. Not a new execution request.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsultationInputSnapshot {
    pub turn_id: String,
    pub input: Message,
    pub provider_name: String,
}

/// One atomic journal read. A pending id does not prove process liveness:
/// it may describe active work or effects interrupted by a crash.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsultationRecoverySnapshot {
    pub consultation_id: String,
    pub pending_turn_id: Option<String>,
    pub retained_inputs: Vec<ConsultationInputSnapshot>,
}

/// The single write path for completed agent turns.
///
/// Every send path goes through here so thread identity, title/summary
/// projection, and index upsert have exactly one implementation. Owning the
/// [`ThreadStore`] is what keeps voice and composer turns converging on one
/// thread JSON and one index row instead of racing to write their own.
#[derive(Debug, Clone)]
pub struct ThreadDeliveryGateway {
    store: ThreadStore,
}

impl ThreadDeliveryGateway {
    /// Read the existing selection and its retained input, without selecting a
    /// consultation on first use. Snapshot identity may cease to be selected
    /// after the read; it is not an execution lease or a reset authorization.
    pub fn inspect_selected_max_consultation(&self) -> Result<Option<ConsultationRecoverySnapshot>> {
        super::thread_store::consultation::inspect_selected_max_consultation(&self.store)
    }

    /// Inspect without acquiring execution ownership, selecting a new thread,
    /// creating journal directories, modifying history or invoking a provider.
    pub fn inspect_consultation(&self, id: &str) -> Result<Option<ConsultationRecoverySnapshot>> {
        super::thread_store::consultation::inspect_retained_input(&self.store, id)
    }

    /// Read the durable Max selection, minting an identity only on first use.
    pub fn selected_max_consultation_id(&self) -> Result<String> {
        super::thread_store::consultation::selected_id(&self.store)
    }

    /// Select a fresh conversation only for an explicit, non-stale reset.
    pub fn begin_new_max_consultation(&self, expected: &str) -> Result<String> {
        super::thread_store::consultation::begin_new(&self.store, expected)
    }

    /// Called only while holding the consultation lease. Never substitute an
    /// empty history for a missing completed thread or an unreadable message.
    pub(crate) fn restore_consultation(&self, id: &str, history_required: bool) -> Result<Vec<Message>> {
        let path = self.store.thread_file_path(id)?;
        if !path.try_exists()? {
            anyhow::ensure!(!history_required, "Completed consultation history is missing");
            return Ok(Vec::new());
        }
        let thread = self.store.load_thread(id)?;
        anyhow::ensure!(thread.id == id && thread.mode == "max", "Consultation history identity or mode mismatch");
        let messages = thread.messages.iter().map(ThreadMessage::try_to_message)
            .collect::<Result<Vec<_>>>()?;
        validate_consultation_tool_history(&messages)?;
        Ok(messages)
    }

    /// Lock the same store's consultation admission state before executing tools.
    pub(crate) fn open_consultation(&self, id: &str) -> Result<super::thread_store::consultation::ConsultationJournal> {
        super::thread_store::consultation::ConsultationJournal::open(&self.store, id)
    }

    /// Open the gateway over the user's real threads directory.
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: ThreadStore::new().context("Failed to initialize ThreadStore")?,
        })
    }

    /// Open the gateway over an explicit threads directory.
    ///
    /// Lets tests exercise the full persistence contract against a temp dir
    /// instead of the user's data.
    pub fn new_in<P: AsRef<Path>>(threads_dir: P) -> Result<Self> {
        Ok(Self {
            store: ThreadStore::new_in(threads_dir)?,
        })
    }

    /// Persist a completed turn and return proof it landed.
    ///
    /// An upsert keyed on `backend_id`: an existing thread is loaded and updated
    /// in place, otherwise one is created. The stored message list is *replaced*
    /// by the input, and provider, model, mode, and tags are refreshed to match
    /// the delivering turn.
    ///
    /// The title is re-derived only while it is still heuristic, so a custom or
    /// already-generated title survives every later delivery. The receipt reports
    /// `first_exchange` (this delivery completed the first user→assistant pair)
    /// and `title_eligible` (that first exchange may launch title generation).
    ///
    /// The receipt is returned only after the thread JSON and the index upsert
    /// both succeed — no receipt is issued for a partial write.
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

/// Whether the history contains a completed user→assistant exchange.
///
/// Ordering matters: an assistant message must appear *after* the first user
/// message. A greeting emitted before the user says anything is not an exchange,
/// and must not make the thread title-eligible.
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

/// One-line preview for the thread rail: the latest assistant reply, clipped to
/// 240 characters.
///
/// Clipping is by `char`, not by byte, so a multi-byte boundary cannot panic.
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

/// Message text with its **line structure intact**, or `None` if blank.
///
/// Blocks are joined with newlines because the title path strips boilerplate
/// line by line — collapsing whitespace first would destroy the boundaries it
/// needs. Contrast [`extract_text_from_message`].
fn raw_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let text = out.join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// Message text flattened to a single whitespace-normalized line, or `None` if
/// blank.
///
/// The display form, used for summaries and as the title fallback.
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

/// Append a block's text to `out`, recursing into tool results.
///
/// Non-text blocks and whitespace-only text are skipped, so blocks that carry no
/// readable content contribute nothing to a title or summary.
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

/// Lowercase line prefixes that mark preamble rather than the user's request.
///
/// Bilingual by necessity: prompts reach the agent in Polish and English, and a
/// thread titled "INSTRUKCJA UŻYTKOWNIKA" tells the user nothing.
const BOILERPLATE_LINE_PREFIXES: &[&str] = &[
    "instrukcja",
    "instruction",
    "jesteś agentem",
    "jestes agentem",
    "you are an agent",
    "system prompt",
    "system:",
];

/// First line that reads like an actual request, whitespace-normalized.
///
/// Returns `None` when every line is blank or boilerplate; the caller then falls
/// back to the unstripped text rather than showing an empty title.
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

/// Completed consultation context must retain tool causality, not just valid JSON.
/// Result payloads are data and may not introduce nested calls or results.
fn validate_consultation_tool_history(messages: &[Message]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut pending = std::collections::HashSet::new();
    for message in messages {
        let has_results = message.content.iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
        if has_results {
            anyhow::ensure!(message.role == Role::User,
                "Consultation tool result has the wrong role");
            for block in &message.content {
                let ContentBlock::ToolResult { tool_use_id, content, .. } = block else {
                    anyhow::bail!("Consultation tool results are mixed with unrelated content");
                };
                anyhow::ensure!(pending.remove(tool_use_id),
                    "Consultation tool result has no unmatched prior invocation");
                anyhow::ensure!(content.iter().all(|child| !matches!(
                    child, ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                )), "Consultation tool payload contains nested control blocks");
            }
        } else {
            anyhow::ensure!(pending.is_empty(),
                "Consultation continues before prior tool results are complete");
            for block in &message.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    anyhow::ensure!(message.role == Role::Assistant,
                        "Consultation tool invocation has the wrong role");
                    anyhow::ensure!(seen.insert(id.clone()),
                        "Consultation reuses a tool invocation identity");
                    pending.insert(id.clone());
                }
            }
        }
    }
    anyhow::ensure!(pending.is_empty(), "Completed consultation has unresolved tool calls");
    Ok(())
}

/// Whether a line is preamble: a known prefix, or an all-caps header.
fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    BOILERPLATE_LINE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || is_all_caps_header(line)
}

/// Whether a line has letters and none of them are lowercase.
///
/// Requiring at least one letter keeps pure punctuation or digit lines (`---`,
/// `2026`) from being classified as shouted headers.
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

/// Delivery gateway contract tests: single-thread upsert, title eligibility,
/// and boilerplate stripping against a temp store.
#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::agent::{ThreadIndex, ThreadIndexData, ThreadNote, TokenUsage};

    #[test]
    fn consultation_restore_checks_tool_causality_without_rewriting_history() -> Result<()> {
        let dir = TempDir::new()?;
        let gateway = ThreadDeliveryGateway::new_in(dir.path())?;
        let call = |id: &str| ContentBlock::ToolUse {
            id: id.into(), name: "read_clipboard".into(), input: json!({}),
        };
        let result = |id: &str| ContentBlock::ToolResult {
            tool_use_id: id.into(), content: vec![ContentBlock::Text("a.rs".into())],
            is_error: false,
        };
        let valid = vec![
            Message::new(Role::User, vec![ContentBlock::Text("prepare a command".into())]),
            Message::new(Role::Assistant, vec![call("a"), call("b")]),
            Message::new(Role::User, vec![result("a")]),
            Message::new(Role::User, vec![result("b")]),
            Message::new(Role::Assistant, vec![ContentBlock::Text("git add -- a.rs".into())]),
        ];
        let mut cases = vec![(valid.clone(), true)];
        let mut orphan = valid.clone();
        orphan[2].content = vec![result("unknown")];
        cases.push((orphan, false));
        let mut duplicate_result = valid.clone();
        duplicate_result[3].content = vec![result("a")];
        cases.push((duplicate_result, false));
        let mut duplicate_call = valid.clone();
        duplicate_call[1].content = vec![call("a"), call("a")];
        cases.push((duplicate_call, false));
        let mut wrong_call_role = valid.clone();
        wrong_call_role[1].role = Role::User;
        cases.push((wrong_call_role, false));
        let mut wrong_result_role = valid.clone();
        wrong_result_role[2].role = Role::Assistant;
        cases.push((wrong_result_role, false));
        let mut missing = valid.clone();
        missing.remove(3);
        cases.push((missing, false));
        cases.push((valid[..2].to_vec(), false));
        let mut nested = valid.clone();
        nested[2].content = vec![ContentBlock::ToolResult {
            tool_use_id: "a".into(), content: vec![call("nested")], is_error: false,
        }];
        cases.push((nested, false));
        let mut mixed = valid.clone();
        mixed[2].content.push(ContentBlock::Text("new instruction".into()));
        cases.push((mixed, false));
        let mut failed_tool = valid.clone();
        if let ContentBlock::ToolResult { is_error, .. } = &mut failed_tool[2].content[0] {
            *is_error = true;
        }
        cases.push((failed_tool, true));

        for (index, (messages, accepted)) in cases.into_iter().enumerate() {
            let id = format!("causality-{index}");
            let mut delivery = input(&id, ThreadDeliverySource::MaxConsultation,
                messages.iter().map(ThreadMessage::from).collect(), timestamp(1));
            delivery.mode = "max".into();
            gateway.deliver(delivery)?;
            let path = gateway.store.thread_file_path(&id)?;
            let before = fs::read(&path)?;
            assert_eq!(gateway.restore_consultation(&id, true).is_ok(), accepted,
                "case {index}");
            assert_eq!(fs::read(path)?, before, "restore must not repair stored evidence");
        }
        Ok(())
    }

    #[test]
    fn consultation_restore_requires_matching_history_and_mode() -> Result<()> {
        let dir = TempDir::new()?;
        let gateway = ThreadDeliveryGateway::new_in(dir.path())?;
        assert!(gateway.restore_consultation("missing", true).is_err());
        assert!(gateway.restore_consultation("fresh", false)?.is_empty());
        let mut delivery = input("max-a", ThreadDeliverySource::MaxConsultation,
            vec![message("user", "first", timestamp(1)), message("assistant", "answer", timestamp(1))], timestamp(1));
        delivery.mode = "max".into();
        gateway.deliver(delivery.clone())?;
        let history = gateway.restore_consultation("max-a", true)?;
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].role, Role::Assistant);
        delivery.mode = "assistive".into();
        gateway.deliver(delivery)?;
        assert!(gateway.restore_consultation("max-a", true).is_err());
        Ok(())
    }

    /// Fixed UTC timestamp on 2026-07-19 at the given hour for deterministic tests.
    fn timestamp(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 19, hour, 0, 0)
            .single()
            .expect("fixed timestamp should be valid")
    }

    /// One-block text `ThreadMessage` with the given role and wall-clock.
    fn message(role: &str, text: &str, at: DateTime<Utc>) -> ThreadMessage {
        ThreadMessage {
            role: role.to_string(),
            content: vec![json!({"type":"text","text":text})],
            timestamp: at,
            metadata: None,
        }
    }

    /// Build a full `ThreadDeliveryInput` with fixed provider/model/mode/tags.
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

    /// User then assistant messages at the same timestamp (one completed exchange).
    fn exchange(at: DateTime<Utc>, user: &str, assistant: &str) -> Vec<ThreadMessage> {
        vec![
            message("user", user, at),
            message("assistant", assistant, at),
        ]
    }

    /// Assert the temp dir has exactly one thread JSON and one index row.
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

    /// Voice and composer inputs share backend id while preserving distinct sources.
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

    /// Same backend id: voice then composer land on one JSON file and one index row.
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

    /// Two different backend ids leave two artifacts; the single-thread verifier panics.
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

    /// Custom titles survive deliver; first_exchange may still fire, title_eligible does not.
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

    /// Title derivation skips ALL-CAPS / instruction preambles and keeps the request line.
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

    /// Plain first-line user text becomes the title without boilerplate stripping.
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

    /// When every line is boilerplate, fall back to the unstripped first-user text.
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
