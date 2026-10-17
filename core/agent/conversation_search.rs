//! Shared conversation query matching and read-only saved-archive search.
//!
//! Agent thread search and bus-archive search both reduce a query the same way:
//! Unicode lowercase, accents preserved, whitespace-separated terms, every term
//! a substring. The bus side reads released `codescribe.agent-archive.v1` files
//! and the JSONL they name. It does not write, acknowledge, or truncate history
//! to the overlay's 16-bus / 256-message / 16 KiB windows.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// One saved conversation the archive walk could prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusConversationSearchHit {
    /// `provider + NUL + provider_session_id + NUL + lease_id`.
    pub conversation_id: String,
    /// Newest RFC3339 activity of current text, or empty when no date is known.
    pub last_activity: String,
}

/// Read-only archive search result. `complete` is false when any history read
/// or released-archive validation failed; other hits stay in `results`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusConversationSearch {
    pub results: Vec<BusConversationSearchHit>,
    pub complete: bool,
    pub error_count: u32,
}

/// Collapse whitespace and apply Unicode lowercase without stripping accents.
pub fn normalize_search_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Query terms after [`normalize_search_text`]. A blank query yields no terms.
pub fn query_terms(query: &str) -> Vec<String> {
    normalize_search_text(query)
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether every query term is a substring of `haystack`.
///
/// An empty query matches everything. Both sides are Unicode-lowercased, so
/// `Ł` matches `ł` and a stripped `l` does not.
pub fn query_matches(haystack: &str, query: &str) -> bool {
    let terms = query_terms(query);
    if terms.is_empty() {
        return true;
    }
    let haystack = normalize_search_text(haystack);
    terms.iter().all(|term| haystack.contains(term))
}

/// Search released bus archives under `root` (`…/archives/{lease}-{channel}.json`).
///
/// A missing archives directory is an honest empty result. An unreadable root
/// is an error, not an exhaustive zero. One bad history file does not drop hits
/// from the others.
pub fn search_bus_conversations(root: &Path, query: &str) -> Result<BusConversationSearch> {
    let root = root.canonicalize().with_context(|| {
        format!(
            "Agent bridge root is not a readable directory: {}",
            root.display()
        )
    })?;
    if !root.is_dir() {
        bail!("Agent bridge root is not a directory: {}", root.display());
    }

    let archives = root.join("archives");
    if !archives.exists() {
        return Ok(BusConversationSearch {
            results: Vec::new(),
            complete: true,
            error_count: 0,
        });
    }
    if !archives.is_dir() {
        bail!(
            "Agent bridge archives path is not a directory: {}",
            archives.display()
        );
    }
    let archives = archives.canonicalize().with_context(|| {
        format!(
            "Failed to resolve archives directory: {}",
            archives.display()
        )
    })?;

    let mut coverage = Coverage::default();
    let mut names = Vec::new();
    for entry in fs::read_dir(&archives)
        .with_context(|| format!("Failed to read archives directory: {}", archives.display()))?
    {
        match entry {
            Ok(entry) => names.push(entry.file_name()),
            Err(_) => coverage.record(),
        }
    }
    names.sort();

    let mut verified = Vec::new();
    for name in &names {
        let Some(name) = name.to_str() else {
            coverage.record();
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        match read_archive(&archives, name) {
            ArchiveRead::Skip => {}
            ArchiveRead::Malformed => coverage.record(),
            ArchiveRead::Verified(archive) => verified.push(archive),
        }
    }

    let mut buses: HashMap<PathBuf, HashMap<String, OwnerCorpus>> = HashMap::new();
    for archive in &verified {
        if !buses.contains_key(&archive.bus) {
            let corpus = project_bus(&archive.bus, &mut coverage);
            buses.insert(archive.bus.clone(), corpus);
        }
    }

    let mut pending: HashMap<String, PendingHit> = HashMap::new();
    for archive in &verified {
        let corpus = buses
            .get(&archive.bus)
            .and_then(|by_owner| by_owner.get(&archive.conversation_id));
        let haystack = haystack_for(archive, corpus);
        let activity = activity_for(archive, corpus);
        let entry = pending.entry(archive.conversation_id.clone()).or_default();
        if !entry.haystack.is_empty() && !haystack.is_empty() {
            entry.haystack.push('\n');
        }
        entry.haystack.push_str(&haystack);
        entry.last_activity = prefer_later(&entry.last_activity, &activity);
    }

    let mut results = Vec::new();
    for (conversation_id, hit) in pending {
        if query_matches(&hit.haystack, query) {
            results.push(BusConversationSearchHit {
                conversation_id,
                last_activity: hit.last_activity,
            });
        }
    }
    results.sort_by(cmp_hits);
    Ok(BusConversationSearch {
        results,
        complete: coverage.error_count == 0,
        error_count: coverage.error_count,
    })
}

/// Swift archive reader refuses documents larger than this.
const ARCHIVE_BYTE_LIMIT: usize = 65_536;

#[derive(Debug, Default)]
struct Coverage {
    error_count: u32,
}

impl Coverage {
    fn record(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
    }
}

#[derive(Debug, Clone)]
struct Owner {
    provider: String,
    session: String,
    lease: String,
    name: String,
    channel: String,
}

impl Owner {
    fn id(&self) -> String {
        format!("{}\0{}\0{}", self.provider, self.session, self.lease)
    }
}

struct VerifiedArchive {
    conversation_id: String,
    provider: String,
    session: String,
    name: String,
    bus: PathBuf,
    archive_activity: String,
}

enum ArchiveRead {
    Skip,
    Malformed,
    Verified(VerifiedArchive),
}

#[derive(Debug)]
struct LiveText {
    text: String,
    emitted_at: String,
    owners: Vec<String>,
    revision: u64,
    source_channel: String,
}

#[derive(Debug, Default)]
struct Projection {
    messages: HashMap<String, LiveText>,
    capture_recipients: HashMap<String, Vec<Owner>>,
    session_opened_at: HashMap<String, String>,
}

#[derive(Debug, Default)]
struct OwnerCorpus {
    texts: Vec<String>,
    last_activity: String,
}

#[derive(Debug, Default)]
struct PendingHit {
    haystack: String,
    last_activity: String,
}

fn read_archive(archives: &Path, name: &str) -> ArchiveRead {
    let child = archives.join(name);
    let canon = match child.canonicalize() {
        Ok(path) => path,
        Err(_) => return ArchiveRead::Malformed,
    };
    if canon.parent() != Some(archives) || !canon.is_file() {
        return ArchiveRead::Malformed;
    }
    let bytes = match read_bounded(&canon) {
        Ok(bytes) => bytes,
        Err(()) => return ArchiveRead::Malformed,
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return ArchiveRead::Malformed,
    };
    let Some(row) = value.as_object() else {
        return ArchiveRead::Malformed;
    };
    if field_str(row, "schema") != Some("codescribe.agent-archive.v1") {
        return ArchiveRead::Skip;
    }
    if !matches!(row.get("released"), Some(Value::Bool(true))) {
        return ArchiveRead::Skip;
    }
    let Some(owner) = parse_owner(row) else {
        return ArchiveRead::Malformed;
    };
    if !matches!(
        owner.channel.as_str(),
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    ) {
        return ArchiveRead::Malformed;
    }
    if name != format!("{}-{}.json", owner.lease, owner.channel) {
        return ArchiveRead::Malformed;
    }
    if owner.lease != lease_identifier(&owner.provider, &owner.session) {
        return ArchiveRead::Malformed;
    }
    let Some(bus) = field_str(row, "bus") else {
        return ArchiveRead::Malformed;
    };
    let bus_path = PathBuf::from(bus);
    if !is_safe_bus(&bus_path) {
        return ArchiveRead::Malformed;
    }
    let archive_activity = field_str(row, "time")
        .filter(|value| rfc3339(value).is_some())
        .unwrap_or("")
        .to_string();
    ArchiveRead::Verified(VerifiedArchive {
        conversation_id: owner.id(),
        provider: owner.provider,
        session: owner.session,
        name: owner.name,
        bus: bus_path,
        archive_activity,
    })
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, ()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- path is a canonical direct child of the archives directory.
    let mut file = File::open(path).map_err(|_| ())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = file.read(&mut chunk).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        if buf.len().saturating_add(read) > ARCHIVE_BYTE_LIMIT {
            return Err(());
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

fn project_bus(path: &Path, coverage: &mut Coverage) -> HashMap<String, OwnerCorpus> {
    if !is_safe_bus(path) || !path.is_file() {
        coverage.record();
        return HashMap::new();
    }
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- bus is the archive's absolute history path; parent-dir components are rejected and the file is opened read-only.
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            coverage.record();
            return HashMap::new();
        }
    };
    let mut projection = Projection::default();
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                coverage.record();
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => {
                coverage.record();
                continue;
            }
        };
        let Some(row) = value.as_object() else {
            coverage.record();
            continue;
        };
        consume_row(&mut projection, row);
    }
    corpus_from_projection(&projection)
}

fn consume_row(state: &mut Projection, row: &Map<String, Value>) {
    match field_str(row, "schema") {
        Some("codescribe.agent-user-message.v1") => consume_user_message(state, row),
        Some("codescribe.agent-reply.v1") => consume_reply(state, row),
        Some("codescribe.channel-recipients.v1") => consume_recipients(state, row),
        Some("codescribe.channel-session.v1") => consume_channel_session(state, row),
        Some("codescribe.transcript.v1") => consume_transcript(state, row, false),
        Some("codescribe.transcript-evidence.v1") => consume_transcript(state, row, true),
        _ => {}
    }
}

fn consume_user_message(state: &mut Projection, row: &Map<String, Value>) {
    if field_str(row, "kind") != Some("agent_user_message") {
        return;
    }
    let Some(identity) = field_str(row, "message_id") else {
        return;
    };
    if !is_lower_hex(identity, 24) {
        return;
    }
    let Some(text) = field_str(row, "text") else {
        return;
    };
    if text.is_empty() || field_str(row, "audience").is_none() {
        return;
    }
    let Some(entries) = row.get("recipients").and_then(Value::as_array) else {
        return;
    };
    let owners = entries
        .iter()
        .filter_map(|entry| entry.as_object().and_then(parse_owner))
        .collect::<Vec<_>>();
    if owners.len() != 1 {
        return;
    }
    let owner = owners.into_iter().next().expect("len checked");
    let key = format!("typed:{identity}");
    let owner_id = owner.id();
    if let Some(existing) = state.messages.get_mut(&key) {
        let origin = field_str(row, "origin_channel").unwrap_or("");
        if origin == "0"
            && existing.source_channel == "0"
            && existing.text == text
            && !existing.owners.iter().any(|id| id == &owner_id)
        {
            existing.owners.push(owner_id);
        }
        return;
    }
    let source_channel = field_str(row, "origin_channel")
        .or_else(|| field_str(row, "channel"))
        .unwrap_or("")
        .to_string();
    state.messages.insert(
        key,
        LiveText {
            text: text.to_string(),
            emitted_at: field_str(row, "emitted_at").unwrap_or("").to_string(),
            owners: vec![owner_id],
            revision: 0,
            source_channel,
        },
    );
}

fn consume_reply(state: &mut Projection, row: &Map<String, Value>) {
    let (Some(reply_id), Some(owner), Some(text)) = (
        field_str(row, "reply_id"),
        parse_owner(row),
        field_str(row, "text"),
    ) else {
        return;
    };
    let key = format!("reply:{reply_id}");
    let owner_id = owner.id();
    if state.messages.get(&key).is_some_and(|previous| {
        previous.owners.first().map(String::as_str) != Some(owner_id.as_str())
    }) {
        return;
    }
    state.messages.insert(
        key,
        LiveText {
            text: text.to_string(),
            emitted_at: field_str(row, "emitted_at").unwrap_or("").to_string(),
            owners: vec![owner_id],
            revision: 0,
            source_channel: String::new(),
        },
    );
}

fn consume_recipients(state: &mut Projection, row: &Map<String, Value>) {
    let Some(session) = field_str(row, "session_id") else {
        return;
    };
    let Some(entries) = row.get("recipients").and_then(Value::as_array) else {
        return;
    };
    let owners = entries
        .iter()
        .filter_map(|entry| entry.as_object().and_then(parse_owner))
        .collect::<Vec<_>>();
    state.capture_recipients.insert(session.to_string(), owners);
}

fn consume_channel_session(state: &mut Projection, row: &Map<String, Value>) {
    if field_str(row, "state") != Some("open") {
        return;
    }
    let (Some(session), Some(opened_at)) =
        (field_str(row, "session_id"), field_str(row, "opened_at"))
    else {
        return;
    };
    if opened_at.is_empty() {
        return;
    }
    state
        .session_opened_at
        .insert(session.to_string(), opened_at.to_string());
}

fn consume_transcript(state: &mut Projection, row: &Map<String, Value>, evidence: bool) {
    if field_str(row, "status") == Some("session_ended")
        || field_str(row, "reducer_action") == Some("session_ended")
    {
        return;
    }
    let Some(session) = field_str(row, "session_id") else {
        return;
    };
    let Some(audience) = field_str(row, "audience") else {
        return;
    };
    if audience.is_empty() {
        return;
    }
    let owners = if let Some(entries) = row.get("recipients").and_then(Value::as_array) {
        let parsed = entries
            .iter()
            .filter_map(|entry| entry.as_object().and_then(parse_owner))
            .collect::<Vec<_>>();
        state
            .capture_recipients
            .insert(session.to_string(), parsed.clone());
        parsed
    } else {
        state
            .capture_recipients
            .get(session)
            .cloned()
            .unwrap_or_default()
    };
    if owners.is_empty() {
        return;
    }

    let channel_key = if evidence {
        channel_message_key(session)
    } else {
        None
    };
    let text = if channel_key.is_some() {
        field_str(row, "rendered_text")
    } else {
        field_str(row, "label")
            .or_else(|| field_str(row, "text"))
            .or_else(|| field_str(row, "rendered_text"))
    };
    let Some(text) = text else {
        return;
    };
    // Trim-empty channel `rendered_text` is not a document. The bus history
    // entry drops it, `apply_document_text` refuses `EmptyText`, and the
    // ledger refuses `manual_document_text_empty`. Leave the current text and
    // the revision tombstone where the last nonempty document put them.
    if channel_key.is_some() {
        if text.trim().is_empty() {
            return;
        }
    } else if text.is_empty() {
        return;
    }
    if !evidence
        && !row
            .get("utterance_id")
            .is_some_and(|value| value.is_string() || value.is_number())
    {
        return;
    }
    if evidence && !evidence_coordinates_present(row) {
        return;
    }

    let epoch = coordinate(row.get("capture_epoch"));
    let start = coordinate(row.get("sample_start"));
    let end = coordinate(row.get("sample_end"));
    let utterance = coordinate(row.get("utterance_id"));
    let occurrence_session = field_str(row, "occurrence_session_id").unwrap_or(session);
    let occurrence = if evidence {
        identity(&["occurrence", occurrence_session, &epoch, &start, &end])
    } else {
        identity(&["utterance", session, &utterance])
    };
    let key = channel_key.unwrap_or_else(|| format!("utterance:{occurrence}"));
    let revision = if evidence {
        json_u64(row.get("reducer_revision"))
    } else {
        json_u64(row.get("sequence"))
    };
    if state
        .messages
        .get(&key)
        .is_some_and(|current| revision < current.revision)
    {
        return;
    }
    let emitted_at = if evidence && channel_message_key(session).is_some() {
        state
            .messages
            .get(&key)
            .map(|message| message.emitted_at.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| state.session_opened_at.get(session).cloned())
            .or_else(|| field_str(row, "emitted_at").map(str::to_string))
            .unwrap_or_default()
    } else {
        field_str(row, "emitted_at").unwrap_or("").to_string()
    };
    state.messages.insert(
        key,
        LiveText {
            text: text.to_string(),
            emitted_at,
            owners: owners.into_iter().map(|owner| owner.id()).collect(),
            revision,
            source_channel: String::new(),
        },
    );
}

fn corpus_from_projection(projection: &Projection) -> HashMap<String, OwnerCorpus> {
    let mut corpora: HashMap<String, OwnerCorpus> = HashMap::new();
    for message in projection.messages.values() {
        for owner_id in &message.owners {
            let corpus = corpora.entry(owner_id.clone()).or_default();
            if !message.text.is_empty() {
                corpus.texts.push(message.text.clone());
            }
            corpus.last_activity = prefer_later(&corpus.last_activity, &message.emitted_at);
        }
    }
    corpora
}

fn haystack_for(archive: &VerifiedArchive, corpus: Option<&OwnerCorpus>) -> String {
    let mut parts = Vec::new();
    if !archive.name.is_empty() {
        parts.push(archive.name.as_str());
    }
    parts.push(archive.provider.as_str());
    parts.push(archive.session.as_str());
    let mut joined = parts.join("\n");
    if let Some(corpus) = corpus {
        for text in &corpus.texts {
            if text.is_empty() {
                continue;
            }
            if !joined.is_empty() {
                joined.push('\n');
            }
            joined.push_str(text);
        }
    }
    joined
}

fn activity_for(archive: &VerifiedArchive, corpus: Option<&OwnerCorpus>) -> String {
    if let Some(corpus) = corpus
        && rfc3339(&corpus.last_activity).is_some()
    {
        return corpus.last_activity.clone();
    }
    archive.archive_activity.clone()
}

fn parse_owner(row: &Map<String, Value>) -> Option<Owner> {
    let provider = field_str(row, "provider")?;
    if provider.is_empty() {
        return None;
    }
    let session = field_str(row, "provider_session_id")?;
    if session.is_empty() {
        return None;
    }
    let lease = field_str(row, "lease_id")?;
    if !is_lower_hex(lease, 32) {
        return None;
    }
    let name = field_str(row, "name")
        .or_else(|| field_str(row, "agent"))
        .or_else(|| field_str(row, "audience"))
        .unwrap_or(provider)
        .to_string();
    let channel = field_str(row, "channel").unwrap_or("").to_string();
    Some(Owner {
        provider: provider.to_string(),
        session: session.to_string(),
        lease: lease.to_string(),
        name,
        channel,
    })
}

fn field_str<'a>(row: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn lease_identifier(provider: &str, session: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider.to_lowercase().as_bytes());
    hasher.update([0u8]);
    hasher.update(session.as_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn identity(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hasher.update([0u8]);
        }
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..12])
}

fn channel_message_key(session: &str) -> Option<String> {
    let parts = session
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() > 2
        && parts[0] == "agent"
        && parts[1] == "channel"
        && parts[2].chars().all(|ch| ch.is_ascii_digit())
    {
        Some(format!(
            "utterance:{}",
            identity(&["channel-message", session])
        ))
    } else {
        None
    }
}

fn evidence_coordinates_present(row: &Map<String, Value>) -> bool {
    field_str(row, "occurrence_session_id").is_some()
        && row.get("capture_epoch").is_some_and(Value::is_number)
        && row.get("sample_start").is_some_and(Value::is_number)
        && row.get("sample_end").is_some_and(Value::is_number)
}

fn coordinate(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn json_u64(value: Option<&Value>) -> u64 {
    let Some(Value::Number(number)) = value else {
        return 0;
    };
    if let Some(value) = number.as_u64() {
        return value;
    }
    if let Some(value) = number.as_i64() {
        return u64::try_from(value).unwrap_or(0);
    }
    if let Some(value) = number.as_f64()
        && value.is_finite()
        && value >= 0.0
        && value <= u64::MAX as f64
        && value.fract() == 0.0
    {
        return value as u64;
    }
    0
}

fn is_safe_bus(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn rfc3339(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|stamp| stamp.with_timezone(&Utc))
}

fn prefer_later(current: &str, candidate: &str) -> String {
    match (rfc3339(current), rfc3339(candidate)) {
        (Some(current_at), Some(candidate_at)) => {
            if candidate_at >= current_at {
                candidate.to_string()
            } else {
                current.to_string()
            }
        }
        (Some(_), None) => current.to_string(),
        (None, Some(_)) => candidate.to_string(),
        (None, None) => String::new(),
    }
}

fn cmp_hits(
    left: &BusConversationSearchHit,
    right: &BusConversationSearchHit,
) -> std::cmp::Ordering {
    match (rfc3339(&left.last_activity), rfc3339(&right.last_activity)) {
        (Some(left_at), Some(right_at)) => right_at
            .cmp(&left_at)
            .then_with(|| left.conversation_id.cmp(&right.conversation_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.conversation_id.cmp(&right.conversation_id),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    fn conversation_id(provider: &str, session: &str) -> String {
        format!(
            "{provider}\0{session}\0{}",
            lease_identifier(provider, session)
        )
    }

    fn owner_json(provider: &str, session: &str, channel: &str, name: &str) -> Value {
        json!({
            "provider": provider,
            "provider_session_id": session,
            "lease_id": lease_identifier(provider, session),
            "channel": channel,
            "name": name,
        })
    }

    fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn write_jsonl(path: &Path, rows: &[Value]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        for row in rows {
            let mut line = serde_json::to_vec(row).unwrap();
            line.push(b'\n');
            file.write_all(&line).unwrap();
        }
    }

    fn write_released_archive(
        root: &Path,
        provider: &str,
        session: &str,
        channel: &str,
        name: &str,
        bus: &Path,
        time: Option<&str>,
    ) {
        let lease = lease_identifier(provider, session);
        let mut row = json!({
            "schema": "codescribe.agent-archive.v1",
            "released": true,
            "provider": provider,
            "provider_session_id": session,
            "lease_id": lease,
            "channel": channel,
            "name": name,
            "bus": bus,
        });
        if let Some(time) = time {
            row["time"] = json!(time);
        }
        write_json(
            &root
                .join("archives")
                .join(format!("{lease}-{channel}.json")),
            &row,
        );
    }

    fn transcript(
        session: &str,
        utterance: &str,
        text: &str,
        sequence: u64,
        owner: &Value,
        emitted_at: Option<&str>,
    ) -> Value {
        let mut row = json!({
            "schema": "codescribe.transcript.v1",
            "status": "transcript_sealed",
            "sequence": sequence,
            "session_id": session,
            "utterance_id": utterance,
            "audience": "agent",
            "text": text,
            "recipients": [owner],
        });
        if let Some(emitted_at) = emitted_at {
            row["emitted_at"] = json!(emitted_at);
        }
        row
    }

    fn matching_ids(found: &BusConversationSearch) -> Vec<&str> {
        found
            .results
            .iter()
            .map(|hit| hit.conversation_id.as_str())
            .collect()
    }

    fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
        let mut out = BTreeMap::new();
        fn walk(path: &Path, root: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let meta = fs::symlink_metadata(path).unwrap();
            if meta.file_type().is_dir() {
                out.insert(rel, None);
                let mut children = fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect::<Vec<PathBuf>>();
                children.sort();
                for child in children {
                    walk(&child, root, out);
                }
            } else {
                out.insert(rel, Some(fs::read(path).unwrap()));
            }
        }
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn unicode_multiword_query_keeps_accents() {
        assert!(query_matches("ŁÓDŹ clinic", "  łódź   clinic "));
        assert!(!query_matches("ŁÓDŹ clinic", "lodz clinic"));
        assert!(query_matches("Ąkę review", "Ą"));
        assert_eq!(query_terms("  ŁÓDŹ   Clinic "), vec!["łódź", "clinic"]);

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let matching = root.join("lodz.jsonl");
        let other = root.join("plain.jsonl");
        let owner = owner_json("codex", "unicode-session", "4", "Łódź");
        write_jsonl(
            &matching,
            &[transcript(
                "unicode-take",
                "u1",
                "ŁÓDŹ clinic Ąkę",
                1,
                &owner,
                None,
            )],
        );
        let plain_owner = owner_json("codex", "plain-session", "5", "lodz");
        write_jsonl(
            &other,
            &[transcript(
                "plain-take",
                "u1",
                "lodz clinic notes",
                1,
                &plain_owner,
                None,
            )],
        );
        write_released_archive(
            root,
            "codex",
            "unicode-session",
            "4",
            "Łódź",
            &matching,
            None,
        );
        write_released_archive(root, "codex", "plain-session", "5", "lodz", &other, None);

        let found = search_bus_conversations(root, "łódź clinic").unwrap();
        assert!(found.complete);
        assert_eq!(found.error_count, 0);
        assert_eq!(
            matching_ids(&found),
            vec![conversation_id("codex", "unicode-session").as_str()]
        );
        assert!(
            search_bus_conversations(root, "lodz clinic")
                .unwrap()
                .results
                .len()
                == 1
        );
        assert_eq!(
            matching_ids(&search_bus_conversations(root, "lodz clinic").unwrap()),
            vec![conversation_id("codex", "plain-session").as_str()]
        );
        assert_eq!(
            matching_ids(&search_bus_conversations(root, "Ąkę").unwrap()),
            vec![conversation_id("codex", "unicode-session").as_str()]
        );
        assert!(
            search_bus_conversations(root, "łódź absent")
                .unwrap()
                .results
                .is_empty()
        );
    }

    #[test]
    fn covers_archives_past_sixteen_and_text_past_history_caps() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let huge = format!("{} PAST-BYTE-LIMIT", "x".repeat(20_000));
        for index in 0..17 {
            let session = format!("saved-session-{index}");
            let bus = root.join(format!("saved-{index}.jsonl"));
            let owner = owner_json("codex", &session, "4", &format!("saved-{index}"));
            let mut rows = Vec::new();
            if index == 16 {
                rows.push(transcript(
                    &session,
                    "u-needle",
                    "NEEDLE-SEVENTEEN",
                    1,
                    &owner,
                    Some("2026-10-01T00:00:00Z"),
                ));
                for step in 1u64..256 {
                    rows.push(transcript(
                        &session,
                        &format!("u-{step}"),
                        "filler",
                        step,
                        &owner,
                        None,
                    ));
                }
                rows.push(transcript(
                    &session,
                    "u-huge",
                    &huge,
                    257,
                    &owner,
                    Some("2026-10-11T00:00:00Z"),
                ));
            } else {
                rows.push(transcript(
                    &session,
                    "u-1",
                    &format!("filler-{index}"),
                    1,
                    &owner,
                    None,
                ));
            }
            write_jsonl(&bus, &rows);
            write_released_archive(
                root,
                "codex",
                &session,
                "4",
                &format!("saved-{index}"),
                &bus,
                None,
            );
        }

        let all = search_bus_conversations(root, "").unwrap();
        assert!(all.complete, "error_count {}", all.error_count);
        assert_eq!(all.results.len(), 17);

        let seventeenth = conversation_id("codex", "saved-session-16");
        let needle = search_bus_conversations(root, "NEEDLE-SEVENTEEN").unwrap();
        assert_eq!(matching_ids(&needle), vec![seventeenth.as_str()]);
        let past_bytes = search_bus_conversations(root, "PAST-BYTE-LIMIT").unwrap();
        assert_eq!(matching_ids(&past_bytes), vec![seventeenth.as_str()]);
        assert_eq!(past_bytes.results[0].last_activity, "2026-10-11T00:00:00Z");
        let first = search_bus_conversations(root, "filler-0").unwrap();
        assert_eq!(
            matching_ids(&first),
            vec![conversation_id("codex", "saved-session-0").as_str()]
        );
    }

    #[test]
    fn current_revision_and_exact_owner_do_not_cross_match() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let shared = root.join("shared.jsonl");
        let reused = root.join("reused.jsonl");
        let alpha = owner_json("codex", "alpha-session", "4", "alpha");
        let beta = owner_json("codex", "beta-session", "4", "beta");
        let gamma = owner_json("codex", "gamma-session", "4", "gamma");
        let mut stolen = alpha.clone();
        let stolen_obj = stolen.as_object_mut().unwrap();
        stolen_obj.insert("schema".to_string(), json!("codescribe.agent-reply.v1"));
        stolen_obj.insert("reply_id".to_string(), json!("bbbbbbbbbbbbbbbbbbbbbbbb"));
        stolen_obj.insert("text".to_string(), json!("STOLEN-REPLY"));
        stolen_obj.insert("association".to_string(), json!("unsolicited"));
        stolen_obj.insert("emitted_at".to_string(), json!("2026-10-06T00:00:00Z"));

        let mut beta_reply = beta.clone();
        let beta_obj = beta_reply.as_object_mut().unwrap();
        beta_obj.insert("schema".to_string(), json!("codescribe.agent-reply.v1"));
        beta_obj.insert("reply_id".to_string(), json!("bbbbbbbbbbbbbbbbbbbbbbbb"));
        beta_obj.insert("text".to_string(), json!("OTHER-OWNER-SECRET"));
        beta_obj.insert("association".to_string(), json!("unsolicited"));
        beta_obj.insert("emitted_at".to_string(), json!("2026-10-05T04:00:00Z"));

        write_jsonl(
            &shared,
            &[
                json!({
                    "schema": "codescribe.transcript-evidence.v1",
                    "session_id": "agent-channel-4-take",
                    "audience": "alpha",
                    "reducer_revision": 1,
                    "reducer_action": "revise",
                    "rendered_text": "WITHDRAWN-CHANNEL",
                    "occurrence_session_id": "agent-channel-4-take",
                    "capture_epoch": 1,
                    "sample_start": 0,
                    "sample_end": 10,
                    "recipients": [&alpha],
                }),
                json!({
                    "schema": "codescribe.transcript-evidence.v1",
                    "session_id": "agent-channel-4-take",
                    "audience": "alpha",
                    "reducer_revision": 2,
                    "reducer_action": "revise",
                    "rendered_text": "CURRENT-CHANNEL",
                    "label": "WITHDRAWN-CHANNEL",
                    "occurrence_session_id": "agent-channel-4-take",
                    "capture_epoch": 1,
                    "sample_start": 0,
                    "sample_end": 10,
                    "recipients": [&alpha],
                }),
                transcript(
                    "take-a",
                    "same",
                    "CURRENT-PHRASE",
                    2,
                    &alpha,
                    Some("2026-10-04T00:00:00Z"),
                ),
                transcript(
                    "take-a",
                    "same",
                    "WITHDRAWN-OLD-PHRASE",
                    1,
                    &alpha,
                    Some("2026-10-03T00:00:00Z"),
                ),
                beta_reply,
                stolen,
            ],
        );
        write_jsonl(
            &reused,
            &[transcript(
                "take-c",
                "u-c",
                "REUSED-CHANNEL-ONLY",
                1,
                &gamma,
                None,
            )],
        );
        write_released_archive(root, "codex", "alpha-session", "4", "alpha", &shared, None);
        write_released_archive(root, "codex", "beta-session", "4", "beta", &shared, None);
        write_released_archive(root, "codex", "gamma-session", "4", "gamma", &reused, None);

        let alpha_id = conversation_id("codex", "alpha-session");
        let beta_id = conversation_id("codex", "beta-session");
        let gamma_id = conversation_id("codex", "gamma-session");
        assert_ne!(alpha_id, beta_id);
        assert!(alpha_id.contains('\0'));

        let search = |query: &str| search_bus_conversations(root, query).unwrap();
        assert!(search("WITHDRAWN-OLD-PHRASE").results.is_empty());
        assert!(search("WITHDRAWN-CHANNEL").results.is_empty());
        assert!(search("STOLEN-REPLY").results.is_empty());
        assert_eq!(
            matching_ids(&search("CURRENT-PHRASE")),
            vec![alpha_id.as_str()]
        );
        assert_eq!(
            matching_ids(&search("CURRENT-CHANNEL")),
            vec![alpha_id.as_str()]
        );
        assert_eq!(
            matching_ids(&search("OTHER-OWNER-SECRET")),
            vec![beta_id.as_str()]
        );
        assert_eq!(
            matching_ids(&search("REUSED-CHANNEL-ONLY")),
            vec![gamma_id.as_str()]
        );
        assert!(search("CURRENT-PHRASE").complete);
    }

    #[test]
    fn empty_rendered_text_is_not_a_channel_document() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let shared = root.join("shared.jsonl");
        let alpha = owner_json("codex", "alpha-session", "4", "alpha");
        let beta = owner_json("codex", "beta-session", "4", "beta");
        let evidence = |session: &str, revision: u64, rendered: Option<&str>, label: &str| {
            let mut row = json!({
                "schema": "codescribe.transcript-evidence.v1",
                "session_id": session,
                "audience": "alpha",
                "reducer_revision": revision,
                "reducer_action": "revise",
                "label": label,
                "occurrence_session_id": session,
                "capture_epoch": 1,
                "sample_start": 0,
                "sample_end": 10,
                "recipients": [&alpha],
            });
            if let Some(rendered) = rendered {
                row["rendered_text"] = json!(rendered);
            }
            row
        };
        // Root probe order: revision 1 text, revision 3 empty, revision 2 text.
        // Empty is not a tombstone, so revision 2 replaces revision 1.
        write_jsonl(
            &shared,
            &[
                evidence("agent-channel-4-take", 1, Some("REVISION-ONE"), "LABEL-ONE"),
                evidence("agent-channel-4-take", 3, Some(""), "EMPTY-DOC-LABEL"),
                evidence("agent-channel-4-take", 2, Some("REVISION-TWO"), "LABEL-TWO"),
                evidence(
                    "agent-channel-7-held",
                    5,
                    Some("KEPT-DOCUMENT"),
                    "HELD-LABEL",
                ),
                evidence("agent-channel-7-held", 8, None, "ABSENT-DOC-LABEL"),
                evidence("agent-channel-7-held", 9, Some("   "), "BLANK-DOC-LABEL"),
                evidence(
                    "agent-channel-7-held",
                    4,
                    Some("OLDER-REPLAY"),
                    "OLDER-LABEL",
                ),
                transcript("beta-take", "u-beta", "NEIGHBOR-STAYS", 1, &beta, None),
            ],
        );
        write_released_archive(root, "codex", "alpha-session", "4", "alpha", &shared, None);
        write_released_archive(root, "codex", "beta-session", "4", "beta", &shared, None);
        let before = snapshot(root);
        let alpha_id = conversation_id("codex", "alpha-session");
        let beta_id = conversation_id("codex", "beta-session");
        let search = |query: &str| search_bus_conversations(root, query).unwrap();

        assert_eq!(
            matching_ids(&search("REVISION-TWO")),
            vec![alpha_id.as_str()]
        );
        assert_eq!(
            matching_ids(&search("KEPT-DOCUMENT")),
            vec![alpha_id.as_str()]
        );
        assert!(search("REVISION-ONE").results.is_empty());
        assert!(search("OLDER-REPLAY").results.is_empty());
        assert!(search("EMPTY-DOC-LABEL").results.is_empty());
        assert!(search("ABSENT-DOC-LABEL").results.is_empty());
        assert!(search("BLANK-DOC-LABEL").results.is_empty());
        assert_eq!(
            matching_ids(&search("NEIGHBOR-STAYS")),
            vec![beta_id.as_str()]
        );
        let held = search("KEPT-DOCUMENT");
        assert!(held.complete);
        assert_eq!(held.error_count, 0);
        assert_eq!(snapshot(root), before);
    }

    #[test]
    fn partial_history_keeps_other_hits_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let good_bus = root.join("good.jsonl");
        let missing_bus = root.join("missing-history.jsonl");
        let owner = owner_json("codex", "kept-session", "2", "kept");
        if let Some(parent) = good_bus.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut good = fs::File::create(&good_bus).unwrap();
        good.write_all(b"not-json\n").unwrap();
        let mut line = serde_json::to_vec(&transcript(
            "kept-take",
            "u1",
            "KEEP-HIT",
            1,
            &owner,
            Some("2026-08-01T00:00:00Z"),
        ))
        .unwrap();
        line.push(b'\n');
        good.write_all(&line).unwrap();
        write_released_archive(root, "codex", "kept-session", "2", "kept", &good_bus, None);
        write_released_archive(
            root,
            "codex",
            "orphan-session",
            "3",
            "orphan-label",
            &missing_bus,
            None,
        );
        let bad_lease = "0123456789abcdef0123456789abcdef";
        write_json(
            &root.join("archives").join(format!("{bad_lease}-1.json")),
            &json!({
                "schema": "codescribe.agent-archive.v1",
                "released": true,
                "provider": "codex",
                "provider_session_id": "bad-lease-session",
                "lease_id": bad_lease,
                "channel": "1",
                "name": "bad-lease",
                "bus": root.join("bad.jsonl"),
            }),
        );
        let hidden = root.join("hidden.jsonl");
        let hidden_owner = owner_json("codex", "hidden-session", "6", "hidden");
        write_jsonl(
            &hidden,
            &[transcript(
                "hidden-take",
                "u1",
                "UNRELEASED-SECRET",
                1,
                &hidden_owner,
                None,
            )],
        );
        let hidden_lease = lease_identifier("codex", "hidden-session");
        write_json(
            &root.join("archives").join(format!("{hidden_lease}-6.json")),
            &json!({
                "schema": "codescribe.agent-archive.v1",
                "released": false,
                "provider": "codex",
                "provider_session_id": "hidden-session",
                "lease_id": hidden_lease,
                "channel": "6",
                "name": "hidden",
                "bus": hidden,
            }),
        );

        let before = snapshot(root);
        let found = search_bus_conversations(root, "").unwrap();
        assert_eq!(snapshot(root), before);
        assert!(!found.complete);
        assert_eq!(found.error_count, 3);
        let ids = matching_ids(&found);
        assert!(ids.contains(&conversation_id("codex", "kept-session").as_str()));
        assert!(ids.contains(&conversation_id("codex", "orphan-session").as_str()));
        assert!(
            !ids.iter()
                .any(|id| id.contains("hidden-session") || id.contains("bad-lease"))
        );
        assert_eq!(
            matching_ids(&search_bus_conversations(root, "KEEP-HIT").unwrap()),
            vec![conversation_id("codex", "kept-session").as_str()]
        );
        assert!(
            search_bus_conversations(root, "UNRELEASED-SECRET")
                .unwrap()
                .results
                .is_empty()
        );
        assert!(
            search_bus_conversations(root, "orphan-label")
                .unwrap()
                .results
                .len()
                == 1
        );
    }

    #[test]
    fn empty_query_returns_verified_archives_unknown_dates_last() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for (session, text, emitted, archive_time) in [
            (
                "newest-session",
                "newest",
                Some("2026-10-01T00:00:00Z"),
                Some("2020-01-01T00:00:00Z"),
            ),
            (
                "middle-session",
                "middle",
                Some("2026-03-01T00:00:00Z"),
                None,
            ),
            (
                "oldest-session",
                "oldest",
                Some("2026-01-02T00:00:00Z"),
                None,
            ),
            ("undated-b", "undated-b", None, None),
            ("undated-a", "undated-a", None, None),
        ] {
            let bus = root.join(format!("{session}.jsonl"));
            let owner = owner_json("codex", session, "4", session);
            write_jsonl(&bus, &[transcript(session, "u1", text, 1, &owner, emitted)]);
            write_released_archive(root, "codex", session, "4", session, &bus, archive_time);
        }
        write_json(
            &root.join("vc.agent-audience-binding.v1.json"),
            &json!({
                "schema": "vc.agent-audience-binding.v1",
                "bindings": {
                    "4": {
                        "audience": "adam",
                        "provider": "codex",
                        "provider_session_id": "newest-session"
                    }
                }
            }),
        );

        let found = search_bus_conversations(root, "").unwrap();
        assert!(found.complete);
        assert_eq!(found.error_count, 0);
        assert_eq!(
            matching_ids(&found),
            vec![
                conversation_id("codex", "newest-session").as_str(),
                conversation_id("codex", "middle-session").as_str(),
                conversation_id("codex", "oldest-session").as_str(),
                conversation_id("codex", "undated-a").as_str(),
                conversation_id("codex", "undated-b").as_str(),
            ]
        );
        assert_eq!(found.results[0].last_activity, "2026-10-01T00:00:00Z");
        assert!(found.results[3].last_activity.is_empty());
        assert!(found.results[4].last_activity.is_empty());
    }

    #[test]
    fn symlink_outside_archives_is_partial_and_does_not_mutate() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let outside = root.join("outside.json");
        let phrase = "SYMLINK-SECRET";
        write_json(
            &outside,
            &json!({
                "schema": "codescribe.agent-archive.v1",
                "released": true,
                "text": phrase,
            }),
        );
        fs::create_dir_all(root.join("archives")).unwrap();
        std::os::unix::fs::symlink(
            &outside,
            root.join("archives")
                .join("0123456789abcdef0123456789abcdef-1.json"),
        )
        .unwrap();
        let neighbor = root.join("neighbor.jsonl");
        let owner = owner_json("codex", "neighbor-session", "8", "neighbor");
        write_jsonl(
            &neighbor,
            &[transcript(
                "neighbor-take",
                "u1",
                "NEIGHBOR-HIT",
                1,
                &owner,
                None,
            )],
        );
        write_released_archive(
            root,
            "codex",
            "neighbor-session",
            "8",
            "neighbor",
            &neighbor,
            None,
        );
        let before = snapshot(root);
        let found = search_bus_conversations(root, "").unwrap();
        assert_eq!(snapshot(root), before);
        assert!(!found.complete);
        assert!(found.error_count >= 1);
        assert_eq!(
            matching_ids(&found),
            vec![conversation_id("codex", "neighbor-session").as_str()]
        );
        assert!(
            search_bus_conversations(root, phrase)
                .unwrap()
                .results
                .is_empty()
        );
    }
}
