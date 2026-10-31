//! Wire schemas of the Transcript Bus: event envelopes, receipts projected from
//! the acoustic ledger, journal history readers, the shared append descriptor
//! registry and the resolved bus path.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use codescribe_core::pipeline::acoustic_ledger::{
    ConsultationPresentationReceipt, IncrementalShapingReceipt, ManualDocumentRevisionReceipt,
    TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::TranscriptSegment;
use serde::{Deserialize, Serialize};

use super::{ProjectedAcousticReceipt, TranscriptBusEvidenceEvent};

/// Explicit path override for the clean transcript bus.
pub const TRANSCRIPT_BUS_PATH_ENV: &str = "CODESCRIBE_TRANSCRIPT_BUS_PATH";
/// Stable filename under the configured state/data root.
pub const TRANSCRIPT_BUS_FILENAME: &str = "transcript-events.jsonl";

/// Product mode attached to every committed transcript event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    /// Plain dictation or formatting; the downstream action is paste/format.
    Dictation,
    /// Agent voice input (menu bar, overlay or composer); the downstream
    /// action is send.
    Agent,
    /// Hold-based Chat/Selection assistance; downstream action is Agent delivery.
    Assistive,
}

/// Canvas phase carried by the one reducer-owned projection contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptProjectionPhase {
    #[default]
    Listening,
    Finalizing,
    Formatted,
    /// Lifecycle settled with usable words but refused acoustic completeness.
    CoverageRefused,
    NoSpeech,
    Error,
}

impl TranscriptProjectionPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Finalizing => "finalizing",
            Self::Formatted => "formatted",
            Self::CoverageRefused => "coverage_refused",
            Self::NoSpeech => "no_speech",
            Self::Error => "error",
        }
    }
}

/// Immutable identity supplied by the controller before capture starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSession {
    pub session_id: String,
    pub mode: TranscriptMode,
    /// A delivery target captured before the overlay can steal focus.
    pub has_latched_target: bool,
    /// Whether the explicit target is the overlay canvas itself. Normal
    /// capture-time sessions set this false because that caret fact exists
    /// only at the later defer click.
    pub latched_target_is_self: bool,
    /// Agent-channel audience stamped on sealed rows. `Some("*")` is the Fn+0
    /// broadcast. `None` is every dictation row: readers that ignore the field
    /// keep the previous contract.
    pub audience: Option<String>,
    /// Hold-badge preview only. Paste and the overlay document stay off.
    pub badge_only: bool,
}

/// Grain of one published span. Word pins are engine evidence; utterance
/// grain is the honest fallback when Apple committed a window, not words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptWordGrain {
    #[default]
    Word,
    Phrase,
    Utterance,
}

pub(super) fn is_word_grain(grain: &TranscriptWordGrain) -> bool {
    matches!(grain, TranscriptWordGrain::Word)
}

/// One span on the capture PCM clock: text + samples + intensity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWordSpan {
    pub text: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_db: Option<f32>,
    #[serde(default, skip_serializing_if = "is_word_grain")]
    pub grain: TranscriptWordGrain,
}

/// Falsifiable coverage result for one transcript event. Failed receipts keep
/// the clean reducer bytes visible while refusing to pretend they are anchored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCoverageReceipt {
    pub passed: bool,
    pub code: String,
}

/// A complete per-occurrence Light+ proof. Required fields have no defaults.
/// This observer record cannot be submitted to the reducer as mutation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedPresentationReceipt {
    pub receipt_id: String,
    pub provenance: String,
    pub session_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_seal_receipt: Option<String>,
    #[serde(default)]
    pub sentence_break_before: bool,
    pub source_label: String,
    pub left_context: String,
    pub left_context_sha256: String,
    pub shaped_text: String,
}

impl From<&IncrementalShapingReceipt> for ProjectedPresentationReceipt {
    fn from(receipt: &IncrementalShapingReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id.clone(),
            provenance: receipt.provenance.clone(),
            session_id: receipt.session_id.clone(),
            source_revision: receipt.source_revision,
            revision: receipt.revision,
            capture_epoch: receipt.occurrence.capture_epoch,
            sample_start: receipt.occurrence.sample_start,
            sample_end: receipt.occurrence.sample_end,
            source_seal_receipt: receipt.source_seal_receipt.clone(),
            sentence_break_before: receipt.sentence_break_before,
            source_label: receipt.source_label.clone(),
            left_context: receipt.left_context.clone(),
            left_context_sha256: receipt.left_context_sha256.clone(),
            shaped_text: receipt.shaped_text.clone(),
        }
    }
}

/// One uncovered speech span on the canonical capture PCM clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedSealCoverageRange {
    pub sample_start: u64,
    pub sample_end: u64,
}

/// Whole-session Apple-lane/final-pass evidence. Neither rendered string is a
/// Bus mutation input; both are retained so divergence is self-diagnosing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTranscriptComparisonReceipt {
    pub apple_sha256: String,
    pub apple_char_count: u64,
    pub apple_rendered_text: String,
    pub final_pass_sha256: String,
    pub final_pass_char_count: u64,
    pub final_pass_rendered_text: String,
}

impl From<&TranscriptComparisonReceipt> for ProjectedTranscriptComparisonReceipt {
    fn from(receipt: &TranscriptComparisonReceipt) -> Self {
        Self {
            apple_sha256: receipt.apple_sha256.clone(),
            apple_char_count: receipt.apple_char_count,
            apple_rendered_text: receipt.apple_rendered_text.clone(),
            final_pass_sha256: receipt.final_pass_sha256.clone(),
            final_pass_char_count: receipt.final_pass_char_count,
            final_pass_rendered_text: receipt.final_pass_rendered_text.clone(),
        }
    }
}

/// One previously published document revision, read from the Bus journal.
/// This is display evidence, never a mutation input or a substitute reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHistoryEntry {
    pub revision: u64,
    pub rendered_text: String,
    pub provenance: String,
    pub emitted_at: String,
}

/// Durable encoding shares the document once, while live callbacks retain one
/// complete projection per occurrence. The first occurrence remains at the
/// top level for readers that select one document snapshot per revision.
#[derive(Serialize)]
pub(super) struct DurableRevision<'a> {
    #[serde(flatten)]
    pub(super) document: &'a TranscriptBusEvidenceEvent,
    pub(super) persistence_encoding: &'static str,
    pub(super) occurrence_rows: Vec<DurableOccurrence<'a>>,
}

/// Fields that differ between projections of the same reducer revision.
/// All other fields are copied from the top-level document when expanding.
#[derive(Serialize)]
pub(super) struct DurableOccurrence<'a> {
    sequence: u64,
    emitted_at: &'a str,
    occurrence_session_id: &'a str,
    capture_epoch: u64,
    sample_start: u64,
    sample_end: u64,
    document_index: u64,
    label: &'a str,
    acoustic_receipts: &'a [ProjectedAcousticReceipt],
}

impl<'a> From<&'a TranscriptBusEvidenceEvent> for DurableOccurrence<'a> {
    fn from(event: &'a TranscriptBusEvidenceEvent) -> Self {
        Self {
            sequence: event.sequence,
            emitted_at: &event.emitted_at,
            occurrence_session_id: &event.occurrence_session_id,
            capture_epoch: event.capture_epoch,
            sample_start: event.sample_start,
            sample_end: event.sample_end,
            document_index: event.document_index,
            label: &event.label,
            acoustic_receipts: &event.acoustic_receipts,
        }
    }
}

pub(crate) const HISTORY_SCHEMA: &str = "codescribe.transcript-history.v1";

#[derive(Serialize, Deserialize)]
pub(super) struct CompactHistoryRow {
    schema: String,
    session_id: String,
    revision: u64,
    rendered_text: String,
    provenance: String,
    emitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    document_revision_receipt: Option<ManualDocumentRevisionReceipt>,
}

/// A compact copy of a reducer revision whose large acoustic row may expire.
/// It retains document provenance; large per-word acoustic rows may expire.
pub(crate) fn compact_history_row(line: &str) -> Option<(String, u64, String)> {
    let event = serde_json::from_str::<TranscriptBusEvidenceEvent>(line).ok()?;
    let entry = history_entry_from_event(&event)?;
    let row = CompactHistoryRow {
        schema: HISTORY_SCHEMA.to_string(),
        session_id: event.session_id.clone(),
        revision: entry.revision,
        rendered_text: entry.rendered_text,
        provenance: entry.provenance,
        emitted_at: entry.emitted_at,
        document_revision_receipt: event.document_revision_receipt.clone(),
    };
    let encoded = serde_json::to_string(&row).ok()?;
    Some((row.session_id, row.revision, encoded))
}

pub(super) fn history_entry_from_event(
    event: &TranscriptBusEvidenceEvent,
) -> Option<DocumentHistoryEntry> {
    if event.document_index != 0
        || event.reducer_revision == 0
        || event.reducer_action == "session_ended"
        || event.rendered_text.trim().is_empty()
    {
        return None;
    }
    let receipt = event
        .document_revision_receipt
        .as_ref()
        .map(|receipt| receipt.receipt_id.as_str())
        .or_else(|| {
            (event.reducer_action == "apply_manual_edit")
                .then(|| {
                    event
                        .acoustic_receipts
                        .first()
                        .and_then(|acoustic| acoustic.manual_edit_receipt.as_deref())
                })
                .flatten()
        });
    let provenance = [
        "user-edit",
        "retranscribe",
        "formatter",
        "light-plus",
        "navigation",
    ]
    .into_iter()
    .find(|kind| receipt.is_some_and(|id| id.starts_with(&format!("{kind}-"))))
    .map(str::to_string)
    .unwrap_or_else(|| match event.reducer_action.as_str() {
        "apply_ledger_decision" => "acoustic-ledger".to_string(),
        "apply_incremental_shaping" => "light-plus".to_string(),
        "apply_consultation_presentation" => "consultation".to_string(),
        action => action.to_string(),
    });
    Some(DocumentHistoryEntry {
        revision: event.reducer_revision,
        rendered_text: event.rendered_text.clone(),
        provenance,
        emitted_at: event.emitted_at.clone(),
    })
}

/// Read the already published history for one take. A missing journal means
/// there is no persisted history; it never licenses reconstructing it in UI.
pub fn document_history(session_id: &str) -> io::Result<Vec<DocumentHistoryEntry>> {
    document_history_at(&transcript_bus_path(), session_id)
}

pub(crate) fn document_history_at(
    path: &Path,
    session_id: &str,
) -> io::Result<Vec<DocumentHistoryEntry>> {
    let mut bytes_read = 0;
    document_history_at_counted(path, session_id, &mut bytes_read)
}

/// Bytes of backward scan one `document_history_at` call will read while
/// looking for this session's `session_started` row. A session that began
/// while persistence was disabled (`TranscriptBus::open_at` refuses to
/// append after a bus whose last byte is not `\n`) never writes that row, so
/// an unbounded scan would walk the whole shared bus — tens of GB on the
/// Founder's machine — for every take. Capped at the same 64 MiB the Swift
/// `OverlayChannelDeliveryReader` tail window and the Rust
/// `agent_ack::FOLD_PASS_BUDGET` / `agent_channel::ORPHAN_SCAN_WINDOW_BYTES`
/// already use, so history degrades to "whatever is in the tail" instead of
/// reading every row ever written.
pub(super) const DOCUMENT_HISTORY_SCAN_BUDGET_BYTES: u64 = 64 << 20;

/// True if `needle` occurs anywhere in `haystack`. Used as a cheap reject
/// before `serde_json` parsing: a line whose bytes never contain this
/// session's id as a substring cannot have it as the `session_id` field
/// value, so most rows from other interleaved sessions are skipped without
/// building a JSON tree.
pub(super) fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(super) fn document_history_at_counted(
    path: &Path,
    session_id: &str,
    bytes_read: &mut u64,
) -> io::Result<Vec<DocumentHistoryEntry>> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- path is the app-owned transcript_bus_path() or an explicit test temporary Bus path, never request input.
    let mut file =
        match crate::presentation::transcript_bus_maintenance::generation::Reader::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
    // Recent takes live at the tail. Read blocks backwards and stop at this
    // session's started row; other sessions may be interleaved, so merely
    // seeing another session is not a safe stopping condition.
    let session_bytes = session_id.as_bytes();
    let mut position = file.len();
    let mut prefix = Vec::new();
    let mut rows = Vec::new();
    let mut found_start = false;
    let mut budget_exhausted = false;
    while position > 0 && !found_start {
        let width = position.min(64 * 1024) as usize;
        position -= width as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut block = vec![0; width];
        file.read_exact(&mut block)?;
        *bytes_read += width as u64;
        block.extend_from_slice(&prefix);
        let mut end = block.len();
        for index in (0..block.len()).rev() {
            if block[index] != b'\n' {
                continue;
            }
            let line = &block[index + 1..end];
            end = index;
            if !contains_subslice(line, session_bytes) {
                continue;
            }
            let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if row
                .get("session_id")
                .or_else(|| row.get("event").and_then(|event| event.get("session_id")))
                .and_then(|value| value.as_str())
                != Some(session_id)
            {
                continue;
            }
            if row.get("status").and_then(|value| value.as_str()) == Some("session_started") {
                found_start = true;
                break;
            }
            rows.push(line.to_vec());
        }
        prefix = block[..end].to_vec();
        if !found_start && *bytes_read >= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES {
            budget_exhausted = true;
            break;
        }
    }
    if !found_start && !prefix.is_empty() {
        rows.push(prefix);
    }
    if budget_exhausted {
        tracing::warn!(
            session_id = %session_id,
            bytes_read = *bytes_read,
            "document_history_at exhausted its scan budget before finding session_started; returning partial tail history"
        );
    }
    rows.reverse();
    let mut storage =
        crate::presentation::transcript_bus_maintenance::generation::Decoder::default();
    let mut decoded = Vec::new();
    for line in rows {
        let Ok(value) = serde_json::from_slice(&line) else {
            continue;
        };
        for value in storage.push(value)? {
            // History selects one shared document; validate every occurrence
            // without duplicating its document bytes in this observation.
            let mut rows =
                crate::presentation::transcript_bus_maintenance::generation::rows(value)?;
            if let Some(document) = rows.next() {
                decoded.push(serde_json::to_vec(&document).map_err(io::Error::other)?);
            }
        }
    }
    if storage.incomplete() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete document storage",
        ));
    }
    let mut revisions = std::collections::BTreeMap::new();
    for line in decoded {
        if let Ok(row) = serde_json::from_slice::<CompactHistoryRow>(&line)
            && row.schema == HISTORY_SCHEMA
            && row.session_id == session_id
            && !row.rendered_text.trim().is_empty()
        {
            revisions
                .entry((row.session_id, row.revision))
                .or_insert(DocumentHistoryEntry {
                    revision: row.revision,
                    rendered_text: row.rendered_text,
                    provenance: row.provenance,
                    emitted_at: row.emitted_at,
                });
            continue;
        }
        if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line)
            && row["schema"] == "codescribe.derived-transcript.v1"
            && row["session_id"] == session_id
            && row["source_state"] != "admitted_occurrence"
            && row["source_observation"].is_null()
            && row["status"] == "applied"
            && let (Some(revision), Some(text), Some(mode), Some(time)) = (
                row["revision"].as_u64(),
                row["rendered_text"].as_str(),
                row["requested_mode"].as_str(),
                row["emitted_at"].as_str(),
            )
        {
            revisions
                .entry((session_id.to_string(), revision))
                .or_insert(DocumentHistoryEntry {
                    revision,
                    rendered_text: text.into(),
                    provenance: format!("formatter-{mode}"),
                    emitted_at: time.into(),
                });
            continue;
        }
        let Ok(event) = serde_json::from_slice::<TranscriptBusEvidenceEvent>(&line) else {
            continue;
        };
        if event.session_id != session_id {
            continue;
        }
        if let Some(entry) = history_entry_from_event(&event) {
            revisions
                .entry((event.session_id, entry.revision))
                .or_insert(entry);
        }
    }
    let mut history = revisions.into_values().collect::<Vec<_>>();
    history.sort_by(|left, right| {
        left.emitted_at
            .cmp(&right.emitted_at)
            .then(left.revision.cmp(&right.revision))
    });
    Ok(history)
}

/// Group-level provenance, deliberately separate from per-word acoustic rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedConsultationPresentation {
    pub receipt_id: String,
    pub consultation_id: String,
    pub turn_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub members: Vec<ProjectedConsultationMember>,
    pub rendered_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedConsultationMember {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub source_label: String,
    pub seal_receipt: String,
}

impl From<&ConsultationPresentationReceipt> for ProjectedConsultationPresentation {
    fn from(receipt: &ConsultationPresentationReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id.clone(),
            consultation_id: receipt.consultation_id.clone(),
            turn_id: receipt.turn_id.clone(),
            source_revision: receipt.source_revision,
            revision: receipt.revision,
            rendered_text: receipt.rendered_text.clone(),
            members: receipt
                .members
                .iter()
                .map(|member| ProjectedConsultationMember {
                    session_id: member.occurrence.session.clone(),
                    capture_epoch: member.occurrence.capture_epoch,
                    sample_start: member.occurrence.sample_start,
                    sample_end: member.occurrence.sample_end,
                    source_label: member.source_label.clone(),
                    seal_receipt: member.seal_receipt.clone(),
                })
                .collect(),
        }
    }
}

/// Where the stop path sent this take's committed document, as a state an
/// observer branches on. Deliberately typed: the human-facing `label` is
/// presentation and must never carry control meaning, and a route that was
/// *selected* is not a destination that *accepted*.
///
/// The Bus records the controller's disposition only. `ComposerPending` in
/// particular is a standing obligation, not a success: the Agent composer is
/// the intended destination and the receiver has not acknowledged admission.
/// Rust never upgrades it — only the receiver's own typed receipt can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptDelivery {
    /// No stop-path delivery ran for this take (start failure, superseded
    /// hold, an abandoned session). Absence of an attempt, not a failure.
    #[default]
    Unattempted,
    /// The document belongs to the Agent composer draft of the thread that
    /// owned the capture. Pending until the receiver admits it.
    ComposerPending,
    /// A synthetic paste accepted the text at the OS boundary.
    SinkAccepted,
    /// The stop path finished without any sink taking the text. It stays
    /// recoverable in the overlay and the session archive.
    Retained,
    /// The assistive take ended after 30 s without voice, so the stop path
    /// chose no destination. The text stays in the take archive; nothing may
    /// send it on its own, and an explicit send still can.
    EndedWithoutVoice,
    /// The text was copied to the clipboard without posting a paste.
    CopiedToClipboard,
    /// The text is armed for a later explicit insert, not pasted yet.
    DeferredInsertArmed,
}

/// Why the controller left a Bus session. Typed on purpose: the terminal
/// line carries a reason an observer can branch on, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSessionEndReason {
    /// The take went through the serialized stop path (sealed or zero-seal).
    Completed,
    /// Committed words survived a refused terminal seal; delivery is separate.
    CoverageRefused,
    /// Capture settled with refused coverage and no committed words.
    CoverageRefusedEmpty,
    /// A selected destination failed or declined the committed text.
    DeliveryFailed,
    /// A newer hold generation (key-up / reschedule) superseded this start
    /// after `session_started` and before the take became an active recording.
    StartSuperseded,
    /// The recorder could not be started after `session_started` was written.
    StartFailed,
    /// A CLI file decoder or output sink failed after opening its session.
    TranscriptionFailed,
    /// Written only before 2026-10-24: an admitted hold on the agent-channel
    /// modifier turned out to prefix a channel digit chord. A key that records
    /// is no longer the channel modifier, so no session ends this way now; the
    /// value stays so older append-only lines still decode.
    ChordPrefix,
}

/// Append-only public event contract. `text` is always clean reducer truth;
/// unfiltered engine `raw_text` never crosses this boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanTranscriptEvent {
    pub schema: String,
    pub sequence: u64,
    pub session_id: String,
    pub mode: TranscriptMode,
    pub utterance_id: Option<u64>,
    pub emitted_at: String,
    pub status: String,
    pub sample_rate_hz: Option<u32>,
    pub capture_epoch: Option<u64>,
    pub sample_start: Option<u64>,
    pub sample_end: Option<u64>,
    pub audio_start_seconds: Option<f32>,
    pub audio_end_seconds: Option<f32>,
    pub text: String,
    #[serde(default)]
    pub phase: TranscriptProjectionPhase,
    #[serde(default)]
    pub can_paste: bool,
    #[serde(default)]
    pub can_insert: bool,
    #[serde(default)]
    pub can_copy: bool,
    #[serde(default)]
    pub can_retranscribe: bool,
    #[serde(default)]
    pub can_format: bool,
    #[serde(default)]
    pub can_send_to_agent: bool,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptWordSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<TranscriptCoverageReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_session_id: Option<String>,
    /// Present only on `session_ended`: why the controller left the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<TranscriptSessionEndReason>,
    /// Who authored this event. **Absent means the app** — a ledger-observing
    /// [`TranscriptBus`] never sets it, and no app path may. It is set only by
    /// writers that publish text they did not receive from the ledger, today
    /// just [`crate::presentation::cli_transcript_lane`] with `"cli_file_verdict"`. A reader
    /// that requires occurrence-authenticated truth filters on its absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Persistence failure reported to the bus's diagnostic sink before it is
/// logged. Text-free by construction: the committed document has no field
/// here, so no observer can ever receive it.
pub(crate) struct PersistenceDiagnostic<'a> {
    pub session_id: &'a str,
    pub file: &'a str,
    pub error: &'a io::Error,
}

/// Injectable observer for persistence failures. Production installs a no-op;
/// tests install a recording sink so assertions never depend on tracing's
/// process-global callsite state.
pub(crate) type PersistenceDiagnosticSink = Arc<dyn Fn(&PersistenceDiagnostic<'_>) + Send + Sync>;

/// All in-process Bus sessions for a path append through this one descriptor.
/// Compaction takes this exact lock and replaces the descriptor after rename.
pub(super) static BUS_FILES: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<File>>>>> = OnceLock::new();

pub(crate) fn shared_bus_file(path: &Path) -> io::Result<Arc<Mutex<File>>> {
    let registry = BUS_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(file) = registry.get(path).and_then(Weak::upgrade) {
        return Ok(file);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = open_bus_append_file(path)?;
    let shared = Arc::new(Mutex::new(file));
    registry.insert(path.to_path_buf(), Arc::downgrade(&shared));
    Ok(shared)
}

pub(crate) fn open_bus_append_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

pub(super) struct SharedBusWriter(pub(super) Arc<Mutex<File>>, pub(super) PathBuf);

/// One session publishes the same encoded row to every destination. An
/// uncertain append retires only that destination, without retrying its prefix.
pub(super) struct BusFanout {
    pub(super) session_id: String,
    pub(super) destinations: Vec<BusDestination>,
}

pub(super) struct BusDestination {
    pub(super) path: PathBuf,
    pub(super) file: Option<Box<dyn Write + Send>>,
}

impl Write for BusFanout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for destination in &mut self.destinations {
            let Some(file) = destination.file.as_mut() else {
                continue;
            };
            if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
                destination.file = None;
                tracing::warn!(%error, bus = %destination.path.display(), session_id = %self.session_id, persistence = "disabled_for_session", "transcript bus destination disabled; other destinations continue");
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Each destination was flushed as part of the complete-row write.
        Ok(())
    }
}

impl Write for SharedBusWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::presentation::transcript_bus_maintenance::generation::append(
            &self.1, &self.0, bytes,
        )?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Path precedence: explicit contract, XDG state, then Codescribe's existing
/// project/data override (`CODESCRIBE_DATA_DIR`) via `Config::config_dir()`.
pub fn transcript_bus_path() -> PathBuf {
    if let Ok(path) = std::env::var(TRANSCRIPT_BUS_PATH_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            return expand_tilde(path);
        }
    }
    if let Ok(root) = std::env::var("XDG_STATE_HOME") {
        let root = root.trim();
        if !root.is_empty() {
            return expand_tilde(root)
                .join("codescribe")
                .join(TRANSCRIPT_BUS_FILENAME);
        }
    }
    codescribe_core::config::Config::config_dir().join(TRANSCRIPT_BUS_FILENAME)
}

pub(super) fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(relative) = path.strip_prefix("~/") {
        return directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(relative))
            .unwrap_or_else(|| PathBuf::from(path));
    }
    PathBuf::from(path)
}
