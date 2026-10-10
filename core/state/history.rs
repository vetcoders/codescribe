//! Simple transcript history manager for Codescribe
//!
//! Saves transcripts and audio to ~/.codescribe/transcriptions/YYYY-MM-DD/
//! Files are paired: HHMMSS_slug_kind.m4a + HHMMSS_slug_kind.txt with matching timestamps.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use deunicode::deunicode;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{error, info, warn};

use crate::pipeline::take_truth::{TakeTruth, write_truth_sidecar};

/// Audio policy maintenance belongs to this history owner; metadata is stored
/// beside the existing session WAV, never in a second audio archive.
#[cfg(unix)]
#[path = "audio_retention.rs"]
pub mod audio_retention;

/// Audio containers an archived recording may use: `m4a` normally, `wav` when
/// encoding failed and the raw copy was kept as a fallback.
const AUDIO_ARCHIVE_EXTENSIONS: &[&str] = &["m4a", "wav"];
const NO_SPEECH_HISTORY_TITLE: &str = "(no speech)";

/// A single history entry
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Path of the `.txt` transcript on disk.
    pub path: PathBuf,
    /// File modification time, used for ordering.
    pub timestamp: DateTime<Local>,
    /// First line, truncated to 60 characters, for menu display.
    pub preview: String,
    /// Artifact kind recovered from the filename suffix.
    pub kind: TranscriptKind,
}

/// Which artifact of a dictation a file holds.
///
/// One recording can produce several of these, and the kind is encoded in the
/// filename suffix — it is what lets history tell a raw draft apart from its
/// formatted final, and a real transcript apart from a failure marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptKind {
    /// Local engine output, before any formatting pass.
    Raw,
    /// Cloud transcription output.
    Cloud,
    /// Transcript after a successful LLM formatting pass.
    FormattedTranscript,
    /// Assistant answer to a spoken instruction, not a transcript of it.
    AssistantInterpretation,
    /// Formatting failed; the text is the unformatted fallback.
    FormattingFailed,
    /// The dictation produced no usable transcript.
    Failed,
}

/// Transcript outcome accepted by the session archive boundary.
///
/// Diagnostics deliberately have their own variant, so a lane or transport
/// error cannot be confused with committed user speech and written into the
/// transcript document. `NoSpeech` is the only non-transcript outcome that
/// creates a visible history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTranscriptArchive<'a> {
    /// Reducer-committed user speech.
    Committed(&'a str),
    /// A successful take with no committed words.
    NoSpeech,
    /// A lane, transport, or seal failure. The diagnostic is never persisted
    /// as transcript text; its owner reports it through status/log surfaces.
    Unavailable(&'a str),
}

impl<'a> SessionTranscriptArchive<'a> {
    /// Classify a reducer render without letting an empty string masquerade as
    /// committed speech.
    pub fn from_committed(text: &'a str) -> Self {
        if text.trim().is_empty() {
            Self::NoSpeech
        } else {
            Self::Committed(text)
        }
    }
}

impl TranscriptKind {
    /// Filename suffix for this kind. Writes emit these; [`kind_from_suffix`]
    /// reads them back.
    fn suffix(self) -> &'static str {
        match self {
            TranscriptKind::Raw => "raw",
            TranscriptKind::Cloud => "cloud",
            TranscriptKind::FormattedTranscript => "formatted",
            TranscriptKind::AssistantInterpretation => "interpretation",
            TranscriptKind::FormattingFailed => "formatting-failed",
            TranscriptKind::Failed => "failed",
        }
    }

    /// Whether this artifact is real transcript text a user would want pasted.
    ///
    /// Excludes failure markers and assistant answers, so "copy last transcript"
    /// never hands back "no speech detected" or a chat reply.
    pub fn is_copyable_transcript(self) -> bool {
        matches!(
            self,
            TranscriptKind::Raw
                | TranscriptKind::Cloud
                | TranscriptKind::FormattedTranscript
                | TranscriptKind::FormattingFailed
        )
    }
}

/// Derive product-facing history text from artifact class. Failure payloads
/// are diagnostics, including legacy rows that may still contain an error
/// string; their only permitted title is the explicit no-speech sentinel.
fn history_preview(kind: TranscriptKind, text: &str) -> String {
    if kind == TranscriptKind::Failed {
        return NO_SPEECH_HISTORY_TITLE.to_string();
    }
    text.trim()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(60)
        .collect()
}

/// Tally of one [`migrate_transcriptions`] pass, dry-run or applied.
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Transcripts renamed to the canonical scheme.
    pub renamed_text: usize,
    /// Audio files renamed, including orphans with no transcript.
    pub renamed_audio: usize,
    /// Files already canonical, or skipped as unreadable.
    pub skipped: usize,
    /// Failures encountered; migration continues past them.
    pub errors: usize,
}

impl HistoryEntry {
    /// Get a formatted label for display in menus
    pub fn label(&self) -> String {
        let ts = self.timestamp.format("%H:%M:%S").to_string();
        if self.preview.is_empty() {
            ts
        } else {
            format!("{} – {}", ts, self.preview)
        }
    }
}

/// Create a filename-safe slug from the first N words of text
/// Returns empty string if no valid words found
fn make_slug(text: &str, max_words: usize) -> String {
    let ascii = deunicode(text);
    let slug: String = ascii
        .split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_lowercase();

    // Limit length to avoid filesystem issues
    if slug.len() > 30 {
        slug.chars().take(30).collect()
    } else {
        slug
    }
}

/// Assemble the canonical stem `HHMMSS[_slug]_kind`.
///
/// The slug is omitted entirely when empty, rather than leaving a double
/// underscore that the parser would have to special-case.
fn build_base_name(time_base: &str, slug: &str, kind: TranscriptKind) -> String {
    if slug.is_empty() {
        format!("{}_{}", time_base, kind.suffix())
    } else {
        format!("{}_{}_{}", time_base, slug, kind.suffix())
    }
}

/// Recover a kind from a filename suffix, `None` if the suffix is not one.
///
/// Accepts the retired `ai` / `ai-failed` spellings so files written before the
/// rename still classify correctly instead of silently reading as `Raw`.
fn kind_from_suffix(suffix: &str) -> Option<TranscriptKind> {
    match suffix {
        "raw" => Some(TranscriptKind::Raw),
        "cloud" => Some(TranscriptKind::Cloud),
        "ai" | "formatted" => Some(TranscriptKind::FormattedTranscript),
        "ai-failed" | "formatting-failed" => Some(TranscriptKind::FormattingFailed),
        "interpretation" => Some(TranscriptKind::AssistantInterpretation),
        "failed" => Some(TranscriptKind::Failed),
        _ => None,
    }
}

/// Split a stem into `(kind, base, collision index)`.
///
/// Handles both `..._raw` and `..._raw_1`, the latter produced when two saves
/// land in the same second. A stem with no recognizable suffix keeps
/// `default_kind` and is returned whole as the base.
fn split_kind_and_index(
    stem: &str,
    default_kind: TranscriptKind,
) -> (TranscriptKind, String, Option<String>) {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let second_last = parts[parts.len() - 2];
        let last_is_num = last.chars().all(|c| c.is_ascii_digit());

        if last_is_num && let Some(kind) = kind_from_suffix(second_last) {
            let base = parts[..parts.len() - 2].join("_");
            return (kind, base, Some(last.to_string()));
        }

        if let Some(kind) = kind_from_suffix(last) {
            let base = parts[..parts.len() - 1].join("_");
            return (kind, base, None);
        }
    }

    (default_kind, stem.to_string(), None)
}

/// Split a kind-stripped stem into its `HHMMSS` prefix and slug remainder.
///
/// Only a leading run of exactly six digits counts as a time base; anything
/// else is treated as slug text, leaving the caller to source a timestamp from
/// file metadata instead of misreading arbitrary digits as a clock.
fn split_time_and_slug(stem: &str) -> (Option<String>, Option<String>) {
    let stem = stem.trim_start_matches('_');
    if stem.len() >= 6 && stem.chars().take(6).all(|c| c.is_ascii_digit()) {
        let time_base = stem[..6].to_string();
        let rest = stem.get(6..).unwrap_or("").trim_start_matches('_');
        let slug_hint = if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        };
        (Some(time_base), slug_hint)
    } else if stem.is_empty() {
        (None, None)
    } else {
        (None, Some(stem.to_string()))
    }
}

/// Derive an `HHMMSS` base from the file's mtime, for names that carry no time.
fn time_base_from_metadata(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let timestamp = DateTime::<Local>::from(modified);
    Some(timestamp.format("%H%M%S").to_string())
}

/// Rebuild a slug from an existing filename fragment.
///
/// Separators become spaces first, so an old `czesc-jak-sie` fragment re-slugs
/// to the same three words [`make_slug`] would produce from transcript text.
fn slug_from_hint(hint: &str) -> String {
    let normalized = hint.replace(['_', '-'], " ");
    make_slug(&normalized, 3)
}

/// Find a stem that collides with no existing transcript or audio file.
///
/// The file being renamed is passed as `old_txt` / `old_audio` and excluded
/// from the check, so a file never counts as a conflict with itself. Appends
/// `_1`, `_2`, … and gives up after 10 000 attempts, returning `base` unchanged.
fn choose_unique_base(
    dir: &Path,
    base: &str,
    old_txt: Option<&Path>,
    old_audio: Option<&Path>,
    check_txt: bool,
) -> String {
    let mut candidate = base.to_string();
    for i in 0..=10_000 {
        let txt_path = dir.join(format!("{}.txt", candidate));
        let txt_conflict = check_txt && txt_path.exists() && (old_txt != Some(txt_path.as_path()));
        let audio_conflict = AUDIO_ARCHIVE_EXTENSIONS.iter().any(|ext| {
            let audio_path = dir.join(format!("{}.{}", candidate, ext));
            audio_path.exists() && (old_audio != Some(audio_path.as_path()))
        });

        if !txt_conflict && !audio_conflict {
            return candidate;
        }

        candidate = format!("{}_{}", base, i + 1);
    }

    base.to_string()
}

/// Locate the audio file paired with a transcript stem, in either container.
fn existing_audio_for_stem(dir: &Path, stem: &str) -> Option<PathBuf> {
    AUDIO_ARCHIVE_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.exists())
}

/// The audio the daily archive wrote together with one transcript.
///
/// `daily_archive::save` publishes `<base>.{m4a,wav}` and `<base>.txt` from
/// one stem, so the stem IS the durable take identity. Only a `.txt` inside
/// the transcriptions bag qualifies; anything else, or a transcript whose
/// paired audio is gone, yields `None` — never a neighbour, never the last
/// recording.
pub fn paired_audio_for_transcript(transcript: &Path) -> Option<PathBuf> {
    if transcript.extension().and_then(|ext| ext.to_str()) != Some("txt") {
        return None;
    }
    let resolved = transcript.canonicalize().ok()?;
    let root = transcriptions_base_dir().canonicalize().ok()?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return None;
    }
    let stem = resolved.file_stem()?.to_str()?;
    existing_audio_for_stem(resolved.parent()?, stem)
}

// ─────────────────────────────────────────────────────────────────────────────
// Archived transcript revisions
// ─────────────────────────────────────────────────────────────────────────────

const ARCHIVE_REVISION_SCHEMA: &str = "codescribe.archive-revision.v1";
const ARCHIVE_REVISION_SUFFIX: &str = ".revisions.jsonl";

/// What produced one revision of an archived transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveRevisionProvenance {
    UserEdit,
    Formatter,
    Retranscribe,
    /// An earlier version restored as a new revision. Legacy records stay
    /// readable and count as an operation; new writes use `Navigate`.
    Restore,
    /// Undo, redo or a version pick: the cursor moves to an accepted step.
    /// A receipt for the move, never a new step.
    Navigate,
}

impl ArchiveRevisionProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserEdit => "user-edit",
            Self::Formatter => "formatter",
            Self::Retranscribe => "retranscribe",
            Self::Restore => "restore",
            Self::Navigate => "navigate",
        }
    }
}

/// One persisted revision of an archived transcript.
///
/// Revision 0 is the archived `.txt` itself and is never written here. Every
/// line names the raw evidence it revises by digest, so a chain can never be
/// read back over a different transcript.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveRevision {
    pub schema: String,
    pub revision: u64,
    pub source_revision: u64,
    pub provenance: ArchiveRevisionProvenance,
    pub rendered_text: String,
    pub receipt_id: String,
    pub emitted_at: String,
    pub evidence_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_revision: Option<u64>,
    /// Formatter level or retranscription pass, for provenance display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An archived transcript with its revision chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedDocument {
    pub original_text: String,
    pub revisions: Vec<ArchiveRevision>,
}

impl ArchivedDocument {
    /// The accepted revision; 0 while the archive is unrevised.
    pub fn head_revision(&self) -> u64 {
        self.revisions
            .last()
            .map_or(0, |revision| revision.revision)
    }

    pub fn head(&self) -> Option<&ArchiveRevision> {
        self.revisions.last()
    }

    pub fn head_text(&self) -> &str {
        self.head().map_or(self.original_text.as_str(), |revision| {
            revision.rendered_text.as_str()
        })
    }

    pub fn text_at(&self, revision: u64) -> Option<&str> {
        if revision == 0 {
            return Some(&self.original_text);
        }
        self.revisions
            .iter()
            .find(|entry| entry.revision == revision)
            .map(|entry| entry.rendered_text.as_str())
    }

    /// Replay the chain as one linear history: the original and every
    /// accepted operation, with the selected step. An operation after an undo
    /// ends the abandoned redo branch; a navigation only moves the cursor.
    /// Equal texts stay separate steps: each is an attempt the user made.
    pub fn timeline(&self) -> ArchiveTimeline {
        let mut steps = vec![ArchiveStep {
            revision: 0,
            provenance: "original".to_string(),
            detail: None,
            rendered_text: self.original_text.clone(),
            emitted_at: String::new(),
            receipt_id: String::new(),
        }];
        let mut cursor = 0;
        for record in &self.revisions {
            if record.provenance == ArchiveRevisionProvenance::Navigate {
                if let Some(target) = record
                    .restored_revision
                    .and_then(|target| steps.iter().position(|step| step.revision == target))
                {
                    cursor = target;
                }
                continue;
            }
            steps.truncate(cursor + 1);
            steps.push(ArchiveStep {
                revision: record.revision,
                provenance: record.provenance.as_str().to_string(),
                detail: record.detail.clone(),
                rendered_text: record.rendered_text.clone(),
                emitted_at: record.emitted_at.clone(),
                receipt_id: record.receipt_id.clone(),
            });
            cursor = steps.len() - 1;
        }
        ArchiveTimeline { steps, cursor }
    }
}

/// One accepted step of an archived transcript. `revision` is the chain
/// record that accepted it (0 for the archived original): its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveStep {
    pub revision: u64,
    pub provenance: String,
    pub detail: Option<String>,
    pub rendered_text: String,
    pub emitted_at: String,
    pub receipt_id: String,
}

/// The replayed linear history of an archived transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTimeline {
    pub steps: Vec<ArchiveStep>,
    pub cursor: usize,
}

/// Admit one archived transcript: a `.txt` file inside the transcriptions bag.
fn admit_archived_transcript(transcript: &Path) -> Result<daily_archive::ArchivePin> {
    anyhow::ensure!(
        transcript.extension().and_then(|ext| ext.to_str()) == Some("txt"),
        "not an archived transcript: {}",
        transcript.display()
    );
    let resolved = transcript
        .canonicalize()
        .with_context(|| format!("archived transcript missing: {}", transcript.display()))?;
    let root = transcriptions_base_dir()
        .canonicalize()
        .context("transcriptions folder unavailable")?;
    anyhow::ensure!(
        resolved.starts_with(&root) && resolved.is_file(),
        "not an archived transcript: {}",
        transcript.display()
    );
    daily_archive::admit_revision_source(&resolved, &root)
}

fn evidence_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The persisted chain is JSON Lines with one complete-record rule: a record
/// is accepted only together with its terminating newline. Bytes after the
/// last newline were never acknowledged to any caller (an interrupted legacy
/// append, a split UTF-8 character, even a whole JSON object): they are not
/// part of the chain, they are never parsed as a record, and a writer removes
/// them before it appends. Every complete record must be valid, in order and
/// bound to the same raw evidence, or the whole chain is refused untouched.
struct ParsedChain {
    revisions: Vec<ArchiveRevision>,
    accepted_len: usize,
}

fn parse_revision_chain(chain: &[u8], evidence: &str, label: &Path) -> Result<ParsedChain> {
    let accepted_len = chain
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut revisions: Vec<ArchiveRevision> = Vec::new();
    if let Some(body) = chain[..accepted_len].strip_suffix(b"\n") {
        for (index, line) in body.split(|byte| *byte == b'\n').enumerate() {
            let revision = std::str::from_utf8(line)
                .ok()
                .and_then(|line| serde_json::from_str::<ArchiveRevision>(line).ok())
                .with_context(|| {
                    format!("corrupt revision {} in {}", index + 1, label.display())
                })?;
            let expected = revisions.len() as u64 + 1;
            anyhow::ensure!(
                revision.schema == ARCHIVE_REVISION_SCHEMA
                    && revision.revision == expected
                    && revision.source_revision.checked_add(1) == Some(expected),
                "revision chain out of order in {}",
                label.display()
            );
            anyhow::ensure!(
                revision.evidence_sha256 == evidence,
                "archived transcript changed under its revision chain: {}",
                label.display()
            );
            revisions.push(revision);
        }
    }
    Ok(ParsedChain {
        revisions,
        accepted_len,
    })
}

fn archived_document_from(
    transcript: &[u8],
    chain: &[u8],
    label: &Path,
) -> Result<(ArchivedDocument, usize)> {
    let original_text = String::from_utf8(transcript.to_vec())
        .with_context(|| format!("archived transcript is not UTF-8: {}", label.display()))?;
    let parsed = parse_revision_chain(chain, &evidence_digest(&original_text), label)?;
    Ok((
        ArchivedDocument {
            original_text,
            revisions: parsed.revisions,
        },
        parsed.accepted_len,
    ))
}

/// One planned rewrite of a revision chain: the full accepted contents, the
/// unacknowledged tail it drops (kept aside as evidence) and the receipt.
struct ChainWrite<T> {
    contents: Vec<u8>,
    uncommitted_tail: Vec<u8>,
    value: T,
}

/// The archived transcript at `transcript` with its accepted revisions.
/// Readers hold the shared directory lease, so they see one whole chain.
pub fn read_archived_document(transcript: &Path) -> Result<ArchivedDocument> {
    let transcript = admit_archived_transcript(transcript)?;
    let (raw, chain) = daily_archive::read_revision_chain(&transcript)?;
    let (document, accepted_len) = archived_document_from(&raw, &chain, &transcript.path)?;
    if accepted_len < chain.len() {
        warn!(
            "archived transcript {} has {} unacknowledged revision bytes; the next accepted revision sets them aside",
            transcript.path.display(),
            chain.len() - accepted_len
        );
    }
    Ok(document)
}

/// What an accepted revision says, decided against the current document.
struct PlannedRevision {
    provenance: ArchiveRevisionProvenance,
    rendered_text: String,
    restored_revision: Option<u64>,
    detail: Option<String>,
}

/// Accept one revision under the exclusive directory lease: validate the raw
/// evidence and the accepted prefix, compare the head, then rewrite the chain
/// as that prefix plus exactly one new record. The receipt exists only after
/// the rewrite and its directory entry are durable.
fn accept_archived_revision(
    transcript: &Path,
    source_revision: u64,
    plan: impl FnOnce(&ArchivedDocument) -> Result<PlannedRevision>,
) -> Result<ArchiveRevision> {
    let transcript = admit_archived_transcript(transcript)?;
    daily_archive::update_revision_chain(&transcript, |raw, chain| {
        let (document, accepted_len) = archived_document_from(raw, chain, &transcript.path)?;
        anyhow::ensure!(
            document.head_revision() == source_revision,
            "stale archived transcript revision: current {}, requested {}",
            document.head_revision(),
            source_revision
        );
        let PlannedRevision {
            provenance,
            rendered_text,
            restored_revision,
            detail,
        } = plan(&document)?;
        let revision = ArchiveRevision {
            schema: ARCHIVE_REVISION_SCHEMA.to_string(),
            revision: source_revision
                .checked_add(1)
                .context("archived transcript revision exhausted")?,
            source_revision,
            provenance,
            rendered_text,
            receipt_id: format!("archive-{}-{}", provenance.as_str(), uuid::Uuid::new_v4()),
            emitted_at: chrono::Utc::now().to_rfc3339(),
            evidence_sha256: evidence_digest(&document.original_text),
            restored_revision,
            detail,
        };
        let mut contents = chain[..accepted_len].to_vec();
        contents.extend_from_slice(&serde_json::to_vec(&revision)?);
        contents.push(b'\n');
        Ok(ChainWrite {
            contents,
            uncommitted_tail: chain[accepted_len..].to_vec(),
            value: revision,
        })
    })
}

/// Accept one revision of an archived transcript against `source_revision`.
///
/// The archived `.txt` and its audio are raw evidence and stay untouched; the
/// chain lives in `<base>.txt.revisions.jsonl` beside them, which no history
/// listing treats as a transcript. A head that moved since the caller read it
/// refuses the request instead of overwriting newer work, across processes.
pub fn commit_archived_revision(
    transcript: &Path,
    source_revision: u64,
    rendered_text: &str,
    provenance: ArchiveRevisionProvenance,
    detail: Option<String>,
) -> Result<ArchiveRevision> {
    anyhow::ensure!(
        !matches!(
            provenance,
            ArchiveRevisionProvenance::Restore | ArchiveRevisionProvenance::Navigate
        ),
        "a navigation names the version it selects"
    );
    anyhow::ensure!(
        !rendered_text.trim().is_empty(),
        "A transcript revision cannot be empty"
    );
    accept_archived_revision(transcript, source_revision, |document| {
        // A retranscription is an attempt even when it hears the same words.
        anyhow::ensure!(
            provenance == ArchiveRevisionProvenance::Retranscribe
                || document.head_text() != rendered_text,
            "the revision does not change the transcript"
        );
        Ok(PlannedRevision {
            provenance,
            rendered_text: rendered_text.to_string(),
            restored_revision: None,
            detail,
        })
    })
}

/// Undo, redo or pick a version of an archived transcript: select the
/// accepted step `target_revision` names. Appends one navigation receipt;
/// no step is added and no text is produced again.
pub fn navigate_archived_revision(
    transcript: &Path,
    source_revision: u64,
    target_revision: u64,
) -> Result<ArchiveRevision> {
    accept_archived_revision(transcript, source_revision, |document| {
        let timeline = document.timeline();
        let target = timeline
            .steps
            .iter()
            .position(|step| step.revision == target_revision)
            .context("selected version is not in this transcript's history")?;
        anyhow::ensure!(
            target != timeline.cursor,
            "the selected version is already shown"
        );
        Ok(PlannedRevision {
            provenance: ArchiveRevisionProvenance::Navigate,
            rendered_text: timeline.steps[target].rendered_text.clone(),
            restored_revision: Some(target_revision),
            detail: None,
        })
    })
}

/// Get the transcriptions base directory
fn transcriptions_base_dir() -> PathBuf {
    // Use config_dir as the single source of truth for filesystem roots.
    // This keeps behavior identical in normal runs (defaults to $HOME/.codescribe)
    // while allowing deterministic overrides in tests via CODESCRIBE_DATA_DIR.
    crate::config::Config::config_dir().join("transcriptions")
}

/// Get the transcriptions directory for a specific date, creating it if needed
pub fn transcriptions_dir(date: &DateTime<Local>) -> PathBuf {
    let base = transcriptions_base_dir();
    let date_folder = date.format("%Y-%m-%d").to_string();
    let dir = base.join(date_folder);

    if !dir.exists()
        && let Err(e) = fs::create_dir_all(&dir)
    {
        error!("Failed to create transcriptions directory: {}", e);
    }

    dir
}

/// Get the history directory, creating it if needed
/// Note: Now an alias for transcriptions_dir with current date for backwards compatibility
pub fn history_dir() -> PathBuf {
    transcriptions_dir(&Local::now())
}

// ─────────────────────────────────────────────────────────────────────────────
// Voice Drafts - for Mission Control overlay
// ─────────────────────────────────────────────────────────────────────────────

/// Get the drafts directory, creating it if needed
/// Drafts are voice transcriptions saved for later editing/review
pub fn drafts_dir() -> PathBuf {
    let dir = crate::config::Config::config_dir().join("drafts");

    if !dir.exists()
        && let Err(e) = fs::create_dir_all(&dir)
    {
        error!("Failed to create drafts directory: {}", e);
    }

    dir
}

/// Save a voice draft and return the file path
/// Drafts are saved as: ~/.codescribe/drafts/YYYY-MM-DD_HH-MM-SS.txt
pub fn save_draft(text: &str) -> PathBuf {
    let now = Local::now();
    let filename = format!("{}.txt", now.format("%Y-%m-%d_%H-%M-%S"));
    let path = drafts_dir().join(&filename);

    match fs::write(&path, text) {
        Ok(_) => info!("Saved voice draft: {}", path.display()),
        Err(e) => error!("Failed to save voice draft: {}", e),
    }

    path
}

/// List all draft files, sorted by modification time (newest first)
pub fn list_drafts() -> Vec<PathBuf> {
    let dir = drafts_dir();

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "txt")
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();

    // Sort by filename (which contains timestamp) - newest first
    entries.sort_by(|a, b| b.cmp(a));

    entries
}

/// Get draft content by path
pub fn read_draft(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Delete a draft file
pub fn delete_draft(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

/// Save a transcript to history and return the entry
///
/// # Arguments
/// * `text` - The transcript text to save
/// * `timestamp` - Optional timestamp to use (for pairing with audio files).
///   If None, uses current time.
/// * `kind` - What kind of transcript artifact this is
pub fn save_entry_with_timestamp(
    text: &str,
    timestamp: Option<DateTime<Local>>,
    kind: TranscriptKind,
) -> HistoryEntry {
    save_entry_with_timestamp_and_slug(text, timestamp, kind, None)
}

/// Save a transcript to history with explicit slug source (for consistent pairing)
pub fn save_entry_with_timestamp_and_slug(
    text: &str,
    timestamp: Option<DateTime<Local>>,
    kind: TranscriptKind,
    slug_hint: Option<&str>,
) -> HistoryEntry {
    let text = text.trim();
    let now = timestamp.unwrap_or_else(Local::now);

    let base = build_base_name(
        &now.format("%H%M%S").to_string(),
        &make_slug(slug_hint.unwrap_or(text), 3),
        kind,
    );
    let intended = transcriptions_base_dir()
        .join(now.format("%Y-%m-%d").to_string())
        .join(format!("{base}.txt"));
    let path =
        match daily_archive::save_text(&crate::config::Config::config_dir(), &now, &base, text) {
            Ok(path) => path,
            Err(error) => {
                error!(
                    "Failed to save transcript {}: {error:#}",
                    intended.display()
                );
                // Legacy return type cannot express failure; no file is invented.
                intended
            }
        };

    let preview = history_preview(kind, text);

    HistoryEntry {
        path,
        timestamp: now,
        preview,
        kind,
    }
}

/// Save a transcript to history and return the entry (convenience wrapper)
pub fn save_entry_with_kind(text: &str, kind: TranscriptKind) -> HistoryEntry {
    save_entry_with_timestamp(text, None, kind)
}

/// Save a transcript to history and return the entry (convenience wrapper)
pub fn save_entry(text: &str) -> HistoryEntry {
    save_entry_with_timestamp(text, None, TranscriptKind::Raw)
}

/// Get recent history entries, sorted by modification time (newest first)
pub fn recent_entries(limit: usize) -> Vec<HistoryEntry> {
    let base_dir = transcriptions_base_dir();
    let mut entries = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();

    // Collect all .txt files from date subdirectories
    if let Ok(day_dirs) = fs::read_dir(&base_dir) {
        for day_entry in day_dirs.flatten() {
            if day_entry.path().is_dir()
                && let Ok(txt_files) = fs::read_dir(day_entry.path())
            {
                for txt_entry in txt_files.flatten() {
                    let path = txt_entry.path();
                    if path.extension().is_some_and(|ext| ext == "txt") {
                        files.push(path);
                    }
                }
            }
        }
    }

    // Sort by modification time (newest first)
    files.sort_by(|a, b| {
        let a_time = fs::metadata(a).and_then(|m| m.modified()).ok();
        let b_time = fs::metadata(b).and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    // Collapse same-save families: a raw draft and its post-processed final can
    // land within the same second as `base.txt` + `base_1.txt` (see the collision
    // suffix in save_entry_with_timestamp_and_slug). History surfaces one row per
    // dictation — newest file of each (dir, base, kind) family wins.
    let mut seen_families: HashSet<(PathBuf, String, &'static str)> = HashSet::new();
    let files: Vec<PathBuf> = files
        .into_iter()
        .filter(|path| {
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let (kind, base, _) = split_kind_and_index(stem, TranscriptKind::Raw);
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            // Distinct paired takes must remain discoverable even in one second.
            let family = if existing_audio_for_stem(&dir, stem).is_some() {
                stem.to_string()
            } else {
                base
            };
            seen_families.insert((dir, family, kind.suffix()))
        })
        .collect();

    // Take the requested limit and create entries
    for path in files.into_iter().take(limit) {
        let timestamp = fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(DateTime::<Local>::from)
            .unwrap_or_else(|_| Local::now());
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let (kind, _, _) = split_kind_and_index(stem, TranscriptKind::Raw);

        let preview = history_preview(kind, &fs::read_to_string(&path).unwrap_or_default());

        entries.push(HistoryEntry {
            path,
            timestamp,
            preview,
            kind,
        });
    }

    entries
}

/// Get the latest history entry, if any
pub fn latest_entry() -> Option<HistoryEntry> {
    recent_entries(1).into_iter().next()
}

/// Get the latest entry that is still a transcript artifact suitable for copy/paste.
///
/// This skips explicit failure markers and non-transcript categories so tray
/// actions do not present "last transcript" as a failed/no-speech artifact.
pub fn latest_copyable_entry() -> Option<HistoryEntry> {
    recent_entries(256)
        .into_iter()
        .find(|entry| entry.kind.is_copyable_transcript())
}

/// Open the transcriptions folder in Finder
pub fn open_history_folder() {
    let dir = transcriptions_base_dir();
    if let Err(e) = Command::new("open").arg(&dir).spawn() {
        error!("Failed to open transcriptions folder: {}", e);
    }
}

/// Migrate existing transcript/audio filenames to ASCII + suffix naming.
///
/// Notes:
/// - Existing suffixes (_raw/_ai/_ai-failed/_failed) are preserved.
/// - Files without suffix use `assume_kind`.
/// - Slugs are regenerated from transcript text when possible.
/// - Audio files are renamed to match their transcript when paired.
pub fn migrate_transcriptions(
    assume_kind: TranscriptKind,
    dry_run: bool,
) -> Result<MigrationReport> {
    let base_dir = transcriptions_base_dir();
    let mut report = MigrationReport::default();

    if !base_dir.exists() {
        warn!(
            "No transcriptions directory found at {}",
            base_dir.display()
        );
        return Ok(report);
    }

    let day_dirs = fs::read_dir(&base_dir)
        .with_context(|| format!("Failed to read {}", base_dir.display()))?;

    for day_entry in day_dirs {
        let day_entry = match day_entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!("Failed to read entry in {}: {}", base_dir.display(), e);
                report.errors += 1;
                continue;
            }
        };
        let day_path = day_entry.path();
        if !day_path.is_dir() {
            continue;
        }

        let mut txt_files = Vec::new();
        let mut audio_files = Vec::new();
        let entries = match fs::read_dir(&day_path) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Failed to read {}: {}", day_path.display(), e);
                report.errors += 1;
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|s| s.to_str()) {
                Some("txt") => txt_files.push(path),
                Some("wav" | "m4a") => audio_files.push(path),
                _ => {}
            }
        }

        let mut handled_audio: HashSet<PathBuf> = HashSet::new();

        for txt_path in txt_files {
            let dir = txt_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| day_path.clone());
            let stem = match txt_path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem.to_string(),
                None => {
                    warn!("Skipping non-UTF8 transcript name: {}", txt_path.display());
                    report.skipped += 1;
                    continue;
                }
            };

            let (kind, base_stem, index) = split_kind_and_index(&stem, assume_kind);
            let (time_base_opt, slug_hint) = split_time_and_slug(&base_stem);
            let time_base = time_base_opt
                .or_else(|| time_base_from_metadata(&txt_path))
                .unwrap_or_else(|| Local::now().format("%H%M%S").to_string());

            let text = fs::read_to_string(&txt_path).ok();
            let mut slug = text.as_deref().map(|t| make_slug(t, 3)).unwrap_or_default();
            if slug.is_empty()
                && let Some(hint) = slug_hint.as_deref()
            {
                slug = slug_from_hint(hint);
            }

            let base = build_base_name(&time_base, &slug, kind);
            let base = match index {
                Some(i) if !i.is_empty() => format!("{}_{}", base, i),
                _ => base,
            };

            let old_audio = existing_audio_for_stem(&dir, &stem);

            let unique_base =
                choose_unique_base(&dir, &base, Some(&txt_path), old_audio.as_deref(), true);
            let new_txt_path = dir.join(format!("{}.txt", unique_base));

            if new_txt_path != txt_path {
                info!(
                    "{} transcript: {} -> {}",
                    if dry_run { "Would rename" } else { "Renaming" },
                    txt_path.display(),
                    new_txt_path.display()
                );
                if !dry_run && let Err(e) = fs::rename(&txt_path, &new_txt_path) {
                    warn!("Failed to rename transcript {}: {}", txt_path.display(), e);
                    report.errors += 1;
                    continue;
                }
                report.renamed_text += 1;
            } else {
                report.skipped += 1;
            }

            if let Some(old_audio_path) = old_audio {
                let audio_ext = old_audio_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("wav");
                let new_audio_path = dir.join(format!("{}.{}", unique_base, audio_ext));
                if new_audio_path != old_audio_path {
                    info!(
                        "{} audio: {} -> {}",
                        if dry_run { "Would rename" } else { "Renaming" },
                        old_audio_path.display(),
                        new_audio_path.display()
                    );
                    if !dry_run {
                        if let Err(e) = fs::rename(&old_audio_path, &new_audio_path) {
                            warn!("Failed to rename audio {}: {}", old_audio_path.display(), e);
                            report.errors += 1;
                        } else {
                            report.renamed_audio += 1;
                        }
                    } else {
                        report.renamed_audio += 1;
                    }
                } else {
                    report.skipped += 1;
                }
                handled_audio.insert(new_audio_path);
            }
        }

        for audio_path in audio_files {
            if handled_audio.contains(&audio_path) {
                continue;
            }

            let dir = audio_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| day_path.clone());
            let stem = match audio_path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem.to_string(),
                None => {
                    warn!("Skipping non-UTF8 audio name: {}", audio_path.display());
                    report.skipped += 1;
                    continue;
                }
            };

            let (kind, base_stem, index) = split_kind_and_index(&stem, assume_kind);
            let (time_base_opt, slug_hint) = split_time_and_slug(&base_stem);
            let time_base = time_base_opt
                .or_else(|| time_base_from_metadata(&audio_path))
                .unwrap_or_else(|| Local::now().format("%H%M%S").to_string());

            let slug = slug_hint.as_deref().map(slug_from_hint).unwrap_or_default();
            let base = build_base_name(&time_base, &slug, kind);
            let base = match index {
                Some(i) if !i.is_empty() => format!("{}_{}", base, i),
                _ => base,
            };

            let unique_base = choose_unique_base(&dir, &base, None, Some(&audio_path), false);
            let audio_ext = audio_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("wav");
            let new_audio_path = dir.join(format!("{}.{}", unique_base, audio_ext));

            if new_audio_path != audio_path {
                info!(
                    "{} orphan audio: {} -> {}",
                    if dry_run { "Would rename" } else { "Renaming" },
                    audio_path.display(),
                    new_audio_path.display()
                );
                if !dry_run {
                    if let Err(e) = fs::rename(&audio_path, &new_audio_path) {
                        warn!("Failed to rename audio {}: {}", audio_path.display(), e);
                        report.errors += 1;
                    } else {
                        report.renamed_audio += 1;
                    }
                } else {
                    report.renamed_audio += 1;
                }
            } else {
                report.skipped += 1;
            }
        }
    }

    Ok(report)
}

/// Save audio file to transcriptions folder with the given timestamp and optional slug.
///
/// Creates an optimized m4a archive alongside the transcript
/// (e.g., 143052_czesc-jak_raw.m4a pairs with 143052_czesc-jak_raw.txt).
///
/// # Arguments
/// * `src_path` - Path to the source WAV file (typically a temp file)
/// * `timestamp` - Timestamp to use for the filename (should match the transcript)
/// * `transcript_text` - Optional transcript text to generate slug from (first 3 words)
/// * `kind` - What kind of transcript artifact this is
///
/// # Returns
/// * `Some(PathBuf)` - Path to the saved audio file on success
/// * `None` - If src_path doesn't exist or copy failed
pub fn save_audio(
    src_path: &Path,
    timestamp: DateTime<Local>,
    transcript_text: Option<&str>,
    kind: TranscriptKind,
) -> Option<PathBuf> {
    let result = (|| -> Result<PathBuf> {
        let mut source = daily_archive::admit_source(src_path)?;
        let base = build_base_name(
            &timestamp.format("%H%M%S").to_string(),
            &transcript_text
                .map(|text| make_slug(text, 3))
                .unwrap_or_default(),
            kind,
        );
        daily_archive::save(
            &crate::config::Config::config_dir(),
            &mut source,
            &timestamp,
            &base,
            None,
            crate::audio::archive::encode_wav_to_m4a,
        )
    })();
    archive_result(result)
}

fn archive_result(result: Result<PathBuf>) -> Option<PathBuf> {
    match result {
        Ok(path) => Some(path),
        Err(error) => {
            warn!("daily archive failed; source preserved: {error:#}");
            None
        }
    }
}

/// Put one take — hold or toggle — into the daily transcriptions bag.
///
/// Committed speech writes a paired `*_raw.{m4a,wav}` + `*_raw.txt` under
/// `~/.codescribe/transcriptions/YYYY-MM-DD/`. No-speech writes a `*_failed`
/// audio + empty marker with the fixed history title; unavailable lane output
/// archives audio only and never persists its diagnostic as transcript text.
/// `sessions/<id>.wav` is demux identity, not a second product bag.
#[cfg(test)]
fn save_session_transcript(
    transcript: SessionTranscriptArchive<'_>,
    timestamp: DateTime<Local>,
) -> Option<HistoryEntry> {
    match transcript {
        SessionTranscriptArchive::Committed(text) if !text.trim().is_empty() => Some(
            save_entry_with_timestamp(text, Some(timestamp), TranscriptKind::Raw),
        ),
        SessionTranscriptArchive::Committed(_) | SessionTranscriptArchive::NoSpeech => {
            Some(save_entry_with_timestamp_and_slug(
                "",
                Some(timestamp),
                TranscriptKind::Failed,
                Some("no-speech"),
            ))
        }
        SessionTranscriptArchive::Unavailable(_diagnostic) => None,
    }
}

/// Admit a pathname once, then use the held-source daily archive owner.
pub fn archive_session_take(
    src_path: &Path,
    transcript: SessionTranscriptArchive<'_>,
) -> Option<PathBuf> {
    match daily_archive::admit_source(src_path) {
        Ok(mut source) => archive_session_take_from_file(&mut source, transcript),
        Err(error) => archive_result(Err(error)),
    }
}

/// Archive the already admitted WAV. Never reopen its former pathname.
pub fn archive_session_take_from_file(
    source: &mut fs::File,
    transcript: SessionTranscriptArchive<'_>,
) -> Option<PathBuf> {
    archive_session_take_from_file_with_truth(source, transcript, None).map(|take| take.audio)
}

/// Daily audio plus the observer card when that write landed beside the text.
pub struct ArchivedTake {
    pub audio: PathBuf,
    pub observer: Option<PathBuf>,
}

/// Archive the already admitted WAV and, when a take truth is supplied and the
/// take persisted transcript text, write its observer card beside the text:
/// `<base>.txt.truth.json` (`docs/truth-contract.md`).
///
/// The sidecar is written for the RETURNED archive path, so a collision-rename
/// (`_raw_1`) still pairs correctly. It is an OBSERVER projection — no
/// delivery path reads it back. An `Unavailable` take persists no text and
/// grows no sidecar. A sidecar write failure is a warning, never an archive
/// failure: the paired audio + text are already published at that point.
pub fn archive_session_take_from_file_with_truth(
    source: &mut fs::File,
    transcript: SessionTranscriptArchive<'_>,
    truth: Option<&TakeTruth>,
) -> Option<ArchivedTake> {
    let now = Local::now();
    let (slug, kind, text) = archive_classification(transcript);
    let base = build_base_name(&now.format("%H%M%S").to_string(), &make_slug(slug, 3), kind);
    let archived = archive_result(daily_archive::save(
        &crate::config::Config::config_dir(),
        source,
        &now,
        &base,
        text,
        crate::audio::archive::encode_wav_to_m4a,
    ));
    let mut observer = None;
    if let (Some(audio), Some(truth), Some(_)) = (archived.as_ref(), truth, text) {
        let transcript_path = audio.with_extension("txt");
        match write_truth_sidecar(&transcript_path, truth) {
            Ok(path) => observer = Some(path),
            Err(error) => {
                warn!(
                    "truth sidecar write failed for {}: {error:#}",
                    transcript_path.display()
                );
            }
        }
    }
    archived.map(|audio| ArchivedTake { audio, observer })
}

fn archive_classification(
    transcript: SessionTranscriptArchive<'_>,
) -> (&str, TranscriptKind, Option<&str>) {
    match transcript {
        SessionTranscriptArchive::Committed(text) if !text.trim().is_empty() => {
            (text, TranscriptKind::Raw, Some(text.trim()))
        }
        SessionTranscriptArchive::Committed(_) | SessionTranscriptArchive::NoSpeech => {
            ("no-speech", TranscriptKind::Failed, Some(""))
        }
        SessionTranscriptArchive::Unavailable(_) => ("", TranscriptKind::Failed, None),
    }
}

/// Daily archive filesystem owner. All mutations are relative to pinned dirs.
#[cfg(unix)]
mod daily_archive {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

    pub(super) fn open_at(dir: &File, name: &CStr, flags: libc::c_int) -> std::io::Result<File> {
        if name.to_bytes().is_empty()
            || name.to_bytes().contains(&b'/')
            || matches!(name.to_bytes(), b"." | b"..")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsafe archive component",
            ));
        }
        // SAFETY: live directory descriptor and single NUL-terminated component.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful open transfers exactly one owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub(super) fn parent(path: &Path) -> Result<(File, CString)> {
        anyhow::ensure!(
            !path.components().any(|c| matches!(c, Component::ParentDir)),
            "archive refuses parent traversal"
        );
        let leaf = CString::new(path.file_name().context("archive needs leaf")?.as_bytes())?;
        let resolved = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?;
        let mut dir = File::open("/")?;
        for component in resolved.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    dir = open_at(
                        &dir,
                        &CString::new(name.as_bytes())?,
                        libc::O_RDONLY | libc::O_DIRECTORY,
                    )?;
                }
                _ => anyhow::bail!("archive parent must be absolute"),
            }
        }
        Ok((dir, leaf))
    }

    pub(super) fn admit_source(path: &Path) -> Result<File> {
        let (dir, leaf) = parent(path)?;
        let source = open_at(&dir, &leaf, libc::O_RDONLY | libc::O_NONBLOCK)?;
        anyhow::ensure!(source.metadata()?.is_file(), "archive WAV must be regular");
        Ok(source)
    }

    pub(super) fn subdir(dir: &File, name: &CStr) -> Result<File> {
        // SAFETY: names come from fixed components or validated date/root leaves.
        if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        Ok(open_at(dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?)
    }

    fn day(root: &Path, now: &DateTime<Local>) -> Result<(File, PathBuf)> {
        let (parent, leaf) = parent(root)?;
        let root_dir = subdir(&parent, &leaf)?;
        let archive = subdir(&root_dir, c"transcriptions")?;
        let date = now.format("%Y-%m-%d").to_string();
        let dir = subdir(&archive, &CString::new(date.as_str())?)?;
        Ok((dir, root.join("transcriptions").join(date)))
    }

    /// Own only an exclusively created entry. Published links have other names
    /// and are never removed by this guard, even after partial pair failure.
    struct Entry<'a> {
        dir: &'a File,
        name: CString,
        file: File,
    }

    impl Entry<'_> {
        /// The stage was renamed onto its final leaf: its name now belongs to
        /// a published file and must not be unlinked.
        fn disarm(&mut self) {
            self.name = CString::default();
        }
    }

    impl Drop for Entry<'_> {
        fn drop(&mut self) {
            if self.name.as_bytes().is_empty() {
                return;
            }
            // SAFETY: remove only this guard's single staging/reservation entry.
            if unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0) } < 0 {
                warn!(
                    "archive owned-entry cleanup failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    fn create<'a>(dir: &'a File, name: CString) -> std::io::Result<Entry<'a>> {
        let file = open_at(dir, &name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)?;
        Ok(Entry { dir, name, file })
    }

    fn occupied(dir: &File, name: &CStr) -> Result<bool> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstatat initializes stat on success; no fields are read.
        let rc = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error.into())
        }
    }

    /// One reservation coordinates all writers, including text-only history.
    /// Existing dangling links, hardlinks and any other leaves occupy the stem.
    fn reserve<'a>(dir: &'a File, base: &str) -> Result<(Entry<'a>, String)> {
        for index in 0..=10_000 {
            let stem = if index == 0 {
                base.to_string()
            } else {
                format!("{base}_{index}")
            };
            let lock = match create(dir, CString::new(format!(".{stem}.reserve"))?) {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            let mut conflict = false;
            for extension in ["txt", "wav", "m4a"] {
                conflict |= occupied(dir, &CString::new(format!("{stem}.{extension}"))?)?;
            }
            if !conflict {
                return Ok((lock, stem));
            }
        }
        anyhow::bail!("daily archive stem allocation exhausted")
    }

    fn stage(dir: &File) -> Result<Entry<'_>> {
        for _ in 0..10_000 {
            let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!(".archive-{}-{sequence}.tmp", std::process::id()))?;
            match create(dir, name) {
                Ok(entry) => return Ok(entry),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("daily archive staging allocation exhausted")
    }

    fn publish(entry: &Entry<'_>, name: &str) -> Result<()> {
        entry.file.sync_all()?;
        let name = CString::new(name)?;
        // SAFETY: atomic no-replace publication in the same pinned directory.
        // A newly planted final leaf causes EEXIST, never an overwrite.
        if unsafe {
            libc::linkat(
                entry.dir.as_raw_fd(),
                entry.name.as_ptr(),
                entry.dir.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    /// `flock` on a fresh description of a pinned directory: the same lease
    /// shape as the truth sidecar writer, held by every process that reads or
    /// revises an archived transcript's revision chain in that directory.
    struct DirLease(File);

    impl DirLease {
        fn acquire(dir: &File, operation: libc::c_int) -> Result<Self> {
            // SAFETY: `.` names the pinned directory itself; a successful
            // openat transfers exactly one owned descriptor.
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    c".".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                    0,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("archive revision lease refused");
            }
            // SAFETY: `fd` came from a successful openat and has one owner.
            let lease = unsafe { File::from_raw_fd(fd) };
            // SAFETY: the descriptor stays open for the life of this lease.
            if unsafe { libc::flock(lease.as_raw_fd(), operation) } < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("archive revision lease refused");
            }
            Ok(Self(lease))
        }
    }

    impl Drop for DirLease {
        fn drop(&mut self) {
            // SAFETY: release only the lock this lease acquired.
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }

    fn identity(file: &File) -> Result<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    /// Admission owns the root, day and raw source descriptors before waiting
    /// for the day lease. Never resolve their paths again through symlinks.
    pub(super) struct ArchivePin {
        pub(super) path: PathBuf,
        root_path: PathBuf,
        root: File,
        relative_parent: PathBuf,
        dir: File,
        leaf: CString,
        source: File,
        source_stamp: LeafStamp,
    }

    fn open_directory_below(dir: &File, relative: &Path) -> Result<File> {
        let mut current = dir.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                anyhow::bail!("archive directory needs normal relative components");
            };
            current = open_at(
                &current,
                &CString::new(name.as_bytes())?,
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
        }
        Ok(current)
    }

    fn open_directory(path: &Path) -> Result<File> {
        let relative = path
            .strip_prefix(Path::new("/"))
            .context("archive directory must be absolute")?;
        open_directory_below(&File::open("/")?, relative)
    }

    pub(super) fn admit_revision_source(transcript: &Path, root: &Path) -> Result<ArchivePin> {
        let relative = transcript
            .strip_prefix(root)
            .context("archived transcript is outside its admitted root")?;
        let relative_parent = relative
            .parent()
            .context("archive needs parent")?
            .to_path_buf();
        let leaf = CString::new(
            relative
                .file_name()
                .context("archive needs leaf")?
                .as_bytes(),
        )?;
        let root_dir = open_directory(root)?;
        let dir = open_directory_below(&root_dir, &relative_parent)?;
        let source = open_at(&dir, &leaf, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let source_stamp = LeafStamp::read(&source)?;
        let admitted = ArchivePin {
            path: transcript.to_path_buf(),
            root_path: root.to_path_buf(),
            root: root_dir,
            relative_parent,
            dir,
            leaf,
            source,
            source_stamp,
        };
        admitted.ensure_admitted()?;
        Ok(admitted)
    }

    impl ArchivePin {
        fn ensure_admitted(&self) -> Result<()> {
            let current_root = open_directory(&self.root_path)?;
            let current_dir = open_directory_below(&current_root, &self.relative_parent)?;
            anyhow::ensure!(
                identity(&self.root)? == identity(&current_root)?
                    && identity(&self.dir)? == identity(&current_dir)?,
                "archive directory changed after admission: {}",
                self.path.display()
            );
            let current_source = open_at(&self.dir, &self.leaf, libc::O_RDONLY | libc::O_NONBLOCK)?;
            anyhow::ensure!(
                self.source_stamp == LeafStamp::read(&self.source)?
                    && self.source_stamp == LeafStamp::read(&current_source)?,
                "raw archive source changed after admission: {}",
                self.path.display()
            );
            Ok(())
        }
    }

    fn pin_leased(transcript: &ArchivePin, operation: libc::c_int) -> Result<DirLease> {
        let lease = DirLease::acquire(&transcript.dir, operation)?;
        transcript.ensure_admitted()?;
        Ok(lease)
    }

    fn chain_leaf(leaf: &CStr) -> Result<CString> {
        let mut name = leaf.to_bytes().to_vec();
        name.extend_from_slice(ARCHIVE_REVISION_SUFFIX.as_bytes());
        Ok(CString::new(name)?)
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct LeafStamp {
        dev: u64,
        ino: u64,
        len: u64,
        modified: (i64, i64),
        changed: (i64, i64),
    }

    impl LeafStamp {
        fn read(file: &File) -> Result<Self> {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            anyhow::ensure!(metadata.is_file(), "archive leaf is not a regular file");
            Ok(Self {
                dev: metadata.dev(),
                ino: metadata.ino(),
                len: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }
    }

    /// Keep the descriptor open so replacement cannot recycle its identity;
    /// absence is a receipt too, distinct from an existing empty file.
    struct LeafSnapshot {
        bytes: Vec<u8>,
        admitted: Option<(File, LeafStamp)>,
    }

    impl LeafSnapshot {
        fn ensure_unchanged(&self, dir: &File, name: &CStr) -> Result<()> {
            match (
                self.admitted.as_ref(),
                open_at(dir, name, libc::O_RDONLY | libc::O_NONBLOCK),
            ) {
                (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                (Some((file, expected)), Ok(current)) => {
                    anyhow::ensure!(
                        *expected == LeafStamp::read(file)?
                            && *expected == LeafStamp::read(&current)?,
                        "archive leaf changed after read: {}",
                        name.to_string_lossy()
                    );
                    Ok(())
                }
                (_, Err(error)) => Err(error.into()),
                (None, Ok(_)) => anyhow::bail!(
                    "archive leaf appeared after read: {}",
                    name.to_string_lossy()
                ),
            }
        }
    }

    /// Read one regular leaf through the pinned directory and retain its
    /// identity. A symlink or special file is refused; absence is explicit.
    fn read_leaf(dir: &File, name: &CStr, optional: bool) -> Result<LeafSnapshot> {
        use std::io::Read;
        let mut file = match open_at(dir, name, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => file,
            Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LeafSnapshot {
                    bytes: Vec::new(),
                    admitted: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let stamp = LeafStamp::read(&file)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let snapshot = LeafSnapshot {
            bytes,
            admitted: Some((file, stamp)),
        };
        snapshot.ensure_unchanged(dir, name)?;
        Ok(snapshot)
    }

    /// The raw transcript and its revision chain, read as one coherent pair
    /// under the shared lease.
    pub(super) fn read_revision_chain(transcript: &ArchivePin) -> Result<(Vec<u8>, Vec<u8>)> {
        let _lease = pin_leased(transcript, libc::LOCK_SH)?;
        let chain = chain_leaf(&transcript.leaf)?;
        let raw = read_leaf(&transcript.dir, &transcript.leaf, false)?;
        let revisions = read_leaf(&transcript.dir, &chain, true)?;
        raw.ensure_unchanged(&transcript.dir, &transcript.leaf)?;
        revisions.ensure_unchanged(&transcript.dir, &chain)?;
        transcript.ensure_admitted()?;
        Ok((raw.bytes, revisions.bytes))
    }

    /// Keep bytes after the last record boundary beside the chain before a
    /// rewrite drops them. They were never acknowledged, but they are the
    /// only evidence of an interrupted writer.
    fn quarantine_tail(dir: &File, chain: &CStr, tail: &[u8]) -> Result<()> {
        let leaf = std::str::from_utf8(chain.to_bytes()).unwrap_or("archive.revisions.jsonl");
        let name = format!("{leaf}.torn-{}", uuid::Uuid::new_v4());
        let mut entry = stage(dir)?;
        entry.file.write_all(tail)?;
        publish(&entry, &name)?;
        warn!(
            "archive revision chain had {} unacknowledged bytes; kept as {name}",
            tail.len()
        );
        Ok(())
    }

    /// Revise one chain under the exclusive lease. `plan` sees the raw
    /// transcript and current chain bytes while the lease is held. Admission
    /// and leaf receipts are checked again immediately before publication.
    /// Creating the first chain is atomic no-replace. Replacing an existing
    /// chain is atomic rename, but not inode-conditional: a process ignoring
    /// the lease can still race the final check and rename. Do not claim an
    /// adversarial filesystem CAS that the platform does not provide.
    pub(super) fn update_revision_chain<T>(
        transcript: &ArchivePin,
        plan: impl FnOnce(&[u8], &[u8]) -> Result<ChainWrite<T>>,
    ) -> Result<T> {
        let _lease = pin_leased(transcript, libc::LOCK_EX)?;
        let dir = &transcript.dir;
        let chain = chain_leaf(&transcript.leaf)?;
        let raw = read_leaf(dir, &transcript.leaf, false)?;
        let current = read_leaf(dir, &chain, true)?;
        let write = plan(&raw.bytes, &current.bytes)?;
        transcript.ensure_admitted()?;
        raw.ensure_unchanged(dir, &transcript.leaf)?;
        current.ensure_unchanged(dir, &chain)?;
        if !write.uncommitted_tail.is_empty() {
            quarantine_tail(dir, &chain, &write.uncommitted_tail)?;
        }
        let mut entry = stage(dir)?;
        entry.file.write_all(&write.contents)?;
        entry.file.sync_all()?;
        transcript.ensure_admitted()?;
        raw.ensure_unchanged(dir, &transcript.leaf)?;
        current.ensure_unchanged(dir, &chain)?;
        if current.admitted.is_none() {
            // SAFETY: no-replace publication of the synced stage in the
            // pinned directory refuses an entry planted after our check.
            if unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    entry.name.as_ptr(),
                    dir.as_raw_fd(),
                    chain.as_ptr(),
                    0,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error())
                    .context("archive revision first publication refused");
            }
        } else {
            // SAFETY: both names live in the leased directory. This never
            // follows a symlink, but the final identity check and rename are
            // separate syscalls; noncooperating writers are not serialized.
            if unsafe {
                libc::renameat(
                    dir.as_raw_fd(),
                    entry.name.as_ptr(),
                    dir.as_raw_fd(),
                    chain.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error())
                    .context("archive revision publication refused");
            }
            entry.disarm();
        }
        // SAFETY: fsync of the leased directory makes publication durable.
        if unsafe { libc::fsync(dir.as_raw_fd()) } < 0 {
            return Err(std::io::Error::last_os_error())
                .context("archive revision not confirmed durable");
        }
        transcript.ensure_admitted()?;
        let published = open_at(dir, &chain, libc::O_RDONLY | libc::O_NONBLOCK)?;
        anyhow::ensure!(
            identity(&entry.file)? == identity(&published)?,
            "archive revision replaced before acknowledgement"
        );
        Ok(write.value)
    }

    pub(super) fn save_text(
        root: &Path,
        now: &DateTime<Local>,
        base: &str,
        text: &str,
    ) -> Result<PathBuf> {
        let (dir, path) = day(root, now)?;
        let (_reservation, stem) = reserve(&dir, base)?;
        let mut entry = stage(&dir)?;
        entry.file.write_all(text.as_bytes())?;
        let name = format!("{stem}.txt");
        publish(&entry, &name)?;
        Ok(path.join(name))
    }

    pub(super) fn save(
        root: &Path,
        source: &mut File,
        now: &DateTime<Local>,
        base: &str,
        text: Option<&str>,
        encode: impl FnOnce(&mut File, &mut File) -> Result<()>,
    ) -> Result<PathBuf> {
        anyhow::ensure!(
            source.metadata()?.is_file(),
            "archive source must be regular"
        );
        let (dir, path) = day(root, now)?;
        let (_reservation, stem) = reserve(&dir, base)?;
        // The converter can neither mutate the admitted WAV nor published data.
        let mut input = tempfile::tempfile()?;
        source.seek(SeekFrom::Start(0))?;
        std::io::copy(source, &mut input)?;
        input.seek(SeekFrom::Start(0))?;
        let mut encoded = tempfile::tempfile()?;
        let result = encode(&mut input, &mut encoded).and_then(|()| {
            anyhow::ensure!(
                encoded.metadata()?.len() > 0,
                "archive encoder returned empty success"
            );
            Ok(())
        });
        let mut audio = stage(&dir)?;
        let extension = match result {
            Ok(()) => {
                encoded.seek(SeekFrom::Start(0))?;
                std::io::copy(&mut encoded, &mut audio.file)?;
                "m4a"
            }
            Err(error) => {
                warn!("daily archive encoder refused; retaining admitted WAV: {error:#}");
                source.seek(SeekFrom::Start(0))?;
                std::io::copy(source, &mut audio.file)?;
                "wav"
            }
        };
        let audio_name = format!("{stem}.{extension}");
        publish(&audio, &audio_name)?;
        // Audio is retained if text persistence fails; never roll it back.
        if let Some(text) = text {
            let mut transcript = stage(&dir)?;
            transcript.file.write_all(text.as_bytes())?;
            publish(&transcript, &format!("{stem}.txt")).with_context(|| {
                format!(
                    "audio retained at {}; paired transcript failed",
                    path.join(&audio_name).display()
                )
            })?;
        }
        info!(
            "daily archive retained {}",
            path.join(&audio_name).display()
        );
        Ok(path.join(audio_name))
    }
}

/// Unsupported platforms refuse all daily writes rather than relax no-follow.
#[cfg(not(unix))]
mod daily_archive {
    use super::*;
    pub(super) struct ArchivePin {
        pub(super) path: PathBuf,
    }
    pub(super) fn admit_revision_source(_transcript: &Path, _root: &Path) -> Result<ArchivePin> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn admit_source(_path: &Path) -> Result<fs::File> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn read_revision_chain(_transcript: &ArchivePin) -> Result<(Vec<u8>, Vec<u8>)> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn update_revision_chain<T>(
        _transcript: &ArchivePin,
        _plan: impl FnOnce(&[u8], &[u8]) -> Result<ChainWrite<T>>,
    ) -> Result<T> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn save_text(
        _root: &Path,
        _now: &DateTime<Local>,
        _base: &str,
        _text: &str,
    ) -> Result<PathBuf> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn save(
        _root: &Path,
        _source: &mut fs::File,
        _now: &DateTime<Local>,
        _base: &str,
        _text: Option<&str>,
        _encode: impl FnOnce(&mut fs::File, &mut fs::File) -> Result<()>,
    ) -> Result<PathBuf> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
}

/// Clear all history entries
pub fn clear_history() {
    let dir = history_dir();
    if let Ok(day_dirs) = fs::read_dir(&dir) {
        for day_entry in day_dirs.flatten() {
            if day_entry.path().is_dir()
                && let Ok(txt_files) = fs::read_dir(day_entry.path())
            {
                for txt_entry in txt_files.flatten() {
                    let path = txt_entry.path();
                    if path.extension().is_some_and(|ext| ext == "txt")
                        && let Err(e) = fs::remove_file(&path)
                    {
                        warn!("Failed to delete history entry '{}': {}", path.display(), e);
                    }
                }
            }
        }
    }
}

/// History path, save/retrieve, family collapse, and audio-archive regressions.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn archive_fixture(
        root: &Path,
        source: &mut fs::File,
        now: &DateTime<Local>,
        transcript: SessionTranscriptArchive<'_>,
        encode: impl FnOnce(&mut fs::File, &mut fs::File) -> Result<()>,
    ) -> Result<PathBuf> {
        let (slug, kind, text) = archive_classification(transcript);
        let base = build_base_name(&now.format("%H%M%S").to_string(), &make_slug(slug, 3), kind);
        daily_archive::save(root, source, now, &base, text, encode)
    }

    #[cfg(unix)]
    fn refuse_encoder(_input: &mut fs::File, _output: &mut fs::File) -> Result<()> {
        anyhow::bail!("injected converter failure")
    }

    /// A committed take archived with a `TakeTruth` leaves `<base>.txt` and a
    /// parseable `<base>.txt.truth.json` beside it; an `Unavailable` take
    /// persists no text and grows no sidecar.
    #[test]
    #[serial]
    #[cfg(unix)]
    fn archived_committed_take_with_truth_writes_parseable_sidecar() {
        use crate::pipeline::contracts::{
            RawTranscript, TranscriptionEngineMode, TranscriptionEngineVerdict,
            TranscriptionSource, TranscriptionVerdict, VadVerdict,
        };
        use crate::pipeline::take_truth::{read_truth_sidecar, truth_sidecar_path};

        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"take pcm").expect("source");

        let verdict = TranscriptionVerdict::from_parts(
            "zdanie".to_string(),
            RawTranscript {
                text: "zdanie".to_string(),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 61.0,
                speech_windows: 10,
                total_windows: 25,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "░███".to_string(),
                fine_sparkline: "▁▃█▃▁".to_string(),
                fine_hop_ms: 32,
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        let truth = TakeTruth::from_verdict(&verdict, "Final-pass local • Transcript", 0);

        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let audio = archive_session_take_from_file_with_truth(
            &mut source,
            SessionTranscriptArchive::Committed("zdanie"),
            Some(&truth),
        )
        .expect("archive");
        let transcript_path = audio.audio.with_extension("txt");
        assert_eq!(
            fs::read_to_string(&transcript_path).expect("txt persisted"),
            "zdanie"
        );
        let restored = read_truth_sidecar(&transcript_path).expect("sidecar parses");
        assert_eq!(restored, truth);

        let mut unavailable_source = daily_archive::admit_source(&source_path).expect("admit");
        let unavailable_audio = archive_session_take_from_file_with_truth(
            &mut unavailable_source,
            SessionTranscriptArchive::Unavailable("lane down"),
            Some(&truth),
        )
        .expect("archive unavailable");
        let unavailable_txt = unavailable_audio.audio.with_extension("txt");
        assert!(
            !unavailable_txt.exists(),
            "Unavailable persists no transcript text"
        );
        assert!(
            !truth_sidecar_path(&unavailable_txt).exists(),
            "Unavailable grows no truth sidecar"
        );
    }

    #[test]
    #[cfg(unix)]
    fn archive_rejects_root_archive_and_day_symlinks_without_outside_writes() {
        use std::os::unix::fs::symlink;
        for component in ["root", "archive", "day"] {
            let tmp = TempDir::new().expect("tempdir");
            let root = tmp.path().join("root");
            let outside = tmp.path().join("outside");
            fs::create_dir(&outside).expect("outside");
            fs::write(outside.join("sentinel"), b"untouched").expect("sentinel");
            let now = Local::now();
            let target = match component {
                "root" => root.clone(),
                "archive" => {
                    fs::create_dir(&root).expect("root");
                    root.join("transcriptions")
                }
                _ => {
                    fs::create_dir_all(root.join("transcriptions")).expect("archive");
                    root.join("transcriptions")
                        .join(now.format("%Y-%m-%d").to_string())
                }
            };
            symlink(&outside, target).expect("link");
            let source_path = tmp.path().join("source.wav");
            fs::write(&source_path, b"source survives").expect("source");
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            assert!(
                archive_fixture(
                    &root,
                    &mut source,
                    &now,
                    SessionTranscriptArchive::NoSpeech,
                    refuse_encoder
                )
                .is_err()
            );
            assert_eq!(fs::read(source_path).expect("source"), b"source survives");
            assert_eq!(fs::read_dir(&outside).expect("outside").count(), 1);
            assert_eq!(
                fs::read(outside.join("sentinel")).expect("sentinel"),
                b"untouched"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn dangling_and_hardlinked_output_leaves_occupy_stem_without_overwrite() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        fs::create_dir_all(&day).expect("day");
        let base = build_base_name(
            &now.format("%H%M%S").to_string(),
            "words",
            TranscriptKind::Raw,
        );
        let absent = tmp.path().join("absent");
        symlink(&absent, day.join(format!("{base}.m4a"))).expect("dangling");
        let outside = tmp.path().join("outside");
        fs::write(&outside, b"outside inode").expect("outside");
        fs::hard_link(&outside, day.join(format!("{base}_1.wav"))).expect("hardlink");
        symlink(&outside, day.join(format!("{base}_2.txt"))).expect("text link");
        let mut source = daily_archive::admit_source(&outside).expect("admit");
        let audio = archive_fixture(
            tmp.path(),
            &mut source,
            &now,
            SessionTranscriptArchive::Committed("words"),
            refuse_encoder,
        )
        .expect("archive");
        assert_eq!(
            audio.file_stem().and_then(|s| s.to_str()),
            Some(format!("{base}_3").as_str())
        );
        assert_eq!(fs::read(&outside).expect("outside"), b"outside inode");
        assert!(!absent.exists());
        assert_eq!(
            fs::read(audio.with_extension("txt")).expect("text"),
            b"words"
        );
    }

    #[test]
    #[cfg(unix)]
    fn renamed_day_and_source_replacement_keep_admitted_objects() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"admitted voice").expect("source");
        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        let moved = tmp.path().join("moved-day");
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let audio = archive_fixture(
            tmp.path(),
            &mut source,
            &now,
            SessionTranscriptArchive::Committed("voice"),
            |input, _| {
                use std::io::{Read, Seek};
                fs::rename(&source_path, tmp.path().join("original.wav"))?;
                fs::write(&source_path, b"substituted")?;
                fs::rename(&day, &moved)?;
                symlink(&outside, &day)?;
                let mut bytes = Vec::new();
                input.rewind()?;
                input.read_to_end(&mut bytes)?;
                assert_eq!(bytes, b"admitted voice");
                anyhow::bail!("force WAV fallback")
            },
        )
        .expect("fallback");
        assert_eq!(
            fs::read(moved.join(audio.file_name().expect("name"))).expect("audio"),
            b"admitted voice"
        );
        assert_eq!(fs::read_dir(&outside).expect("outside").count(), 0);
        assert_eq!(fs::read_dir(&moved).expect("moved").count(), 2);
        assert_eq!(fs::read(source_path).expect("replacement"), b"substituted");
    }

    #[test]
    #[cfg(unix)]
    fn late_output_collision_refuses_publication_and_preserves_unowned_entry() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"source").expect("source");
        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        let base = build_base_name(
            &now.format("%H%M%S").to_string(),
            "",
            TranscriptKind::Failed,
        );
        let absent = tmp.path().join("absent");
        assert!(
            archive_fixture(
                tmp.path(),
                &mut source,
                &now,
                SessionTranscriptArchive::Unavailable("diagnostic"),
                |_, _| {
                    symlink(&absent, day.join(format!("{base}.wav")))?;
                    anyhow::bail!("fallback")
                }
            )
            .is_err()
        );
        assert!(!absent.exists());
        assert_eq!(fs::read_dir(day).expect("day").count(), 1);
        assert_eq!(fs::read(source_path).expect("source"), b"source");
    }

    #[test]
    #[cfg(unix)]
    #[serial]
    fn concurrent_same_second_archives_keep_pairs_and_history_rows() {
        use std::sync::{Arc, Barrier};
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"same admitted voice").expect("source");
        let now = Local::now();
        let barrier = Arc::new(Barrier::new(2));
        let paths = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    let root = tmp.path();
                    let source_path = &source_path;
                    let now = &now;
                    scope.spawn(move || {
                        let mut source = daily_archive::admit_source(source_path).expect("admit");
                        archive_fixture(
                            root,
                            &mut source,
                            now,
                            SessionTranscriptArchive::Committed("same words"),
                            |_, output| {
                                barrier.wait();
                                output.write_all(b"fake encoded audio")?;
                                Ok(())
                            },
                        )
                        .expect("archive")
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("worker"))
                .collect::<Vec<_>>()
        });
        assert_ne!(paths[0], paths[1]);
        for audio in paths {
            assert_eq!(fs::read(&audio).expect("audio"), b"fake encoded audio");
            assert_eq!(
                fs::read(audio.with_extension("txt")).expect("text"),
                b"same words"
            );
        }
        assert_eq!(recent_entries(10).len(), 2);
    }

    #[test]
    #[cfg(unix)]
    fn child_failure_deadline_and_empty_exit_publish_wav_and_clean_staging() {
        use crate::audio::archive::TestEncoderOutcome;
        for outcome in [
            TestEncoderOutcome::Failed,
            TestEncoderOutcome::Empty,
            TestEncoderOutcome::Hanging,
            TestEncoderOutcome::Symlink,
            TestEncoderOutcome::Directory,
            TestEncoderOutcome::Fifo,
            TestEncoderOutcome::Hardlink,
        ] {
            let tmp = TempDir::new().expect("tempdir");
            let source_path = tmp.path().join("source.wav");
            fs::write(&source_path, b"original WAV").expect("source");
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            let audio = archive_fixture(
                tmp.path(),
                &mut source,
                &Local::now(),
                SessionTranscriptArchive::Committed("words"),
                |input, output| crate::audio::archive::encode_test_child(input, output, outcome),
            )
            .expect("fallback");
            assert_eq!(audio.extension().and_then(|s| s.to_str()), Some("wav"));
            assert_eq!(fs::read(&audio).expect("audio"), b"original WAV");
            assert_eq!(fs::read(source_path).expect("source"), b"original WAV");
            assert_eq!(
                fs::read(audio.with_extension("txt")).expect("text"),
                b"words"
            );
            assert_eq!(
                fs::read_dir(audio.parent().expect("day"))
                    .expect("day")
                    .count(),
                2
            );
        }
    }

    #[test]
    #[cfg(unix)]
    #[serial]
    fn real_archive_classifies_outcomes_and_empty_success_falls_back() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"recover this voice").expect("source");
        for transcript in [
            SessionTranscriptArchive::Committed("spoken words"),
            SessionTranscriptArchive::NoSpeech,
            SessionTranscriptArchive::Unavailable("SECRET diagnostic"),
        ] {
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            let audio = archive_fixture(
                tmp.path(),
                &mut source,
                &Local::now(),
                transcript,
                |_, _| Ok(()),
            )
            .expect("empty-success WAV fallback");
            assert_eq!(audio.extension().and_then(|s| s.to_str()), Some("wav"));
            assert_eq!(fs::read(&audio).expect("audio"), b"recover this voice");
            let text = audio.with_extension("txt");
            match transcript {
                SessionTranscriptArchive::Committed(words) => {
                    assert_eq!(fs::read_to_string(text).expect("text"), words)
                }
                SessionTranscriptArchive::NoSpeech => {
                    assert_eq!(fs::read(text).expect("marker"), b"")
                }
                SessionTranscriptArchive::Unavailable(_) => assert!(!text.exists()),
            }
        }
        let entries = recent_entries(10);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|e| e.kind == TranscriptKind::Failed && e.preview == NO_SPEECH_HISTORY_TITLE)
        );
        assert_eq!(
            latest_copyable_entry().expect("copyable").preview,
            "spoken words"
        );
        assert_eq!(
            fs::read(source_path).expect("source"),
            b"recover this voice"
        );
    }

    /// Write a short PCM16 sine WAV fixture for m4a archive size/decode checks.
    fn write_pcm16_sine_wav(path: &Path, sample_rate: u32, seconds: u32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav fixture");
        let total_samples = sample_rate as usize * seconds as usize;
        for i in 0..total_samples {
            let t = i as f32 / sample_rate as f32;
            let tone = (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.35 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let sample = (tone * 12_000.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            writer.write_sample(sample).expect("write wav sample");
        }
        writer.finalize().expect("finalize wav fixture");
    }

    use crate::test_isolation::EnvGuard;

    /// transcriptions_dir lives under CODESCRIBE_DATA_DIR and includes the day path.
    #[test]
    #[serial]
    fn test_transcriptions_dir() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        // Canonicalize to handle macOS /var → /private/var symlink
        let tmp_canon = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());

        let dir = transcriptions_dir(&Local::now());
        assert!(dir.to_string_lossy().contains("transcriptions"));
        assert!(dir.starts_with(&tmp_canon));
    }

    /// Archived audio is the same-stem pair only: a sibling take, a formatted
    /// artifact of another stem, or a path outside the bag never resolves.
    #[test]
    #[serial]
    fn paired_audio_resolves_only_the_same_stem_take() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let day = transcriptions_dir(&Local::now());
        let take_a = day.join("101500_alpha-take_raw.txt");
        let take_b = day.join("101600_bravo-take_raw.txt");
        let formatted_a = day.join("101500_alpha-take_formatted.txt");
        fs::write(&take_a, "alpha").expect("text a");
        fs::write(day.join("101500_alpha-take_raw.m4a"), b"a").expect("audio a");
        fs::write(&take_b, "bravo").expect("text b");
        fs::write(day.join("101600_bravo-take_raw.wav"), b"b").expect("audio b");
        fs::write(&formatted_a, "Alpha.").expect("formatted a");

        let audio_a = paired_audio_for_transcript(&take_a).expect("take a audio");
        assert!(audio_a.ends_with("101500_alpha-take_raw.m4a"));
        let audio_b = paired_audio_for_transcript(&take_b).expect("take b audio");
        assert!(audio_b.ends_with("101600_bravo-take_raw.wav"));
        assert_eq!(paired_audio_for_transcript(&formatted_a), None);

        fs::remove_file(day.join("101500_alpha-take_raw.m4a")).expect("drop audio a");
        assert_eq!(paired_audio_for_transcript(&take_a), None);
        assert_eq!(
            paired_audio_for_transcript(&day.join("101600_bravo-take_raw.wav")),
            None
        );

        let outside = TempDir::new().expect("outside");
        let foreign = outside.path().join("101600_bravo-take_raw.txt");
        fs::write(&foreign, "bravo").expect("foreign text");
        fs::write(outside.path().join("101600_bravo-take_raw.m4a"), b"x").expect("foreign");
        assert_eq!(paired_audio_for_transcript(&foreign), None);
    }

    /// save_entry writes a .txt raw artifact with preview equal to the stored text.
    #[test]
    #[serial]
    fn test_save_and_retrieve() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        // Canonicalize to handle macOS /var → /private/var symlink
        let tmp_canon = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());

        let text = "Test transcript content";
        let entry = save_entry(text);

        assert!(entry.path.exists());
        assert_eq!(entry.preview, text);
        assert_eq!(entry.kind, TranscriptKind::Raw);
        assert!(entry.path.to_string_lossy().ends_with(".txt"));
        assert!(entry.path.starts_with(&tmp_canon));

        // Clean up
        let _ = fs::remove_file(&entry.path);
    }

    /// save_entry_with_timestamp stamps the entry with the provided local time.
    #[test]
    #[serial]
    fn test_save_entry_with_timestamp() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());

        let text = "Timestamped transcript";
        let now = Local::now();
        let entry = save_entry_with_timestamp(text, Some(now), TranscriptKind::Raw);

        assert!(entry.path.exists());
        assert_eq!(
            entry.timestamp.format("%H%M%S").to_string(),
            now.format("%H%M%S").to_string()
        );

        // Clean up
        let _ = fs::remove_file(&entry.path);
    }

    /// Same-second same-slug raw saves collapse to one recent row (newest wins).
    #[test]
    #[serial]
    fn test_recent_entries_collapse_same_second_save_family() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());

        // Raw draft and its post-processed final land in the same second with
        // the same slug → the second save collides into `…_raw_1.txt`.
        let now = Local::now();
        let slug_hint = Some("event zwykly nie");
        let draft = save_entry_with_timestamp_and_slug(
            "event zwykły nie może",
            Some(now),
            TranscriptKind::Raw,
            slug_hint,
        );
        let finalized = save_entry_with_timestamp_and_slug(
            "Event zwykły nie może być agentowym.",
            Some(now),
            TranscriptKind::Raw,
            slug_hint,
        );
        assert_ne!(draft.path, finalized.path, "collision suffix expected");

        // A different kind at the same second is a separate history row.
        let interpretation = save_entry_with_timestamp_and_slug(
            "Odpowiedź asystenta.",
            Some(now),
            TranscriptKind::AssistantInterpretation,
            slug_hint,
        );

        let entries = recent_entries(10);
        let raw_rows: Vec<_> = entries
            .iter()
            .filter(|e| e.kind == TranscriptKind::Raw)
            .collect();
        assert_eq!(
            raw_rows.len(),
            1,
            "same-family raw draft + final must collapse to one row: {:?}",
            entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
        assert_eq!(
            raw_rows[0].path, finalized.path,
            "the newest (post-processed) file wins the family"
        );
        assert!(
            entries.iter().any(|e| e.path == interpretation.path),
            "different kind survives as its own row"
        );

        for path in [&draft.path, &finalized.path, &interpretation.path] {
            let _ = fs::remove_file(path);
        }
    }

    /// HistoryEntry::label includes the transcript preview for UI display.
    #[test]
    #[serial]
    fn test_entry_label() {
        let entry = HistoryEntry {
            path: PathBuf::from("/tmp/test.txt"),
            timestamp: Local::now(),
            preview: "Hello world".to_string(),
            kind: TranscriptKind::Raw,
        };

        let label = entry.label();
        assert!(label.contains("Hello world"));
    }

    /// A simulated lane failure has no transcript artifact path. The previous
    /// committed user words remain the copy target, while a legitimate empty
    /// take gets the one explicit non-speech title.
    #[test]
    #[serial]
    fn session_archive_outcome_keeps_lane_diagnostic_out_of_history_and_copy() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let now = Local::now();
        let user_words = "To są słowa Foundera";
        let lane_error = "Tool-enabled response failed (ConnectError: gateway unavailable)";

        let committed =
            save_session_transcript(SessionTranscriptArchive::Committed(user_words), now)
                .expect("committed speech must create a history row");
        assert!(
            save_session_transcript(
                SessionTranscriptArchive::Unavailable(lane_error),
                now + chrono::Duration::seconds(1),
            )
            .is_none(),
            "diagnostics must not create transcript artifacts"
        );
        let no_speech = save_session_transcript(
            SessionTranscriptArchive::NoSpeech,
            now + chrono::Duration::seconds(2),
        )
        .expect("no-speech must create an explicit history row");

        assert_eq!(committed.preview, user_words);
        assert!(committed.label().contains(user_words));
        assert_eq!(no_speech.preview, NO_SPEECH_HISTORY_TITLE);
        assert!(no_speech.label().contains(NO_SPEECH_HISTORY_TITLE));

        let copyable = latest_copyable_entry().expect("last transcript remains available");
        assert_eq!(copyable.path, committed.path);
        assert_eq!(fs::read_to_string(&copyable.path).unwrap(), user_words);
        assert!(recent_entries(8).iter().all(|entry| {
            !fs::read_to_string(&entry.path)
                .unwrap_or_default()
                .contains(lane_error)
        }));
    }

    /// Shared slug_hint aligns base filenames across raw vs formatted kinds.
    #[test]
    #[serial]
    fn test_save_entry_with_slug_hint_consistency() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());

        let now = Local::now();
        let raw = save_entry_with_timestamp_and_slug(
            "raw content",
            Some(now),
            TranscriptKind::Raw,
            Some("shared slug source"),
        );
        let ai = save_entry_with_timestamp_and_slug(
            "ai content",
            Some(now),
            TranscriptKind::FormattedTranscript,
            Some("shared slug source"),
        );

        let raw_stem = raw.path.file_stem().unwrap().to_string_lossy();
        let ai_stem = ai.path.file_stem().unwrap().to_string_lossy();
        let raw_base = raw_stem.strip_suffix("_raw").unwrap_or(&raw_stem);
        let ai_base = ai_stem.strip_suffix("_formatted").unwrap_or(&ai_stem);

        assert_eq!(raw_base, ai_base, "Slug hint should align base name");
    }

    /// recent_entries recovers TranscriptKind from the on-disk filename suffix.
    #[test]
    #[serial]
    fn test_recent_entries_parses_kind_from_filename_suffix() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());

        let now = Local::now();
        let _raw = save_entry_with_timestamp_and_slug(
            "raw content",
            Some(now),
            TranscriptKind::Raw,
            Some("shared slug source"),
        );
        let formatted = save_entry_with_timestamp_and_slug(
            "formatted content",
            Some(now),
            TranscriptKind::FormattedTranscript,
            Some("shared slug source"),
        );

        let entries = recent_entries(8);
        assert!(entries.iter().any(|entry| {
            entry.path == formatted.path && entry.kind == TranscriptKind::FormattedTranscript
        }));
    }

    /// Legacy Failed payloads are never title or copy authority.
    #[test]
    #[serial]
    fn test_latest_copyable_entry_skips_failed_artifacts() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());

        let now = Local::now();
        let raw = save_entry_with_timestamp_and_slug(
            "usable transcript",
            Some(now),
            TranscriptKind::Raw,
            Some("usable transcript"),
        );
        let failed = save_entry_with_timestamp_and_slug(
            "Tool-enabled response failed (ConnectError: gateway unavailable)",
            Some(now + chrono::Duration::seconds(1)),
            TranscriptKind::Failed,
            Some("no-speech"),
        );

        let latest = latest_entry().expect("latest entry");
        assert_eq!(latest.path, failed.path);
        assert_eq!(latest.kind, TranscriptKind::Failed);
        assert_eq!(latest.preview, NO_SPEECH_HISTORY_TITLE);
        assert!(!latest.label().contains("ConnectError"));

        let copyable = latest_copyable_entry().expect("latest copyable entry");
        assert_eq!(copyable.path, raw.path);
        assert_eq!(copyable.kind, TranscriptKind::Raw);
        assert_eq!(
            fs::read_to_string(copyable.path).unwrap(),
            "usable transcript"
        );
    }

    /// Real encoder witness using held files after both public paths are replaced.
    #[test]
    #[cfg(target_os = "macos")]
    fn production_encoder_preserves_held_source_and_destination_and_decodes_m4a() {
        use std::io::{Read, Seek};
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        write_pcm16_sine_wav(&source_path, 16_000, 5);
        let original = fs::read(&source_path).expect("original WAV");
        let mut source = daily_archive::admit_source(&source_path).expect("admit source");
        let moved_source = tmp.path().join("held.wav");
        fs::rename(&source_path, &moved_source).expect("move source");
        fs::write(&source_path, b"foreign source").expect("replace source");
        let destination_path = tmp.path().join("destination.m4a");
        let mut destination = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .expect("held destination");
        let moved_destination = tmp.path().join("held.m4a");
        fs::rename(&destination_path, &moved_destination).expect("move destination");
        fs::write(&destination_path, b"foreign destination").expect("replace destination");

        crate::audio::archive::encode_wav_to_m4a(&mut source, &mut destination)
            .expect("production afconvert conversion");
        let mut encoded = Vec::new();
        destination.read_to_end(&mut encoded).expect("held result");
        assert!(encoded.len() > 12 && encoded.len() < original.len());
        assert_eq!(&encoded[4..8], b"ftyp", "M4A container, never WAV fallback");
        let (decoded, rate) =
            crate::audio::load_audio_file(&moved_destination).expect("decode production M4A");
        assert!(rate > 0 && !decoded.is_empty());
        let seconds = decoded.len() as f32 / rate as f32;
        assert!((4.0..=6.0).contains(&seconds), "decoded duration {seconds}");
        assert!(
            decoded.iter().any(|sample| sample.abs() > 0.01),
            "non-silent PCM"
        );
        source.rewind().expect("rewind source");
        let mut preserved = Vec::new();
        source.read_to_end(&mut preserved).expect("read source");
        assert_eq!(preserved, original);
        assert_eq!(fs::read(&moved_source).expect("held source"), original);
        assert_eq!(
            fs::read(&source_path).expect("foreign source"),
            b"foreign source"
        );
        assert_eq!(
            fs::read(&destination_path).expect("foreign destination"),
            b"foreign destination"
        );
    }

    /// macOS: save_audio archives to smaller m4a that still decodes near source duration.
    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn test_save_audio_archives_m4a_smaller_and_decodable() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set("CODESCRIBE_DATA_DIR", tmp.path());
        let src_wav = tmp.path().join("source.wav");
        write_pcm16_sine_wav(&src_wav, 16_000, 5);
        let wav_size = fs::metadata(&src_wav).expect("wav metadata").len();

        let saved = save_audio(
            &src_wav,
            Local::now(),
            Some("archive verifier speech"),
            TranscriptKind::Raw,
        )
        .expect("archive audio");

        assert_eq!(saved.extension().and_then(|ext| ext.to_str()), Some("m4a"));
        assert!(saved.exists(), "archive file should exist");

        let m4a_size = fs::metadata(&saved).expect("m4a metadata").len();
        assert!(
            m4a_size < wav_size,
            "m4a archive should be smaller than source wav (m4a={m4a_size}, wav={wav_size})"
        );

        let (decoded, decoded_rate) =
            crate::audio::load_audio_file(&saved).expect("decode archived m4a");
        assert!(decoded_rate > 0, "decoded sample rate should be known");
        assert!(
            !decoded.is_empty(),
            "decoded archive should produce PCM samples"
        );

        let decoded_sec = decoded.len() as f32 / decoded_rate as f32;
        assert!(
            (4.0..=6.0).contains(&decoded_sec),
            "decoded archive duration should stay near source duration, got {decoded_sec:.2}s"
        );
    }
}

#[cfg(test)]
mod archive_revision_integration_tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn fixture(root: &Path, name: &str) -> PathBuf {
        let day = root.join("2026-10-09");
        fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("{name}.txt"));
        fs::write(&path, "Raw source: five Iwo Iwo Iwo Iwo Iwo.").unwrap();
        path
    }

    #[test]
    #[serial]
    fn archive_revisions_reopen_restore_and_refuse_stale_head_without_touching_raw() {
        let root = TempDir::new().unwrap();
        let _env = crate::test_isolation::EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            root.path().to_str().unwrap(),
        );
        let path = fixture(&transcriptions_base_dir(), "revision");
        let raw = fs::read(&path).unwrap();
        let audio = path.with_extension("wav");
        fs::write(&audio, b"immutable audio evidence").unwrap();
        let accepted = commit_archived_revision(
            &path,
            0,
            "Formatted source.",
            ArchiveRevisionProvenance::Formatter,
            Some("smart".into()),
        )
        .unwrap();
        assert_eq!(accepted.revision, 1);
        let reopened = read_archived_document(&path).unwrap();
        assert_eq!(reopened.head_text(), "Formatted source.");
        assert_eq!(
            reopened.text_at(0),
            Some(std::str::from_utf8(&raw).unwrap())
        );
        assert_eq!(reopened.undo_revision(), Some(0));
        assert!(
            commit_archived_revision(
                &path,
                0,
                "Stale text",
                ArchiveRevisionProvenance::UserEdit,
                None
            )
            .is_err()
        );
        let restored = restore_archived_revision(&path, 1, 0).unwrap();
        assert_eq!(restored.revision, 2);
        assert_eq!(restored.restored_revision, Some(0));
        assert_eq!(
            read_archived_document(&path)
                .unwrap()
                .head_text()
                .as_bytes(),
            raw
        );
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_eq!(fs::read(audio).unwrap(), b"immutable audio evidence");
    }

    #[test]
    #[serial]
    fn torn_utf8_and_unterminated_json_are_quarantined_before_next_accepted_revision() {
        let root = TempDir::new().unwrap();
        let _env = crate::test_isolation::EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            root.path().to_str().unwrap(),
        );
        for index in 0..4 {
            let path = fixture(&transcriptions_base_dir(), &format!("torn-{index}"));
            commit_archived_revision(
                &path,
                0,
                "First accepted",
                ArchiveRevisionProvenance::UserEdit,
                None,
            )
            .unwrap();
            let chain = PathBuf::from(format!("{}{}", path.display(), ARCHIVE_REVISION_SUFFIX));
            let prefix = fs::read(&chain).unwrap();
            let tail = match index {
                0 => b"{\"schema\":".to_vec(),
                1 => vec![0xe2, 0x82],
                2 => b"{\"revision\":999}".to_vec(),
                _ => {
                    let mut ghost = read_archived_document(&path)
                        .unwrap()
                        .head()
                        .unwrap()
                        .clone();
                    ghost.revision = 2;
                    ghost.source_revision = 1;
                    ghost.rendered_text = "Valid JSON, never acknowledged".into();
                    ghost.receipt_id = "unacknowledged-ghost".into();
                    serde_json::to_vec(&ghost).unwrap()
                }
            };
            let mut torn = prefix.clone();
            torn.extend_from_slice(&tail);
            fs::write(&chain, &torn).unwrap();
            assert_eq!(read_archived_document(&path).unwrap().head_revision(), 1);
            commit_archived_revision(
                &path,
                1,
                "Second accepted",
                ArchiveRevisionProvenance::Retranscribe,
                Some("quality".into()),
            )
            .unwrap();
            let reopened = read_archived_document(&path).unwrap();
            assert_eq!(reopened.head_revision(), 2);
            assert_eq!(reopened.head_text(), "Second accepted");
            assert_eq!(reopened.head().unwrap().detail.as_deref(), Some("quality"));
            assert!(fs::read(&chain).unwrap().starts_with(&prefix));
            let tail_copy = fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry.file_name().to_string_lossy().starts_with(&format!(
                        "{}{}.torn-",
                        path.file_name().unwrap().to_string_lossy(),
                        ARCHIVE_REVISION_SUFFIX
                    ))
                })
                .expect("unacknowledged bytes remain as evidence");
            assert_eq!(fs::read(tail_copy.path()).unwrap(), tail);
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "Raw source: five Iwo Iwo Iwo Iwo Iwo."
            );
        }
    }

    #[test]
    #[serial]
    fn malformed_complete_record_and_changed_raw_refuse_without_rewriting_chain() {
        let root = TempDir::new().unwrap();
        let _env = crate::test_isolation::EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            root.path().to_str().unwrap(),
        );
        let path = fixture(&transcriptions_base_dir(), "corrupt");
        commit_archived_revision(
            &path,
            0,
            "Accepted",
            ArchiveRevisionProvenance::UserEdit,
            None,
        )
        .unwrap();
        let chain = PathBuf::from(format!("{}{}", path.display(), ARCHIVE_REVISION_SUFFIX));
        let accepted = fs::read(&chain).unwrap();
        let mut corrupt = accepted.clone();
        corrupt.extend_from_slice(b"{not-json}\n");
        fs::write(&chain, &corrupt).unwrap();
        assert!(
            commit_archived_revision(
                &path,
                1,
                "Must refuse",
                ArchiveRevisionProvenance::UserEdit,
                None
            )
            .is_err()
        );
        assert_eq!(fs::read(&chain).unwrap(), corrupt);
        fs::write(&chain, &accepted).unwrap();
        fs::write(&path, "Replaced raw").unwrap();
        assert!(read_archived_document(&path).is_err());
        assert!(
            commit_archived_revision(
                &path,
                1,
                "Must refuse",
                ArchiveRevisionProvenance::UserEdit,
                None
            )
            .is_err()
        );
        assert_eq!(fs::read(&chain).unwrap(), accepted);
    }

    #[cfg(unix)]
    #[test]
    fn archive_pin_refuses_directory_redirection_after_admission() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = fixture(&root, "pin");
        let pin = daily_archive::admit_revision_source(&path, &root).unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("pin.txt"), "Redirected source").unwrap();
        let day = path.parent().unwrap();
        fs::rename(day, root.join("saved-day")).unwrap();
        symlink(outside.path(), day).unwrap();
        assert!(daily_archive::read_revision_chain(&pin).is_err());
        assert!(
            daily_archive::update_revision_chain(&pin, |_, _| Ok(ChainWrite {
                contents: b"must not land\n".to_vec(),
                uncommitted_tail: Vec::new(),
                value: ()
            }))
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("pin.txt")).unwrap(),
            "Redirected source"
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn revision_publication_refuses_leaf_replacement_and_absent_to_present() {
        use std::os::unix::fs::symlink;
        for variant in ["replace", "symlink", "appear", "in-place", "raw"] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().canonicalize().unwrap();
            let path = fixture(&root, variant);
            let chain = PathBuf::from(format!("{}{}", path.display(), ARCHIVE_REVISION_SUFFIX));
            if variant != "appear" {
                fs::write(&chain, b"previous\n").unwrap();
            }
            let pin = daily_archive::admit_revision_source(&path, &root).unwrap();
            let planted = root.join("planted");
            fs::write(&planted, b"external replacement\n").unwrap();
            let result = daily_archive::update_revision_chain(&pin, |_, _| {
                match variant {
                    "replace" => fs::rename(&planted, &chain).unwrap(),
                    "symlink" => {
                        fs::remove_file(&chain).unwrap();
                        symlink(&planted, &chain).unwrap();
                    }
                    "appear" | "in-place" => fs::write(&chain, b"external replacement\n").unwrap(),
                    "raw" => fs::write(&path, b"changed raw").unwrap(),
                    _ => unreachable!(),
                }
                Ok(ChainWrite {
                    contents: b"must not land\n".to_vec(),
                    uncommitted_tail: Vec::new(),
                    value: (),
                })
            });
            assert!(result.is_err(), "{variant} must refuse before publication");
            assert_eq!(
                fs::read(&chain).unwrap(),
                if variant == "raw" {
                    b"previous\n".as_slice()
                } else {
                    b"external replacement\n".as_slice()
                }
            );
            if variant == "symlink" {
                assert!(chain.symlink_metadata().unwrap().file_type().is_symlink());
            }
        }
    }

    #[test]
    fn cas_process_probe() {
        let Some(root) = std::env::var_os("CODESCRIBE_DATA_DIR").map(PathBuf::from) else {
            return;
        };
        let request = root.join("archive-cas-probe.json");
        let Ok(request) = fs::read(request) else {
            return;
        };
        let path: PathBuf = serde_json::from_slice(&request).unwrap();
        let pid = std::process::id();
        fs::write(root.join(format!("ready-{pid}")), b"ready").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !root.join("go").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "parent never released CAS probe"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let accepted = commit_archived_revision(
            &path,
            0,
            &format!("writer {pid}"),
            ArchiveRevisionProvenance::UserEdit,
            None,
        )
        .is_ok();
        fs::write(
            root.join(format!("result-{pid}.json")),
            serde_json::to_vec(&accepted).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[serial]
    fn directory_lease_serializes_archive_cas_across_processes() {
        let root = TempDir::new().unwrap();
        let _env = crate::test_isolation::EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            root.path().to_str().unwrap(),
        );
        let path = fixture(&transcriptions_base_dir(), "processes");
        fs::write(
            root.path().join("archive-cas-probe.json"),
            serde_json::to_vec(&path).unwrap(),
        )
        .unwrap();
        let mut children = (0..3)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "state::history::archive_revision_integration_tests::cas_process_probe",
                        "--test-threads=1",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !children
            .iter()
            .all(|child| root.path().join(format!("ready-{}", child.id())).exists())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "child process failed to reach real CAS barrier"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        fs::write(root.path().join("go"), b"go").unwrap();
        let mut accepted = 0;
        for child in &mut children {
            assert!(child.wait().unwrap().success());
            let result = fs::read(root.path().join(format!("result-{}.json", child.id()))).unwrap();
            if serde_json::from_slice::<bool>(&result).unwrap() {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 1, "exactly one process owns source revision zero");
        let reopened = read_archived_document(&path).unwrap();
        assert_eq!(reopened.head_revision(), 1);
        assert_eq!(reopened.revisions.len(), 1);
        assert_eq!(
            reopened.original_text,
            "Raw source: five Iwo Iwo Iwo Iwo Iwo."
        );
    }
}
