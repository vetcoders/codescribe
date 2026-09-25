//! Event-driven presentation emitter.
//!
//! Converts `EngineEvent`s into user-facing output through the one canonical
//! reducer and an ordered delta-delivery worker.
//!
//! Uses an ordered mpsc channel to guarantee that target updates and finish
//! arrive in the exact order they were emitted,
//! eliminating the fire-and-forget tokio::spawn ordering race.

use std::{collections::BTreeMap, io::Write as _, sync::Arc};

use codescribe_core::agent::consultation::ConsultationGroupAnswer;
use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, ConsultationPresentationInput, ConsultationPresentationReceipt,
    DocumentRevisionProvenance, IncrementalShapingInput, IncrementalShapingReceipt,
    LedgerSealReceipt, ManualDocumentRevisionReceipt, MutationReceipt, NoAuthorityReason,
    ObservationIdentity, ObservationProducer, OccurrenceIdentity, SealCoverageReceipt,
    TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::{
    DeltaSink, EngineEvent, EventSink, PreviewPinReceipt, SpeechIntegrity, SpeechIntegrityPhase,
    TranscriptDelta,
};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::transcript_bus::{TranscriptBus, TranscriptBusEvidenceEvent};

/// Read-only cursor paint observer: bounded text and acoustic warning state.
pub type CursorObserver = Arc<dyn Fn(&CompactProjection) + Send + Sync>;

/// Passive paint from one opened capture. Sequence 1 binds the display before
/// any transcription event; later values replace paint, never document truth.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactProjection {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sequence: u64,
    pub text: String,
    pub degraded: bool,
    /// Every unanchored text of this capture, in PCM order. It is painted
    /// beside the canvas, never inside the canvas string or the Bus. The
    /// stop snapshot pastes uncovered evidence and accounts covered hypotheses
    /// against their committed occurrence.
    pub evidence: Vec<UnanchoredEvidence>,
}

/// Read-only text the ledger kept visible without mutation authority,
/// anchored to its PCM range on the capture clock. `reason` is the ledger's
/// [`NoAuthorityReason`] label. Its text is never compared with the canvas: a
/// differing alternative wholly inside a committed token is shown, not
/// suppressed as a duplicate, because it is not canvas.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnanchoredEvidence {
    pub sample_start: u64,
    pub sample_end: u64,
    pub text: String,
    pub reason: String,
}

/// Commands sent through the ordered channel to the emitter worker.
enum EmitterCmd {
    /// Paint the session-visible projection. Delivery stays the committed text.
    PublishCommittedRevision {
        paint: String,
        delivery: String,
    },
    /// Paint volatile text without touching delivery or any committed sink.
    PaintEphemeralPreview(String),
    PaintBarrier(tokio::sync::oneshot::Sender<()>),
    Finish,
}

fn append_stream_delta(path: &std::path::Path, delta: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let payload = delta
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\u{0008}', "\\b");
    writeln!(file, "[{timestamp}] {payload}")
}

/// One canonical transcript slot, keyed only by the physical occurrence that
/// earned it. Text is a ledger-authorized label; receipt values are immutable
/// provenance references and never participate in entry identity.
///
/// W2 input: one admitted ledger decision for `occurrence`. W2 output: this
/// entry inside a [`TranscriptRevision`]. Formatter proposals re-enter through
/// `EngineEvent::OccurrenceLabelProposal`; delivery consumes the decided route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptDocumentEntry {
    pub occurrence: OccurrenceIdentity,
    pub label: String,
    pub observation_receipt: String,
    pub word_evidence_receipts: Vec<String>,
    pub layer_decision_receipts: Vec<String>,
    pub seal_receipt: Option<String>,
    pub manual_edit_receipt: Option<String>,
    /// Presentation provenance, distinct from acoustic and human-edit evidence.
    pub presentation_receipt: Option<IncrementalShapingReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentContextMarker {
    position: usize,
    label: String,
    order: usize,
}

/// A ledger-authorized document transition. Every variant names a physical
/// occurrence; no action locates content by comparing transcript strings.
///
/// W2 must construct these actions from ledger receipts. W1 deliberately does
/// not expose an `apply` path, so no engine, Bus, bridge, or Swift caller can
/// mutate the document through this declaration alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReducerAction {
    ApplyLedgerDecision {
        entry: TranscriptDocumentEntry,
    },
    RecordLedgerSeal {
        occurrence: OccurrenceIdentity,
        seal_receipt: String,
        terminal: bool,
    },
    RecordSealCoverage {
        receipt: SealCoverageReceipt,
        comparison: Option<TranscriptComparisonReceipt>,
    },
    ApplyManualEdit {
        entry: TranscriptDocumentEntry,
    },
    ApplyUserRevision {
        receipt: ManualDocumentRevisionReceipt,
    },
    ApplyConsultationPresentation {
        receipt: ConsultationPresentationReceipt,
    },
    /// One closed occurrence gained its deterministic presentation shape while
    /// the session is still open. Not a document edit: the receipt names one
    /// occurrence and the ledger label behind it is unchanged.
    ApplyIncrementalShaping {
        receipt: IncrementalShapingReceipt,
    },
    RecordContextMarker {
        position: usize,
        label: String,
        order: usize,
    },
}

/// Immutable reducer output for observers and explicit delivery selection.
/// `entries` is the complete occurrence-ordered document at `revision`.
///
/// W2 output: `TranscriptBus` observation, the single formatter proposal path,
/// and `delivery_route`. Those consumers are intentionally unresolved in W1;
/// this file is the only owner of the revision and its document entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRevision {
    pub schema: String,
    pub revision: u64,
    pub action: ReducerAction,
    pub entries: Vec<TranscriptDocumentEntry>,
    pub rendered_text: String,
    pub seal_coverage: Option<SealCoverageReceipt>,
    pub comparison: Option<TranscriptComparisonReceipt>,
    pub consultation_presentations: Vec<ConsultationPresentationReceipt>,
    // In-process reducer capability, never deserialized or exported as acoustic
    // evidence. Detects edits to any public snapshot field before publication.
    publication_digest: [u8; 32],
}

impl TranscriptRevision {
    fn digest(&self) -> [u8; 32] {
        Sha256::digest(
            format!(
                "{:?}",
                (
                    &self.schema,
                    self.revision,
                    &self.action,
                    &self.entries,
                    &self.rendered_text,
                    &self.seal_coverage,
                    &self.comparison,
                    &self.consultation_presentations,
                )
            )
            .as_bytes(),
        )
        .into()
    }

    /// Validate the complete reducer-minted snapshot before any observer or
    /// delivery side effect. The Bus calls this; it never reconstructs text.
    pub(crate) fn authenticates_publication(&self, ledger: &AcousticLedger, session: &str) -> bool {
        if self.publication_digest != self.digest()
            || self.entries.is_empty()
            || self
                .entries
                .windows(2)
                .any(|pair| pair[0].occurrence >= pair[1].occurrence)
        {
            return false;
        }
        let mut grouped = std::collections::BTreeSet::new();
        for receipt in &self.consultation_presentations {
            if !ledger.consultation_presentations().contains(receipt)
                || receipt.revision > self.revision
                || !group_matches_entries(receipt, self.entries.iter())
                || receipt
                    .members
                    .iter()
                    .any(|member| !grouped.insert(&member.occurrence))
            {
                return false;
            }
        }
        let mut left_context = String::new();
        for entry in &self.entries {
            if entry.occurrence.session != session
                || ledger.serial_of(&entry.occurrence).is_none()
                || ledger.text_of(&entry.occurrence) != Some(entry.label.as_str())
                || entry
                    .seal_receipt
                    .as_ref()
                    .is_some_and(|id| !ledger.authenticates_seal_reference(&entry.occurrence, id))
            {
                return false;
            }
            let group = self.consultation_presentations.iter().find(|receipt| {
                receipt
                    .members
                    .iter()
                    .any(|member| member.occurrence == entry.occurrence)
            });
            let presentation = if let Some(group) = group {
                if entry.presentation_receipt.is_some() {
                    return false;
                }
                if group.members[0].occurrence == entry.occurrence {
                    group.rendered_text.as_str()
                } else {
                    ""
                }
            } else if let Some(receipt) = &entry.presentation_receipt {
                if receipt.occurrence != entry.occurrence
                    || receipt.session_id != session
                    || receipt.source_label != entry.label
                    || receipt.revision > self.revision
                    || receipt.source_revision.checked_add(1) != Some(receipt.revision)
                    || receipt.left_context != left_context
                    || receipt.left_context_sha256
                        != format!("{:x}", Sha256::digest(left_context.as_bytes()))
                    || !ledger.incremental_shapings().contains(receipt)
                    || receipt.source_seal_receipt.as_ref().is_some_and(|id| {
                        ledger
                            .seal_of(&entry.occurrence)
                            .is_none_or(|seal| &seal.receipt_id != id)
                    })
                {
                    return false;
                }
                receipt.shaped_text.as_str()
            } else {
                entry.label.as_str()
            };
            append_exact_fragment(&mut left_context, presentation);
        }
        match &self.action {
            ReducerAction::RecordSealCoverage { receipt, .. } => {
                ledger.latest_seal_coverage() == Some(receipt)
                    && self.seal_coverage.as_ref() == Some(receipt)
                    && receipt.session_id == session
                    && self
                        .entries
                        .iter()
                        .all(|entry| entry.occurrence.capture_epoch == receipt.capture_epoch)
            }
            ReducerAction::RecordLedgerSeal {
                occurrence,
                seal_receipt,
                terminal,
            } => ledger.seal_receipt(seal_receipt).is_some_and(|seal| {
                seal.is_occurrence_seal() != *terminal
                    && seal.sealed_occurrences.first() == Some(occurrence)
                    && seal
                        .sealed_occurrences
                        .iter()
                        .all(|sealed| self.entries.iter().any(|entry| &entry.occurrence == sealed))
            }),
            ReducerAction::ApplyIncrementalShaping { receipt } => {
                self.entries.iter().any(|entry| {
                    entry.presentation_receipt.as_ref() == Some(receipt)
                        && receipt.revision == self.revision
                })
            }
            ReducerAction::ApplyConsultationPresentation { receipt } => {
                receipt.revision == self.revision
                    && self.consultation_presentations.contains(receipt)
            }
            ReducerAction::ApplyUserRevision { receipt } => {
                ledger.manual_document_revisions().contains(receipt)
                    && receipt.session_id == session
                    && receipt.revision == self.revision
                    && receipt.source_revision.checked_add(1) == Some(self.revision)
                    && receipt.rendered_text == self.rendered_text
                    && receipt.source_occurrences
                        == self
                            .entries
                            .iter()
                            .map(|entry| entry.occurrence.clone())
                            .collect::<Vec<_>>()
            }
            _ => true,
        }
    }
}

/// A UI request to replace one exact terminal reducer revision.
///
/// The source session and revision form the compare-and-swap boundary. Text is
/// payload only: it never identifies the document or an acoustic occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRevisionIntent {
    pub session_id: String,
    pub source_revision: u64,
    pub rendered_text: String,
    pub provenance: DocumentRevisionProvenance,
}

/// One terminal document offered for exactly one paid formatter pass.
///
/// Produced only by [`TranscriptReducer::terminal_formatter_request`]. An empty
/// or whitespace-only turn produces no request at all, so "format nothing" is
/// an absent provider call rather than a refused one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFormatterRequest {
    pub session_id: String,
    /// Compare-and-swap revision the formatter result must be admitted against.
    pub source_revision: u64,
    /// Exact committed bytes handed to the provider.
    pub source_text: String,
}

/// Rust-authored acknowledgement for one committed user revision. Swift uses
/// this only as request status; visible text still arrives through projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRevisionCommit {
    pub session_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub rendered_text: String,
    pub provenance_receipt: String,
}

/// Typed refusal reasons for a stale or unauthenticated revision request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserRevisionRefusal {
    EmptyText,
    NoCommittedDocument,
    NotTerminal,
    SessionMismatch,
    StaleRevision { expected: u64, actual: u64 },
    Unchanged,
    RevisionExhausted,
    LedgerRefusal(&'static str),
    AuthorityUnavailable,
    FormatterFailed,
    FormatterUnavailable,
    FormatterNoop,
}

impl std::fmt::Display for UserRevisionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyText => formatter.write_str("user revision text is empty"),
            Self::NoCommittedDocument => formatter.write_str("no committed transcript document"),
            Self::NotTerminal => formatter.write_str("transcript is not terminal"),
            Self::SessionMismatch => formatter.write_str("user revision session does not match"),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "stale user revision: source {actual}, current {expected}"
            ),
            Self::Unchanged => formatter.write_str("user revision is unchanged"),
            Self::RevisionExhausted => formatter.write_str("transcript revision counter exhausted"),
            Self::LedgerRefusal(reason) => {
                write!(formatter, "ledger refused user revision: {reason}")
            }
            Self::AuthorityUnavailable => {
                formatter.write_str("transcript revision authority unavailable")
            }
            Self::FormatterFailed => formatter.write_str("formatter gateway failed"),
            Self::FormatterUnavailable => {
                formatter.write_str("formatter is unavailable for the current policy or text")
            }
            Self::FormatterNoop => formatter.write_str("formatter returned unchanged text"),
        }
    }
}

impl std::error::Error for UserRevisionRefusal {}

/// Why one live per-occurrence shaping was refused.
///
/// Deliberately a separate vocabulary from [`UserRevisionRefusal`]. A live
/// shaping is not a whole-document edit, so it must not borrow that corridor's
/// `NotTerminal` law — inverting a terminal-only refusal is exactly how an
/// occurrence seal starts pretending to be a lifecycle end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncrementalShapingRefusal {
    /// The occurrence carries no committed reducer entry.
    UnknownOccurrence,
    /// The occurrence belongs to another session than the committed document.
    ForeignSession,
    /// A whole-document revision already owns presentation for this document.
    ///
    /// The literal (`force_raw`) contract has no refusal here on purpose: that
    /// promise belongs to the take, so [`PresentationEmitter`] never asks the
    /// reducer to shape at all.
    DocumentRevisionOwnsPresentation,
    /// The shape for this exact label is already committed.
    AlreadyShaped,
    /// Shaping produced no words; refusing keeps the spoken label visible.
    EmptyShape,
    /// Shaping changed nothing, so there is no new presentation to commit.
    Unchanged,
    /// The reducer revision counter is exhausted.
    RevisionExhausted,
    /// The ledger refused to authenticate the shaping.
    LedgerRefusal(&'static str),
}

impl std::fmt::Display for IncrementalShapingRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOccurrence => {
                formatter.write_str("occurrence has no committed document entry")
            }
            Self::ForeignSession => formatter.write_str("occurrence belongs to another session"),
            Self::DocumentRevisionOwnsPresentation => {
                formatter.write_str("a document revision already owns this presentation")
            }
            Self::AlreadyShaped => formatter.write_str("occurrence label is already shaped"),
            Self::EmptyShape => formatter.write_str("shaping produced no words"),
            Self::Unchanged => formatter.write_str("incremental shaping is unchanged"),
            Self::RevisionExhausted => formatter.write_str("transcript revision counter exhausted"),
            Self::LedgerRefusal(reason) => {
                write!(formatter, "ledger refused incremental shaping: {reason}")
            }
        }
    }
}

impl std::error::Error for IncrementalShapingRefusal {}

/// The presentation shape held for one occurrence, plus the exact label it was
/// derived from. Keeping the source label is what makes a stale shape visible:
/// when a later observer relabels the occurrence the shape no longer describes
/// the committed words, and the reducer drops it instead of rendering a lie.
type ShapedPresentation = IncrementalShapingReceipt;

#[cfg(test)]
use codescribe_core::pipeline::contracts::PreviewFinalDisposition;

/// Complete visible paint: main canvas and separately painted preview evidence.
#[derive(Debug, Default)]
struct PaintedCanvas {
    text: String,
    preview_only_words: usize,
    visible_words: Vec<VisibleWord>,
    committed_sources: BTreeMap<OccurrenceIdentity, CommittedPaintSource>,
}

#[derive(Debug, Default)]
struct PhrasePreviewPaint {
    current_rev: Option<u64>,
    current_evidence: Option<UnanchoredEvidence>,
    open_revisions: Vec<u64>,
    retained: Vec<UnanchoredEvidence>,
    supersessions: BTreeMap<u64, PreviewSupersession>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewSupersession {
    Partial(u64),
    Final(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommittedPaintSource {
    occurrence: OccurrenceIdentity,
    observation_receipt: String,
    presentation_receipt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VisibleWordSource {
    Committed(CommittedPaintSource),
    Unanchored(ObservationIdentity),
    Refused(usize),
    Preview(u64),
    DocumentRevision {
        receipt: Option<String>,
        members: Vec<CommittedPaintSource>,
        markers: usize,
    },
}

impl VisibleWordSource {
    fn committed_members(&self) -> &[CommittedPaintSource] {
        match self {
            Self::Committed(source) => std::slice::from_ref(source),
            Self::DocumentRevision { members, .. } => members,
            _ => &[],
        }
    }
}

/// A word position belongs to its typed paint source, never to a text match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleWord {
    pub word: String,
    pub preview_rev: Option<u64>,
    source: VisibleWordSource,
    offset: usize,
    covered_by: Option<OccurrenceIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingVisibleWord {
    pub word: String,
    pub reason: String,
}


/// The one committed Rust document plus explicitly non-authoritative UI paint.
/// Only `document_by_occurrence` can produce a committed revision. The preview
/// field is volatile, has no occurrence identity, and is discarded at terminal
/// boundaries without ever entering the Transcript Bus or delivery buffer.
#[derive(Debug, Default)]
pub struct TranscriptReducer {
    /// Canonical document ordered by the PCM-backed occurrence key. W2 alone
    /// connects authenticated ledger actions and emits revisions from it.
    document_by_occurrence: BTreeMap<OccurrenceIdentity, TranscriptDocumentEntry>,
    /// Deterministic presentation for occurrences already closed by a seal.
    /// Keyed by the same physical occurrence, so a later insert appends beside
    /// these shapes instead of replacing the document the way a single global
    /// override would.
    shaped_by_occurrence: BTreeMap<OccurrenceIdentity, ShapedPresentation>,
    consultation_presentations: Vec<ConsultationPresentationReceipt>,
    revision: u64,
    ephemeral_preview: String,
    /// Updated only by a paint command; unpublished reducer changes stay invisible.
    last_painted_canvas: PaintedCanvas,
    latest_seal_coverage: Option<SealCoverageReceipt>,
    latest_comparison: Option<TranscriptComparisonReceipt>,
    context_markers: Vec<DocumentContextMarker>,
    manual_rendered_text: Option<String>,
    manual_document_revision_receipt: Option<String>,
    /// Lifecycle ended; independent of whether the ledger issued a terminal seal.
    terminal: bool,
    terminal_sealed: bool,
    observed_seals: std::collections::BTreeSet<String>,
    applied_observations: Vec<ObservationIdentity>,
    /// Read-only overlap evidence, keyed by the pin's PCM range. It is painted
    /// beside committed occurrences and never becomes a document token. It
    /// lives until a seal closes a committed token over its range, or until
    /// the lifecycle ends.
    unanchored_evidence: BTreeMap<OccurrenceIdentity, (String, NoAuthorityReason, ObservationIdentity)>,
}

/// Whether `inner` lies wholly inside `outer` on one capture clock.
fn range_within(inner: &OccurrenceIdentity, outer: &OccurrenceIdentity) -> bool {
    inner.same_capture(outer)
        && inner.sample_start >= outer.sample_start
        && inner.sample_end <= outer.sample_end
}

fn group_matches_entries<'a>(
    receipt: &ConsultationPresentationReceipt,
    entries: impl Iterator<Item = &'a TranscriptDocumentEntry>,
) -> bool {
    let (Some(first), Some(last)) = (receipt.members.first(), receipt.members.last()) else {
        return false;
    };
    let entries = entries
        .filter(|entry| entry.occurrence >= first.occurrence && entry.occurrence <= last.occurrence)
        .collect::<Vec<_>>();
    entries.len() == receipt.members.len()
        && entries.iter().zip(&receipt.members).all(|(entry, member)| {
            entry.occurrence == member.occurrence
                && entry.label == member.source_label
                && entry.seal_receipt.is_some()
        })
}

/// Preserve fragment bytes; only an absent inter-occurrence separator is added.
fn append_exact_fragment(rendered: &mut String, fragment: &str) {
    if !rendered.is_empty()
        && !fragment.is_empty()
        && !rendered.ends_with(char::is_whitespace)
        && !fragment.starts_with(char::is_whitespace)
        && !fragment.starts_with(". ")
    {
        rendered.push(' ');
    }
    rendered.push_str(fragment);
}

/// Trim a fragment's outer edges. Interior whitespace and newlines survive —
/// the renderer receives markdown, so collapsing them would flatten structure.
fn normalize_transcript_fragment(text: &str) -> String {
    text.trim().to_string()
}

/// Append a fragment to the rendered buffer, inserting a single separating
/// space only when one is actually needed. Empty fragments are skipped, so a
/// blank preview cannot leave trailing whitespace on the canvas.
fn append_rendered_fragment(rendered: &mut String, fragment: &str) {
    let normalized = normalize_transcript_fragment(fragment);
    if normalized.is_empty() {
        return;
    }

    if !rendered.is_empty() && !rendered.ends_with(char::is_whitespace) {
        rendered.push(' ');
    }
    rendered.push_str(&normalized);
}

impl TranscriptReducer {
    fn encode_serial(serial: &AcousticSerial) -> String {
        format!(
            "v{}:{}:{}:{}:{}:{}",
            serial.version,
            serial.digest,
            serial.occurrence.session,
            serial.occurrence.capture_epoch,
            serial.occurrence.sample_start,
            serial.occurrence.sample_end,
        )
    }

    fn revision_for_action(&mut self, action: ReducerAction) -> TranscriptRevision {
        self.revision = self.revision.saturating_add(1);
        let mut entries = self
            .document_by_occurrence
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for entry in &mut entries {
            entry.presentation_receipt = self.shaped_by_occurrence.get(&entry.occurrence).cloned();
        }
        if let Some(receipt) = &self.manual_document_revision_receipt {
            for entry in &mut entries {
                entry.manual_edit_receipt = Some(receipt.clone());
            }
        }
        let rendered_text = self.committed_rendered_text();
        let mut snapshot = TranscriptRevision {
            schema: "codescribe.transcript-revision.v1".to_string(),
            revision: self.revision,
            action,
            entries,
            rendered_text,
            seal_coverage: self.latest_seal_coverage.clone(),
            comparison: self.latest_comparison.clone(),
            consultation_presentations: self.consultation_presentations.clone(),
            publication_digest: [0; 32],
        };
        snapshot.publication_digest = snapshot.digest();
        snapshot
    }

    /// Apply only the mutation authority granted by the shared ledger. An
    /// unsigned or unqualified occurrence fails closed and creates no document
    /// entry, even when an engine supplied visible text.
    /// PCM-ordered canvas: committed tokens, plus unanchored evidence whose
    /// range is not already occupied by one of those tokens.
    pub fn visible_projection(&self) -> String {
        let mut rendered = String::new();
        for (_, text, _, _) in self.visible_paint_fragments() {
            append_exact_fragment(&mut rendered, &text);
        }
        rendered
    }

    /// The same ordered fragments feed the main paint and its STOP receipt.
    fn visible_paint_fragments(&self) -> Vec<(u64, String, VisibleWordSource, bool)> {
        let mut fragments = self.unanchored_evidence.iter()
            .filter(|(range, _)| !self.document_by_occurrence.keys()
                .any(|owner| range_within(range, owner)))
            .map(|(range, (text, _, observation))| (range.sample_start, text.clone(),
                VisibleWordSource::Unanchored(observation.clone()), true))
            .collect::<Vec<_>>();
        let source_for = |range: &OccurrenceIdentity, entry: &TranscriptDocumentEntry| {
            let presentation = self.consultation_presentations.iter()
                .find(|receipt| receipt.members.iter().any(|member| &member.occurrence == range))
                .map(|receipt| receipt.receipt_id.clone())
                .or_else(|| self.shaped_by_occurrence.get(range).map(|receipt| receipt.receipt_id.clone()));
            CommittedPaintSource {
                occurrence: range.clone(),
                observation_receipt: entry.observation_receipt.clone(),
                presentation_receipt: self.manual_document_revision_receipt.clone().or(presentation),
            }
        };
        if fragments.is_empty()
            && (self.manual_rendered_text.is_some() || !self.context_markers.is_empty())
        {
            let source = VisibleWordSource::DocumentRevision {
                receipt: self.manual_document_revision_receipt.clone(),
                members: self.document_by_occurrence.iter()
                    .map(|(range, entry)| source_for(range, entry)).collect(),
                markers: if self.manual_rendered_text.is_some() { 0 } else { self.context_markers.len() },
            };
            fragments.push((0, self.committed_rendered_text(), source, false));
        } else {
            for (range, entry) in &self.document_by_occurrence {
                let source = self.consultation_presentations.iter()
                    .find(|receipt| receipt.members.first()
                        .is_some_and(|member| &member.occurrence == range))
                    .map(|receipt| VisibleWordSource::DocumentRevision {
                        receipt: Some(receipt.receipt_id.clone()),
                        members: receipt.members.iter().filter_map(|member| {
                            self.document_by_occurrence.get(&member.occurrence)
                                .map(|entry| source_for(&member.occurrence, entry))
                        }).collect(),
                        markers: 0,
                    })
                    .unwrap_or_else(|| VisibleWordSource::Committed(source_for(range, entry)));
                fragments.push((range.sample_start, self.presentation_of(range, entry).to_string(),
                    source, false));
            }
        }
        fragments.sort_by_key(|(start, _, _, _)| *start);
        fragments
    }

    /// Every unanchored text of one capture in PCM order, for the paint beside
    /// the canvas. Unlike [`Self::visible_projection`] this keeps ranges wholly
    /// inside a committed token: no string decides what is shown.
    pub fn unanchored_evidence(
        &self,
        session_id: &str,
        capture_epoch: u64,
    ) -> Vec<UnanchoredEvidence> {
        self.unanchored_evidence
            .iter()
            .filter(|(occurrence, _)| {
                occurrence.session == session_id && occurrence.capture_epoch == capture_epoch
            })
            .map(|(occurrence, (label, reason, _))| UnanchoredEvidence {
                sample_start: occurrence.sample_start,
                sample_end: occurrence.sample_end,
                text: label.clone(),
                reason: reason.as_str().to_string(),
            })
            .collect()
    }

    fn project_unanchored(&mut self, observation: &ObservationIdentity, receipt: &MutationReceipt) {
        if let MutationReceipt::KeepVisibleUnanchored {
            occurrence,
            label,
            reason,
        } = receipt
        {
            let label = label.trim();
            if !label.is_empty() {
                self.unanchored_evidence
                    .insert(occurrence.clone(), (label.to_string(), *reason, observation.clone()));
            }
        }
    }

    pub fn apply_ledger_mutation(
        &mut self,
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        receipt: &MutationReceipt,
    ) -> Option<TranscriptRevision> {
        if matches!(receipt, MutationReceipt::KeepVisibleUnanchored { .. }) {
            self.project_unanchored(observation, receipt);
            return None;
        }
        if !receipt.grants_mutation()
            || !ledger.is_qualified(&observation.occurrence)
            || self
                .document_by_occurrence
                .keys()
                .next()
                .is_some_and(|first| first.session != observation.occurrence.session)
        {
            return None;
        }
        if self.applied_observations.contains(observation)
            || !ledger
                .layer_trail_for(&observation.occurrence)
                .any(|decision| {
                    decision.observation == *observation && decision.decision == *receipt
                })
        {
            return None;
        }
        let serial = ledger.serial_of(&observation.occurrence)?;
        let composition = ledger.compose(&observation.occurrence).ok()?;
        let trail = ledger
            .layer_trail_for(&observation.occurrence)
            .filter(|decision| decision.is_evidence_backed())
            .map(|decision| decision.receipt_id.clone())
            .collect::<Vec<_>>();
        if trail.is_empty() || composition.tokens.is_empty() {
            return None;
        }
        self.manual_rendered_text = None;
        self.manual_document_revision_receipt = None;
        let entry = TranscriptDocumentEntry {
            presentation_receipt: None,
            occurrence: observation.occurrence.clone(),
            label: ledger.text_of(&observation.occurrence)?.to_string(),
            observation_receipt: format!(
                "{}:{}:{}:{}",
                observation.producer.as_str(),
                observation.request,
                observation.generation,
                receipt.as_str(),
            ),
            word_evidence_receipts: composition
                .tokens
                .iter()
                .map(|token| {
                    format!(
                        "{}:{}:{}",
                        token.token_ordinal,
                        token.token,
                        token.cited_digests().collect::<Vec<_>>().join(","),
                    )
                })
                .collect(),
            layer_decision_receipts: trail,
            seal_receipt: ledger
                .seal_of(&observation.occurrence)
                .map(|seal| seal.receipt_id.clone()),
            manual_edit_receipt: ledger
                .manual_edits()
                .iter()
                .rev()
                .find(|edit| edit.occurrence == observation.occurrence)
                .map(|edit| edit.receipt_id.clone()),
        };
        let _serial_receipt = Self::encode_serial(serial);
        // A later observer that changes this occurrence's words invalidates the
        // shape taken from the old ones. Only this occurrence loses its shape:
        // its neighbours keep theirs, which is the whole difference between
        // incremental presentation and a global override.
        if self
            .shaped_by_occurrence
            .get(&observation.occurrence)
            .is_some_and(|shaped| shaped.source_label != entry.label)
        {
            self.shaped_by_occurrence.remove(&observation.occurrence);
        }
        self.document_by_occurrence
            .insert(observation.occurrence.clone(), entry.clone());
        self.invalidate_stale_shapes();
        let action = if observation.producer == ObservationProducer::ManualHuman {
            ReducerAction::ApplyManualEdit { entry }
        } else {
            ReducerAction::ApplyLedgerDecision { entry }
        };
        self.applied_observations.push(observation.clone());
        Some(self.revision_for_action(action))
    }

    /// Project ledger-owned finality; the reducer does not decide whether the
    /// frontier is closed and cannot lift the seal later.
    pub fn apply_ledger_seal(&mut self, receipt: &LedgerSealReceipt) -> Option<TranscriptRevision> {
        if !receipt.is_occurrence_seal()
            && self
                .latest_seal_coverage
                .as_ref()
                // Absence of acoustic measurement blocks a terminal projection
                // exactly as measured uncovered speech does.
                .is_some_and(|coverage| !coverage.status.is_complete())
        {
            return None;
        }
        if self.observed_seals.contains(&receipt.receipt_id)
            || receipt
                .sealed_occurrences
                .iter()
                .any(|occurrence| !self.document_by_occurrence.contains_key(occurrence))
        {
            return None;
        }
        for occurrence in &receipt.sealed_occurrences {
            if let Some(entry) = self.document_by_occurrence.get_mut(occurrence) {
                entry.seal_receipt = Some(receipt.receipt_id.clone());
            }
        }
        // A sealed committed token closes the alternatives painted inside it.
        self.unanchored_evidence.retain(|evidence, _| {
            !receipt
                .sealed_occurrences
                .iter()
                .any(|sealed| range_within(evidence, sealed))
        });
        let occurrence = receipt.sealed_occurrences.first()?.clone();
        self.observed_seals.insert(receipt.receipt_id.clone());
        let terminal = !receipt.is_occurrence_seal();
        if terminal {
            self.terminal_sealed = true;
        }
        Some(self.revision_for_action(ReducerAction::RecordLedgerSeal {
            occurrence,
            seal_receipt: receipt.receipt_id.clone(),
            terminal: !receipt.is_occurrence_seal(),
        }))
    }

    /// Commit a whole-document user edit without fabricating per-word acoustic
    /// ownership. The ledger authenticates the exact committed source occurrence
    /// set, then this reducer mints the only new document revision.
    pub fn apply_user_revision(
        &mut self,
        ledger: &mut AcousticLedger,
        intent: &UserRevisionIntent,
    ) -> Result<TranscriptRevision, UserRevisionRefusal> {
        if intent.rendered_text.trim().is_empty() {
            return Err(UserRevisionRefusal::EmptyText);
        }
        let source_occurrences = if intent.provenance == DocumentRevisionProvenance::LightPlus {
            self.authenticated_presentation_occurrences(&intent.session_id, intent.source_revision)?
        } else {
            self.authenticated_revision_occurrences(&intent.session_id, intent.source_revision)?
        };
        if self.committed_rendered_text() == intent.rendered_text {
            return Err(UserRevisionRefusal::Unchanged);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(UserRevisionRefusal::RevisionExhausted)?;
        let receipt = ledger
            .record_manual_document_revision(
                &intent.session_id,
                intent.source_revision,
                revision,
                &intent.rendered_text,
                &source_occurrences,
                intent.provenance,
            )
            .map_err(UserRevisionRefusal::LedgerRefusal)?;
        self.manual_rendered_text = Some(intent.rendered_text.clone());
        self.consultation_presentations.clear();
        self.shaped_by_occurrence.clear();
        self.manual_document_revision_receipt = Some(receipt.receipt_id.clone());
        Ok(self.revision_for_action(ReducerAction::ApplyUserRevision { receipt }))
    }

    /// Replace only the admitted sealed group. Later suffix revisions do not
    /// invalidate its unchanged members; whole-document edits do.
    pub fn apply_consultation_presentation(
        &mut self,
        ledger: &mut AcousticLedger,
        input: ConsultationPresentationInput<'_>,
    ) -> Result<TranscriptRevision, UserRevisionRefusal> {
        if self.terminal || self.terminal_sealed {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "consultation_capture_already_terminal",
            ));
        }
        if self.manual_rendered_text.is_some() {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "consultation_document_already_revised",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(UserRevisionRefusal::RevisionExhausted)?;
        let first = input
            .members
            .first()
            .ok_or(UserRevisionRefusal::NoCommittedDocument)?;
        if self.document_by_occurrence.keys().any(|occurrence| {
            occurrence.session != first.occurrence.session
                || occurrence.capture_epoch != first.occurrence.capture_epoch
        }) {
            return Err(UserRevisionRefusal::SessionMismatch);
        }
        for member in input.members {
            if !self
                .document_by_occurrence
                .get(&member.occurrence)
                .is_some_and(|entry| {
                    entry.label == member.source_label
                        && entry.seal_receipt.as_ref() == Some(&member.seal_receipt)
                })
            {
                return Err(UserRevisionRefusal::LedgerRefusal(
                    "consultation_source_not_projected",
                ));
            }
        }
        // Only the reducer chooses its next revision number, not the provider.
        let receipt = ledger
            .record_consultation_presentation(ConsultationPresentationInput {
                source_revision: self.revision,
                revision,
                ..input
            })
            .map_err(UserRevisionRefusal::LedgerRefusal)?;
        self.consultation_presentations.push(receipt.clone());
        self.invalidate_stale_shapes();
        Ok(self.revision_for_action(ReducerAction::ApplyConsultationPresentation { receipt }))
    }

    fn authenticated_revision_occurrences(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<Vec<OccurrenceIdentity>, UserRevisionRefusal> {
        if !(self.terminal || self.terminal_sealed) {
            return Err(UserRevisionRefusal::NotTerminal);
        }
        self.authenticated_presentation_occurrences(session_id, source_revision)
    }

    fn authenticated_presentation_occurrences(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<Vec<OccurrenceIdentity>, UserRevisionRefusal> {
        if self.document_by_occurrence.is_empty() {
            return Err(UserRevisionRefusal::NoCommittedDocument);
        }
        if source_revision != self.revision {
            return Err(UserRevisionRefusal::StaleRevision {
                expected: self.revision,
                actual: source_revision,
            });
        }
        let source_occurrences = self
            .document_by_occurrence
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if source_occurrences
            .iter()
            .any(|occurrence| occurrence.session != session_id)
        {
            return Err(UserRevisionRefusal::SessionMismatch);
        }
        Ok(source_occurrences)
    }

    /// Shape one committed occurrence's presentation while the session lifecycle
    /// is still open.
    ///
    /// This is the live half of the Light+ floor and it is deliberately narrow.
    /// It shapes exactly one occurrence with committed text, using the committed
    /// text to its left as casing context, and it authenticates that single
    /// occurrence through the ledger. It does not touch the ledger label, does
    /// not claim the document, does not read the ephemeral preview, and cannot
    /// make the reducer terminal. Acoustic finality remains independent.
    pub fn apply_incremental_shaping(
        &mut self,
        ledger: &mut AcousticLedger,
        occurrence: &OccurrenceIdentity,
    ) -> Result<TranscriptRevision, IncrementalShapingRefusal> {
        self.apply_incremental_shaping_with_pause(ledger, occurrence, 0.7)
    }

    pub fn apply_incremental_shaping_with_pause(
        &mut self,
        ledger: &mut AcousticLedger,
        occurrence: &OccurrenceIdentity,
        sentence_pause_sec: f32,
    ) -> Result<TranscriptRevision, IncrementalShapingRefusal> {
        // A whole-document revision (user edit, formatter, terminal Light+)
        // already owns every visible byte. A per-occurrence shape must not
        // fight it for the same document.
        if self.manual_rendered_text.is_some() {
            return Err(IncrementalShapingRefusal::DocumentRevisionOwnsPresentation);
        }
        if self.consultation_presentations.iter().any(|receipt| {
            receipt
                .members
                .iter()
                .any(|member| &member.occurrence == occurrence)
        }) {
            return Err(IncrementalShapingRefusal::AlreadyShaped);
        }
        let source_label = self
            .document_by_occurrence
            .get(occurrence)
            .map(|entry| entry.label.clone())
            .ok_or(IncrementalShapingRefusal::UnknownOccurrence)?;
        let session_id = self
            .document_by_occurrence
            .keys()
            .next()
            .ok_or(IncrementalShapingRefusal::UnknownOccurrence)?
            .session
            .clone();
        if occurrence.session != session_id
            || self
                .document_by_occurrence
                .keys()
                .any(|entry| entry.session != session_id)
        {
            return Err(IncrementalShapingRefusal::ForeignSession);
        }
        if self
            .shaped_by_occurrence
            .get(occurrence)
            .is_some_and(|shaped| shaped.source_label == source_label)
        {
            return Err(IncrementalShapingRefusal::AlreadyShaped);
        }
        let sentence_break_before = self
            .document_by_occurrence
            .keys()
            .take_while(|key| *key < occurrence)
            .last()
            .filter(|previous| previous.capture_epoch == occurrence.capture_epoch)
            .and_then(|previous| {
                let serial = ledger.serial_of(previous)?;
                let samples = previous.sample_end.checked_sub(previous.sample_start)?;
                let gap = occurrence.sample_start.checked_sub(previous.sample_end)?;
                (samples > 0).then_some(
                    gap as f64 * serial.duration_ms / samples as f64
                        >= f64::from(sentence_pause_sec) * 1000.0,
                )
            })
            .unwrap_or(false);
        let left_context = self.rendered_occurrence_span(Some(occurrence));
        let shaped = codescribe_core::pipeline::light_plus::apply_live_span(
            &left_context,
            &source_label,
            sentence_break_before,
        );
        // Shaping that consumed every word (a hesitation-only utterance) must
        // never be committed: an empty presentation would delete spoken audio
        // from the document on the strength of a formatting pass.
        if shaped.trim().is_empty() {
            return Err(IncrementalShapingRefusal::EmptyShape);
        }
        if shaped == source_label {
            return Err(IncrementalShapingRefusal::Unchanged);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(IncrementalShapingRefusal::RevisionExhausted)?;
        let receipt = ledger
            .record_incremental_shaping(IncrementalShapingInput {
                session_id: &session_id,
                source_revision: self.revision,
                revision,
                occurrence,
                source_label: &source_label,
                left_context: &left_context,
                shaped_text: &shaped,
                sentence_break_before,
            })
            .map_err(IncrementalShapingRefusal::LedgerRefusal)?;
        self.shaped_by_occurrence
            .insert(occurrence.clone(), receipt.clone());
        self.invalidate_stale_shapes();
        Ok(self.revision_for_action(ReducerAction::ApplyIncrementalShaping { receipt }))
    }

    /// The shaping receipt currently presenting one occurrence, if the shape
    /// still describes the label the document holds. Read-only provenance for
    /// auditors and tests; it authenticates nothing on its own.
    pub fn shaping_receipt_of(&self, occurrence: &OccurrenceIdentity) -> Option<&str> {
        let shaped = self.shaped_by_occurrence.get(occurrence)?;
        let entry = self.document_by_occurrence.get(occurrence)?;
        (shaped.source_label == entry.label).then_some(shaped.receipt_id.as_str())
    }

    /// Return the exact current terminal document after authenticating the
    /// session/revision compare-and-swap boundary. This is read-only formatter
    /// input and cannot mint a ledger or Bus event.
    pub fn terminal_revision_source(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<String, UserRevisionRefusal> {
        self.authenticated_revision_occurrences(session_id, source_revision)?;
        Ok(self.committed_rendered_text())
    }

    /// The single paid formatter pass a one-turn take is owed at terminal
    /// processing, or `None` when there is nothing to format: no committed
    /// document, not yet terminal, or an empty/whitespace-only turn.
    ///
    /// Read-only. It authenticates nothing and mints nothing; it hands the
    /// controller the exact bytes plus the CAS pair the result must be admitted
    /// against, so the one formatter corridor stays
    /// [`Self::terminal_revision_source`] → provider → `apply_formatter_revision`.
    pub fn terminal_formatter_request(&self) -> Option<TerminalFormatterRequest> {
        if !(self.terminal || self.terminal_sealed) {
            return None;
        }
        let session_id = self.document_by_occurrence.keys().next()?.session.clone();
        let source_text = self.committed_rendered_text();
        if source_text.trim().is_empty() {
            return None;
        }
        Some(TerminalFormatterRequest {
            session_id,
            source_revision: self.revision,
            source_text,
        })
    }

    /// The Light+ revision this document is owed, or `None` when
    /// there is nothing to shape: no committed document, or
    /// the shaped text is byte-identical (Light+ is idempotent, so a second
    /// pass — or a document a formatter already shaped — mints nothing).
    /// Read-only: the intent enters the same corridor as a user edit.
    pub fn light_plus_intent(&self) -> Option<UserRevisionIntent> {
        let session_id = self.document_by_occurrence.keys().next()?.session.clone();
        let source = self.committed_rendered_text();
        if source.trim().is_empty() {
            return None;
        }
        let shaped = codescribe_core::pipeline::light_plus::apply(&source);
        if shaped == source {
            return None;
        }
        Some(UserRevisionIntent {
            session_id,
            source_revision: self.revision,
            rendered_text: shaped,
            provenance: DocumentRevisionProvenance::LightPlus,
        })
    }

    /// Mark terminal review lifecycle without text or a new reducer revision.
    /// A seal verdict is independent of this lifecycle end. A terminal seal
    /// opens edit CAS before this event; a refused seal opens it here instead.
    fn mark_terminal_lifecycle(&mut self) {
        self.terminal = true;
        self.unanchored_evidence.clear();
    }

    /// Record ledger-computed session coverage without changing a single
    /// document entry. The next terminal seal projects the same immutable
    /// receipt and is refused above while it remains incomplete.
    pub fn apply_seal_coverage(
        &mut self,
        receipt: &SealCoverageReceipt,
        comparison: Option<&TranscriptComparisonReceipt>,
    ) -> TranscriptRevision {
        self.latest_seal_coverage = Some(receipt.clone());
        if let Some(comparison) = comparison {
            self.latest_comparison = Some(comparison.clone());
        }
        self.revision_for_action(ReducerAction::RecordSealCoverage {
            receipt: receipt.clone(),
            comparison: comparison.cloned(),
        })
    }

    /// The sole automatic author may relabel only an occurrence the ledger
    /// already holds. The producer must have launched and scheduled its
    /// exact occurrence before this return arrives; the reducer never turns an
    /// unsolicited proposal into its own authority. Only the ledger receipt
    /// reaches the document. The boolean is true only when this call returned
    /// that exact open Formatter slot; the event handler may seal only then.
    pub fn apply_occurrence_label_proposal(
        &mut self,
        ledger: &mut AcousticLedger,
        proposal: &OccurrenceLabelProposal,
    ) -> (bool, Option<TranscriptRevision>) {
        if !proposal.binds_real_samples() {
            return (false, None);
        }
        let occurrence = OccurrenceIdentity::new(
            proposal.session.clone(),
            proposal.capture_epoch,
            proposal.sample_start,
            proposal.sample_end,
        );
        if !ledger.is_qualified(&occurrence) || ledger.text_of(&occurrence).is_none() {
            return (false, None);
        }
        let formatter_is_open = ledger.frontier_of(&occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&ObservationProducer::Formatter)
        });
        if !formatter_is_open {
            return (false, None);
        }
        if proposal.disposition != LabelProposalDisposition::Propose {
            let _ = ledger.note_frontier_return(&occurrence, ObservationProducer::Formatter);
            return (true, None);
        }
        let candidate_label = proposal.proposed_label.trim();
        if candidate_label.is_empty() {
            let _ = ledger.note_frontier_return(&occurrence, ObservationProducer::Formatter);
            return (true, None);
        }
        let observation = ObservationIdentity::new(
            ObservationProducer::Formatter,
            self.revision.saturating_add(1),
            self.revision.saturating_add(1),
            occurrence,
        );
        let receipt = ledger.admit(&observation, candidate_label);
        let _ =
            ledger.note_frontier_return(&observation.occurrence, ObservationProducer::Formatter);
        (
            true,
            self.apply_ledger_mutation(ledger, &observation, &receipt),
        )
    }

    /// Record one controller-authenticated context reference. The captured
    /// position is applied to every later document render, so an early marker
    /// remains anchored as preceding occurrences arrive.
    pub fn record_context_marker(
        &mut self,
        position: usize,
        label: &str,
    ) -> Option<TranscriptRevision> {
        let label = label.trim();
        if label.is_empty() {
            return None;
        }
        let order = self.context_markers.len();
        self.context_markers.push(DocumentContextMarker {
            position,
            label: label.to_string(),
            order,
        });
        Some(
            self.revision_for_action(ReducerAction::RecordContextMarker {
                position,
                label: label.to_string(),
                order,
            }),
        )
    }

    fn committed_rendered_text(&self) -> String {
        if let Some(text) = &self.manual_rendered_text {
            return text.clone();
        }
        let rendered = self.rendered_occurrence_span(None);
        render_context_markers(&rendered, &self.context_markers)
    }

    /// Join the occurrence-ordered document, stopping before `until` when one
    /// is given. Each occurrence contributes its committed shape when the shape
    /// still describes the label the ledger holds, and the spoken label
    /// otherwise — so an unshaped (open, or newly relabelled) occurrence is
    /// rendered word-for-word rather than dropped or guessed at.
    ///
    /// Context markers are deliberately not applied here: they are positioned
    /// against the whole rendered document, and a shaping decision must be
    /// taken against committed occurrence text alone.
    fn rendered_occurrence_span(&self, until: Option<&OccurrenceIdentity>) -> String {
        let mut rendered = String::new();
        for (occurrence, entry) in &self.document_by_occurrence {
            if until.is_some_and(|limit| occurrence == limit) {
                break;
            }
            append_exact_fragment(&mut rendered, self.presentation_of(occurrence, entry));
        }
        rendered
    }

    /// Drop dependent shapes when insertion/relabel changes their exact left
    /// context. Historical ledger receipts remain immutable and inspectable.
    fn invalidate_stale_shapes(&mut self) {
        self.consultation_presentations
            .retain(|receipt| group_matches_entries(receipt, self.document_by_occurrence.values()));
        let mut left = String::new();
        for (occurrence, entry) in &self.document_by_occurrence {
            if let Some(group) = self.consultation_presentations.iter().find(|receipt| {
                receipt
                    .members
                    .iter()
                    .any(|member| &member.occurrence == occurrence)
            }) {
                self.shaped_by_occurrence.remove(occurrence);
                if &group.members[0].occurrence == occurrence {
                    append_exact_fragment(&mut left, &group.rendered_text);
                }
                continue;
            }
            if self
                .shaped_by_occurrence
                .get(occurrence)
                .is_some_and(|shape| {
                    shape.source_label != entry.label || shape.left_context != left
                })
            {
                self.shaped_by_occurrence.remove(occurrence);
            }
            let text = self
                .shaped_by_occurrence
                .get(occurrence)
                .map(|shape| shape.shaped_text.as_str())
                .unwrap_or(&entry.label);
            append_exact_fragment(&mut left, text);
        }
    }

    /// The presentation bytes for one entry: its committed shape while that
    /// shape still matches the ledger label, otherwise the label itself.
    fn presentation_of<'entry>(
        &'entry self,
        occurrence: &OccurrenceIdentity,
        entry: &'entry TranscriptDocumentEntry,
    ) -> &'entry str {
        if let Some(group) = self.consultation_presentations.iter().find(|receipt| {
            receipt
                .members
                .iter()
                .any(|member| &member.occurrence == occurrence)
        }) {
            return if &group.members[0].occurrence == occurrence {
                &group.rendered_text
            } else {
                ""
            };
        }
        match self.shaped_by_occurrence.get(occurrence) {
            Some(shaped) if shaped.source_label == entry.label => shaped.shaped_text.as_str(),
            _ => entry.label.as_str(),
        }
    }

    fn set_ephemeral_preview(&mut self, text: &str) {
        self.ephemeral_preview = normalize_transcript_fragment(text);
    }

    fn clear_ephemeral_preview(&mut self) {
        self.ephemeral_preview.clear();
    }

    fn ephemeral_visual_text(&self) -> String {
        let mut rendered = self.visible_projection();
        append_rendered_fragment(&mut rendered, &self.ephemeral_preview);
        rendered
    }
}

fn render_context_markers(text: &str, markers: &[DocumentContextMarker]) -> String {
    let mut rendered = text.to_string();
    let mut ordered = markers.to_vec();
    ordered.sort_by(|left, right| {
        right
            .position
            .cmp(&left.position)
            .then_with(|| right.order.cmp(&left.order))
    });
    for marker in ordered {
        let chars = rendered.chars().collect::<Vec<_>>();
        let offset = marker.position.min(chars.len());
        let previous = offset.checked_sub(1).and_then(|index| chars.get(index));
        let next = chars.get(offset);
        let splits_word = previous.is_some_and(|ch| ch.is_alphanumeric())
            && next.is_some_and(|ch| ch.is_alphanumeric());
        let leading_space = !splits_word && previous.is_some_and(|ch| !ch.is_whitespace());
        let trailing_space = !splits_word && next.is_some_and(|ch| !ch.is_whitespace());
        let insertion = format!(
            "{}{}{}",
            if leading_space { " " } else { "" },
            marker.label,
            if trailing_space { " " } else { "" }
        );
        let byte_offset = rendered
            .char_indices()
            .nth(offset)
            .map_or(rendered.len(), |(index, _)| index);
        rendered.insert_str(byte_offset, &insertion);
    }
    rendered
}

/// Presentation emitter — the single reducer and ordered delivery writer.
///
/// Implements `EventSink` so it can be plugged directly into `transcription_session`.
/// Observer of one committed Bus projection.
///
/// Named because this is an authority-bearing role, not an anonymous closure
/// slot: it is invoked only after an occurrence-authenticated ledger receipt
/// has already produced a committed revision, never on preview paint. The
/// controller publishes the terminal copy separately after `session_ended`;
/// that copy reads this same Bus book and never re-enters the reducer.
pub type ProjectionObserver = Arc<dyn Fn(&TranscriptBusEvidenceEvent) + Send + Sync>;

/// The overlay-visible canvas at one capture-stop instant. Preview words may
/// be pasted literally at stop, but never gain document revision authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleCanvasSnapshot {
    pub session_id: String,
    pub capture_epoch: u64,
    pub revision: u64,
    pub text: String,
    /// Visible words without committed occurrence authority, including previews
    /// and uncovered evidence. Covered hypotheses are accounted separately.
    pub preview_only_words: usize,
    /// Whether the reducer has an occurrence-backed document to revise.
    pub has_committed_document: bool,
    /// Ledger-qualified speech occurrences for this session and capture epoch.
    pub qualified_occurrences: usize,
    pub visible_words: Vec<VisibleWord>,
    pub preview_supersessions: BTreeMap<u64, PreviewSupersession>,
    /// Provenance from the same paint, including consultation members whose
    /// text is rendered together under the group's first occurrence.
    committed_sources: BTreeMap<OccurrenceIdentity, CommittedPaintSource>,
}

impl VisibleCanvasSnapshot {
    /// Account by phrase lifecycle and PCM identity. A matching word in a
    /// different occurrence cannot hide a missing visible word.
    pub fn missing_words_from(&self, pasted: &Self) -> Vec<MissingVisibleWord> {
        let same_capture = self.session_id == pasted.session_id
            && self.capture_epoch == pasted.capture_epoch;
        self.visible_words.iter().filter_map(|word| {
            let reason = if !same_capture {
                Some("unaccounted".to_string())
            } else if let Some(occurrence) = &word.covered_by {
                Some(format!("covered_by_committed occurrence={occurrence:?}"))
            } else if let Some(disposition) = word.preview_rev
                .and_then(|rev| pasted.preview_supersessions.get(&rev))
            {
                Some(match disposition {
                    PreviewSupersession::Partial(rev) => format!("superseded_by_partial rev={rev}"),
                    PreviewSupersession::Final(rev) => format!("superseded_by_final rev={rev}"),
                })
            } else if pasted.visible_words.iter().any(|present|
                present.covered_by.is_none() && present.source == word.source
                    && present.offset == word.offset)
            {
                None
            } else if !word.source.committed_members().is_empty() {
                let mut reasons = Vec::new();
                for source in word.source.committed_members() {
                    let occurrence = &source.occurrence;
                    if let Some(present) = pasted.committed_sources.get(occurrence) {
                        if source.observation_receipt != present.observation_receipt {
                            reasons.push(format!("relabeled_in_place occurrence={occurrence:?}"));
                        } else if source.presentation_receipt != present.presentation_receipt {
                            reasons.push(format!("reshaped_in_place occurrence={occurrence:?}"));
                        }
                    } else if let Some(owner) = pasted.committed_sources.keys()
                        .find(|owner| range_within(occurrence, owner))
                    {
                        reasons.push(format!("covered_by_committed occurrence={owner:?}"));
                    } else {
                        // A surviving member must never conceal a removed one.
                        return Some(MissingVisibleWord {
                            word: word.word.clone(), reason: "unaccounted".into(),
                        });
                    }
                }
                if !reasons.is_empty() {
                    // A document word has joint member provenance. Keep one
                    // word receipt while naming each changed member.
                    Some(reasons.join("; "))
                } else if matches!(&word.source, VisibleWordSource::DocumentRevision { .. })
                    || pasted.visible_words.iter().any(|present| {
                        matches!(&present.source, VisibleWordSource::DocumentRevision { members, .. }
                            if word.source.committed_members().iter().all(|source| members.contains(source)))
                    })
                {
                    None
                } else {
                    // Unchanged receipts still require the original offset.
                    Some("unaccounted".to_string())
                }
            } else {
                Some("unaccounted".to_string())
            };
            reason.map(|reason| MissingVisibleWord { word: word.word.clone(), reason })
        }).collect()
    }
}

/// All target mutations are serialized through one mpsc worker, guaranteeing
/// that overlay deltas and the shared transcript snapshot see identical order.
pub struct PresentationEmitter {
    cursor_observer: Option<CursorObserver>,
    cursor_capture: std::sync::OnceLock<(String, u64)>,
    cursor_sequence: std::sync::Mutex<u64>,
    cursor_integrity: std::sync::Mutex<Option<SpeechIntegrity>>,
    /// Last bounded paint, not a document or independently reconstructed delta.
    cursor_tail: std::sync::Mutex<String>,
    cursor_unanchored_preview: std::sync::Mutex<Option<UnanchoredEvidence>>,
    phrase_preview_paint: std::sync::Mutex<PhrasePreviewPaint>,
    active_presentation: Arc<std::sync::Mutex<bool>>,
    cmd_tx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<EmitterCmd>>>,
    cmd_handle: Option<tokio::task::JoinHandle<()>>,
    /// One occurrence-keyed committed document plus volatile overlay paint.
    session_state: std::sync::Mutex<TranscriptReducer>,
    /// Durable observer of this exact reducer's committed/final truth.
    transcript_bus: Option<Arc<TranscriptBus>>,
    acoustic_ledger: Option<Arc<std::sync::Mutex<AcousticLedger>>>,
    projection_callback: Option<ProjectionObserver>,
    /// The literal contract (Ctrl-hold `force_raw`): when set, the terminal
    /// seal mints no Light+ revision and the document stays word-for-word.
    /// Every other lane — including auto-format "off" — gets the Light+
    /// floor, exactly as the pre-ledger controller gated it.
    literal_delivery: std::sync::atomic::AtomicBool,
    /// A terminal diagnostic cannot erase visible words before stop snapshots them.
    stop_snapshot_pending: std::sync::atomic::AtomicBool,
    sentence_pause_sec: f32,
    #[cfg(test)]
    paint_commands: std::sync::Mutex<Vec<String>>,
}

impl PresentationEmitter {
    /// Fence only presentation callbacks; the retired take still drains into
    /// its own ledger, reducer, transcript buffer, and archive.
    pub fn retire_presentation(&self) {
        *self.active_presentation.lock().unwrap_or_else(|error| error.into_inner()) = false;
    }

    pub fn with_active_presentation(&self, publish: impl FnOnce()) {
        let active = self.active_presentation.lock().unwrap_or_else(|error| error.into_inner());
        if *active {
            publish();
        }
    }

    /// Capture the visible-word receipt before closing PCM can publish EOF events.
    pub fn begin_stop_canvas(&self) -> Option<VisibleCanvasSnapshot> {
        self.stop_snapshot_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.snapshot_canvas()
    }

    /// Read the paint once the live-final signal settles or its bound expires.
    pub fn finish_stop_canvas(&self) -> Option<VisibleCanvasSnapshot> {
        let snapshot = self.visible_canvas_snapshot();
        self.stop_snapshot_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        snapshot
    }

    /// Fence queued delta publications after the worker's admitted finals.
    /// The caller includes this wait in the same stop deadline.
    pub async fn wait_paint_published(&self) -> bool {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.send_cmd(EmitterCmd::PaintBarrier(sender));
        receiver.await.is_ok()
    }

    /// Freeze the revision that owns delivery for this take, without waiting for
    /// the ordered paint worker or any pending transcription producer.
    pub fn visible_canvas_snapshot(&self) -> Option<VisibleCanvasSnapshot> {
        self.snapshot_canvas()
    }

    fn snapshot_canvas(&self) -> Option<VisibleCanvasSnapshot> {
        let (session_id, capture_epoch) = self.cursor_capture.get()?;
        // Match mutation publication's ledger-before-reducer lock order.
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .map(|ledger| ledger.lock().unwrap_or_else(|error| error.into_inner()));
        let reducer = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let phrase = self.phrase_preview_paint.lock()
            .unwrap_or_else(|error| error.into_inner());
        Some(VisibleCanvasSnapshot {
            session_id: session_id.clone(),
            capture_epoch: *capture_epoch,
            revision: reducer.revision,
            text: reducer.last_painted_canvas.text.clone(),
            preview_only_words: reducer.last_painted_canvas.preview_only_words,
            visible_words: reducer.last_painted_canvas.visible_words.clone(),
            committed_sources: reducer.last_painted_canvas.committed_sources.clone(),
            preview_supersessions: phrase.supersessions.clone(),
            has_committed_document: !reducer.document_by_occurrence.is_empty(),
            qualified_occurrences: ledger.as_ref().map_or(0, |ledger| {
                ledger
                    .qualified_occurrences()
                    .filter(|occurrence| {
                        occurrence.session == *session_id
                            && occurrence.capture_epoch == *capture_epoch
                    })
                    .count()
            }),
        })
    }

    /// Publish the Light+ revision that owns stop delivery before handing its
    /// exact bytes to the destination. Capture is already closed at this point.
    /// Preview-bearing canvases remain literal: a user revision would claim
    /// occurrence authority for words that the ledger has not committed.
    pub fn shape_frozen_canvas_at_stop(
        &self,
        frozen: VisibleCanvasSnapshot,
    ) -> Result<VisibleCanvasSnapshot, UserRevisionRefusal> {
        if frozen.preview_only_words > 0
            || !frozen.has_committed_document
            || self.literal_delivery()
            || frozen.text.trim().is_empty()
        {
            return Ok(frozen);
        }
        let shaped = codescribe_core::pipeline::light_plus::apply(&frozen.text);
        if shaped.is_empty() || shaped == frozen.text {
            return Ok(frozen);
        }
        let commit = self.apply_user_revision(UserRevisionIntent {
            session_id: frozen.session_id.clone(),
            source_revision: frozen.revision,
            rendered_text: shaped,
            provenance: DocumentRevisionProvenance::LightPlus,
        })?;
        // The accepted revision rewrites this frozen document's presentation.
        // Carry its receipt with the pasted bytes, without sampling later paint.
        let mut committed_sources = frozen.committed_sources.clone();
        for source in committed_sources.values_mut() {
            source.presentation_receipt = Some(commit.provenance_receipt.clone());
        }
        let source = VisibleWordSource::DocumentRevision {
            receipt: Some(commit.provenance_receipt.clone()),
            members: committed_sources.values().cloned().collect(),
            markers: 0,
        };
        let mut visible_words = commit.rendered_text.split_whitespace().enumerate()
            .map(|(offset, word)| VisibleWord {
                word: word.to_string(), preview_rev: None, source: source.clone(),
                offset, covered_by: None,
            }).collect::<Vec<_>>();
        visible_words.extend(frozen.visible_words.iter()
            .filter(|word| word.covered_by.is_some()).cloned());
        Ok(VisibleCanvasSnapshot {
            revision: commit.revision,
            text: commit.rendered_text,
            visible_words,
            committed_sources,
            ..frozen
        })
    }

    /// Build the reducer and start its single FIFO delivery worker.
    pub fn new(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self::new_with_transcript_bus(transcript_buffer, delta_callback, stream_log_path, None)
    }

    /// Build an emitter observed by the clean transcript bus. The bus sees the
    /// same reducer mutation as paste/history and never reconstructs UI deltas.
    pub fn new_with_transcript_bus(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
        transcript_bus: Option<Arc<TranscriptBus>>,
    ) -> Self {
        Self::new_with_authority(
            transcript_buffer,
            delta_callback,
            stream_log_path,
            transcript_bus,
            None,
            None,
        )
    }

    /// Build the production reducer projection over the exact ledger bound to
    /// the recorder. No other constructor is used by the controller.
    pub fn new_with_authority(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        acoustic_ledger: Option<Arc<std::sync::Mutex<AcousticLedger>>>,
        projection_callback: Option<ProjectionObserver>,
    ) -> Self {
        // One ordered worker preserves paint order while keeping authority
        // effects explicit. Both command families may paint; only a committed
        // ledger revision may write the shared delivery buffer.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EmitterCmd>();
        let active_presentation = Arc::new(std::sync::Mutex::new(true));
        let worker_presentation = Arc::clone(&active_presentation);
        let cmd_handle = Some(tokio::spawn(async move {
            let mut painted_text = String::new();
            while let Some(cmd) = rx.recv().await {
                let (paint, delivery) = match cmd {
                    EmitterCmd::PublishCommittedRevision { paint, delivery } => {
                        (paint, Some(delivery))
                    }
                    EmitterCmd::PaintEphemeralPreview(paint) => (paint, None),
                    EmitterCmd::PaintBarrier(sender) => {
                        let _ = sender.send(());
                        continue;
                    }
                    EmitterCmd::Finish => break,
                };
                if let Some(delta) = TranscriptDelta::from_diff(&painted_text, &paint) {
                    if let Some(sink) = &delta_callback {
                        let active = worker_presentation.lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if *active {
                            sink.apply(&delta);
                        }
                    }
                    if let Some(path) = stream_log_path.as_deref()
                        && let Err(error) = append_stream_delta(path, &delta.delta)
                    {
                        tracing::warn!(%error, path = %path.display(), "stream delta log append failed");
                    }
                }
                painted_text.clone_from(&paint);
                if let Some(delivery) = delivery {
                    *transcript_buffer.lock().await = delivery;
                }
            }
        }));

        Self {
            cmd_tx: std::sync::Mutex::new(Some(tx)),
            cmd_handle,
            session_state: std::sync::Mutex::new(TranscriptReducer::default()),
            transcript_bus,
            acoustic_ledger,
            projection_callback,
            literal_delivery: std::sync::atomic::AtomicBool::new(false),
            stop_snapshot_pending: std::sync::atomic::AtomicBool::new(false),
            sentence_pause_sec: 0.7,
            cursor_observer: None,
            cursor_capture: std::sync::OnceLock::new(),
            cursor_sequence: std::sync::Mutex::new(0),
            cursor_integrity: std::sync::Mutex::new(None),
            cursor_tail: std::sync::Mutex::new(String::new()),
            cursor_unanchored_preview: std::sync::Mutex::new(None),
            phrase_preview_paint: std::sync::Mutex::new(PhrasePreviewPaint::default()),
            active_presentation,
            #[cfg(test)]
            paint_commands: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Declare the literal contract for this take. `true` is the Ctrl-hold
    /// `force_raw` lane: the terminal seal then delivers the ledger words
    /// untouched. Default `false`: Light+ shapes the terminal document.
    pub fn set_literal_delivery(&self, literal: bool) {
        self.literal_delivery
            .store(literal, std::sync::atomic::Ordering::SeqCst);
    }

    /// Set from the immutable settings generation held by this capture.
    pub fn with_sentence_pause_sec(mut self, seconds: f32) -> Self {
        self.sentence_pause_sec = seconds.clamp(0.3, 2.0);
        self
    }

    /// Observe ephemeral paint without granting document or delivery authority.
    pub fn with_cursor_observer(mut self, observer: CursorObserver) -> Self {
        self.cursor_observer = Some(observer);
        self
    }

    fn paint_cursor(&self, rendered: &str) {
        if self.cursor_observer.is_none() {
            return;
        }
        let mut words = rendered
            .split_whitespace()
            .rev()
            .take(5)
            .collect::<Vec<_>>();
        words.reverse();
        *self.cursor_tail.lock().unwrap_or_else(|e| e.into_inner()) = words.join(" ");
        self.repaint_cursor();
    }

    fn repaint_cursor(&self) {
        let Some(observer) = &self.cursor_observer else {
            return;
        };
        let Some((session_id, capture_epoch)) = self.cursor_capture.get() else {
            return;
        };
        // Serialize snapshots and observer calls together: an earlier paint
        // must never be published after a later sequence.
        let mut sequence = self
            .cursor_sequence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(next) = sequence.checked_add(1) else {
            return;
        };
        let integrity = self
            .cursor_integrity
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let degraded = integrity.as_ref().is_some_and(|e| {
            matches!(
                e.phase,
                SpeechIntegrityPhase::Stalled
                    | SpeechIntegrityPhase::Recovering
                    | SpeechIntegrityPhase::Unresolved
            )
        });
        let tail = self.cursor_tail.lock().unwrap_or_else(|e| e.into_inner());
        // Snapshot under the sequence lock so a later sequence can never carry
        // older evidence. Callers never hold the reducer lock while painting.
        let mut evidence = self
            .session_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unanchored_evidence(session_id, *capture_epoch);
        evidence.extend(self.phrase_preview_paint.lock()
            .unwrap_or_else(|error| error.into_inner()).retained.clone());
        if let Some(preview) = self.cursor_unanchored_preview.lock()
            .unwrap_or_else(|error| error.into_inner()).as_ref()
        {
            evidence.push(preview.clone());
        }
        evidence.sort_by_key(|item| (item.sample_start, item.sample_end));
        *sequence = next;
        self.with_active_presentation(|| observer(&CompactProjection {
            session_id: session_id.clone(),
            capture_epoch: *capture_epoch,
            sequence: next,
            // Earlier recovery debt must not hide words arriving now. Amber
            // stays authoritative until that debt is actually resolved.
            text: if degraded && tail.is_empty() {
                "…".into()
            } else {
                tail.clone()
            },
            degraded,
            evidence,
        }));
    }

    /// Whether this take promised literal words (see [`Self::set_literal_delivery`]).
    pub fn literal_delivery(&self) -> bool {
        self.literal_delivery
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Signal the emitter to finish after every queued reducer revision.
    pub async fn finish(&mut self) {
        // Send Finish through channel (ordered after all pending pushes).
        if let Ok(guard) = self.cmd_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(EmitterCmd::Finish);
        }

        if let Some(handle) = self.cmd_handle.take()
            && let Err(e) = handle.await
        {
            tracing::error!("Emitter cmd worker failed: {}", e);
        }
    }

    /// Send a command to the emitter worker (non-blocking, ordered).
    fn send_cmd(&self, mut cmd: EmitterCmd) {
        match &mut cmd {
            EmitterCmd::PublishCommittedRevision { paint, .. }
            | EmitterCmd::PaintEphemeralPreview(paint) => {
                let state = self.session_state.lock()
                    .unwrap_or_else(|error| error.into_inner());
                let phrase = self.phrase_preview_paint.lock()
                    .unwrap_or_else(|error| error.into_inner());
                let mut fragments = state.visible_paint_fragments();
                let covered = state.unanchored_evidence.iter().filter_map(|(range, (text, _, observation))| {
                    state.document_by_occurrence.keys()
                        .find(|owner| range_within(range, owner))
                        .map(|owner| (observation.clone(), text.clone(), owner.clone()))
                }).collect::<Vec<_>>();
                for (id, evidence) in phrase.retained.iter().enumerate() {
                    fragments.push((evidence.sample_start, evidence.text.clone(),
                        VisibleWordSource::Refused(id), true));
                }
                fragments.sort_by_key(|(start, _, _, _)| *start);
                let current_zero_preview = self.cursor_unanchored_preview.lock()
                    .unwrap_or_else(|error| error.into_inner()).clone();
                *paint = state.visible_projection();
                if let (Some(rev), Some(evidence)) = (phrase.current_rev, phrase.current_evidence.as_ref())
                {
                    if current_zero_preview.is_none() {
                        append_exact_fragment(paint, &evidence.text);
                    }
                    fragments.push((evidence.sample_start, evidence.text.clone(),
                        VisibleWordSource::Preview(rev), true));
                }
                let mut visible = String::new();
                let mut visible_words = Vec::new();
                let mut preview_only_words = 0;
                let mut committed_sources = BTreeMap::new();
                for (_, text, source, is_evidence) in fragments {
                    for member in source.committed_members() {
                        committed_sources.insert(member.occurrence.clone(), member.clone());
                    }
                    append_exact_fragment(&mut visible, &text);
                    if is_evidence {
                        preview_only_words += text.split_whitespace().count();
                    }
                    let preview_rev = match &source {
                        VisibleWordSource::Preview(rev) => Some(*rev),
                        _ => None,
                    };
                    visible_words.extend(text.split_whitespace().enumerate().map(|(offset, word)| VisibleWord {
                        word: word.to_string(), preview_rev, source: source.clone(), offset, covered_by: None,
                    }));
                }
                // The evidence list paints these hypotheses, but their PCM is
                // already represented by a committed occurrence in the paste.
                for (observation, text, owner) in covered {
                    visible_words.extend(text.split_whitespace().enumerate().map(|(offset, word)| VisibleWord {
                        word: word.to_string(), preview_rev: None,
                        source: VisibleWordSource::Unanchored(observation.clone()), offset,
                        covered_by: Some(owner.clone()),
                    }));
                }
                drop(phrase);
                drop(state);
                #[cfg(test)]
                self.paint_commands.lock().unwrap().push(visible.clone());
                self.session_state.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .last_painted_canvas = PaintedCanvas {
                        text: visible,
                        preview_only_words,
                        visible_words,
                        committed_sources,
                    };
                self.paint_cursor(paint);
            }
            EmitterCmd::PaintBarrier(_) | EmitterCmd::Finish => {}
        }
        if let Ok(guard) = self.cmd_tx.lock()
            && let Some(tx) = guard.as_ref()
            && tx.send(cmd).is_err()
        {
            debug!("Emitter channel closed, dropping command");
        }
    }

    fn authenticates_revision(
        &self,
        revision: &TranscriptRevision,
        ledger: &AcousticLedger,
    ) -> bool {
        let Some(first) = revision.entries.first() else {
            return false;
        };
        let session = self
            .transcript_bus
            .as_ref()
            .map(|bus| bus.session_id())
            .unwrap_or(&first.occurrence.session);
        revision.authenticates_publication(ledger, session)
    }

    fn publish_revision(&self, revision: TranscriptRevision) {
        if let Some(ledger) = &self.acoustic_ledger {
            let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            if !self.authenticates_revision(&revision, &ledger) {
                return;
            }
            if let Some(bus) = &self.transcript_bus {
                let events = bus.publish_revision(&revision, &ledger);
                if events.is_empty() {
                    return;
                }
                if let Some(callback) = &self.projection_callback {
                    for event in &events {
                        self.with_active_presentation(|| callback(event));
                    }
                }
            }
        } else {
            return;
        }
        self.send_committed_paint(revision.rendered_text);
    }

    /// Paint the visible projection. Delivery receives only the committed text.
    fn send_committed_paint(&self, delivery: String) {
        let paint = {
            let state = self
                .session_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.visible_projection()
        };
        self.send_cmd(EmitterCmd::PublishCommittedRevision { paint, delivery });
    }

    /// Accept one explicit overlay revision intent against the retained terminal
    /// reducer. The acknowledgement carries no authority to Swift: projection is
    /// emitted first, and only that callback may repaint the canvas.
    pub fn apply_user_revision(
        &self,
        intent: UserRevisionIntent,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        self.commit_document_revision(&mut ledger, intent)
    }

    /// Publish a settled Agent answer through the same reducer, Bus and ordered
    /// delivery worker. Only the retained consultation's completed group result
    /// can enter here; raw text cannot assert execution/history settlement.
    pub fn apply_consultation_presentation(
        &self,
        completed: &ConsultationGroupAnswer,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        if self.literal_delivery() {
            return Err(UserRevisionRefusal::FormatterUnavailable);
        }
        if self
            .cmd_tx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_none_or(|sender| sender.is_closed())
        {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "consultation_delivery_closed",
            ));
        }
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        let answer = completed.answer();
        let revision = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .apply_consultation_presentation(
                &mut ledger,
                ConsultationPresentationInput {
                    consultation_id: &answer.delivery.backend_id,
                    turn_id: &answer.turn_id,
                    // The reducer chooses both revision numbers under its lock.
                    source_revision: 0,
                    revision: 0,
                    members: completed.input().members(),
                    rendered_text: &answer.text,
                },
            )?;
        if !self.authenticates_revision(&revision, &ledger) {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "publication_authentication_failed",
            ));
        }
        if let Some(bus) = &self.transcript_bus {
            let events = bus.publish_revision(&revision, &ledger);
            if events.is_empty() {
                return Err(UserRevisionRefusal::LedgerRefusal(
                    "bus_publication_refused",
                ));
            }
            if let Some(callback) = &self.projection_callback {
                for event in &events {
                    self.with_active_presentation(|| callback(event));
                }
            }
        }
        self.send_committed_paint(revision.rendered_text.clone());
        let ReducerAction::ApplyConsultationPresentation { receipt } = &revision.action else {
            unreachable!("group admission must mint a group action")
        };
        Ok(UserRevisionCommit {
            session_id: receipt.members[0].occurrence.session.clone(),
            source_revision: receipt.source_revision,
            revision: revision.revision,
            rendered_text: revision.rendered_text.clone(),
            provenance_receipt: receipt.receipt_id.clone(),
        })
    }

    /// The one whole-document revision corridor: reducer mints the revision
    /// against the ledger, the Bus observes it, the delivery buffer follows.
    /// User edits, formatter results, and the terminal Light+ pass all enter
    /// here; only the provenance differs.
    fn commit_document_revision(
        &self,
        ledger: &mut AcousticLedger,
        intent: UserRevisionIntent,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let revision = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .apply_user_revision(ledger, &intent)?;
        if !self.authenticates_revision(&revision, ledger) {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "publication_authentication_failed",
            ));
        }
        if let Some(bus) = &self.transcript_bus {
            let events = bus.publish_revision(&revision, ledger);
            if events.is_empty() {
                return Err(UserRevisionRefusal::LedgerRefusal(
                    "bus_publication_refused",
                ));
            }
            if let Some(callback) = &self.projection_callback {
                for event in &events {
                    self.with_active_presentation(|| callback(event));
                }
            }
        }
        self.send_committed_paint(revision.rendered_text.clone());
        let ReducerAction::ApplyUserRevision { receipt } = &revision.action else {
            unreachable!("apply_user_revision must mint an ApplyUserRevision action")
        };
        Ok(UserRevisionCommit {
            session_id: receipt.session_id.clone(),
            source_revision: receipt.source_revision,
            revision: revision.revision,
            rendered_text: revision.rendered_text,
            provenance_receipt: receipt.receipt_id.clone(),
        })
    }

    /// Light+ document shape at terminal or after a frozen Stop revision gains
    /// late words — capital at sentence starts, a closing period,
    /// hesitation sounds dropped, punctuation seams collapsed — minted as one
    /// ledger-stamped document revision with provenance `light-plus`, so the
    /// Bus, the delivery buffer, and the formatter CAS all see the same bytes.
    /// It runs for every lane except the literal contract and never touches
    /// an occurrence label: the ledger stays the sole author of the words.
    fn mint_light_plus_revision(&self, ledger: &mut AcousticLedger) {
        if self.literal_delivery() {
            return;
        }
        let intent = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .light_plus_intent();
        let Some(intent) = intent else {
            return;
        };
        match self.commit_document_revision(ledger, intent) {
            Ok(commit) => debug!(
                revision = commit.revision,
                receipt = %commit.provenance_receipt,
                "Light+ terminal revision committed"
            ),
            Err(UserRevisionRefusal::Unchanged) => {}
            Err(refusal) => debug!(%refusal, "Light+ terminal revision refused"),
        }
    }

    /// The live half of the Light+ floor. Every occurrence with admitted text
    /// gains its deterministic presentation immediately, so a long
    /// take reads as sentences while it is still being spoken instead of
    /// waiting for Stop.
    ///
    /// It publishes through the ordinary committed corridor — Bus projection,
    /// projection callback, delivery buffer — and deliberately not through the
    /// lifecycle one: no `session_ended`, no terminal flag, no delivery
    /// acknowledgment. The reducer stays non-terminal, so the terminal user
    /// edit and the paid formatter keep their existing terminal-only law.
    ///
    /// The literal contract (`force_raw`) skips this exactly as it skips the
    /// terminal pass: a raw take is delivered word-for-word.
    fn mint_incremental_light_plus(
        &self,
        ledger: &mut AcousticLedger,
        _occurrences: &[OccurrenceIdentity],
    ) {
        if self.literal_delivery() {
            return;
        }
        let occurrences = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .document_by_occurrence
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for occurrence in &occurrences {
            let shaped = {
                let mut reducer = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                reducer.apply_incremental_shaping_with_pause(
                    ledger,
                    occurrence,
                    self.sentence_pause_sec,
                )
            };
            match shaped {
                Ok(revision) => {
                    if !self.authenticates_revision(&revision, ledger) {
                        continue;
                    }
                    if let Some(bus) = &self.transcript_bus {
                        let events = bus.publish_revision(&revision, ledger);
                        if events.is_empty() {
                            continue;
                        }
                        if let Some(callback) = &self.projection_callback {
                            for event in &events {
                                self.with_active_presentation(|| callback(event));
                            }
                        }
                    }
                    self.send_committed_paint(revision.rendered_text);
                }
                // A repeated seal observation and a shape that changes nothing
                // are both healthy no-ops, not failures.
                Err(
                    IncrementalShapingRefusal::AlreadyShaped | IncrementalShapingRefusal::Unchanged,
                ) => {}
                Err(refusal) => debug!(%refusal, "live Light+ shaping refused"),
            }
        }
    }

    /// Authenticate formatter input without mutating reducer, ledger, Bus, or
    /// delivery state. The controller feeds these exact bytes into the one
    /// production formatter pipeline and retains the source revision as CAS.
    pub fn terminal_revision_source(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<String, UserRevisionRefusal> {
        self.session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .terminal_revision_source(session_id, source_revision)
    }

    /// The one paid formatter pass this terminal take is owed, if any.
    ///
    /// Literal delivery keeps its contract: a raw take is never reshaped, so it
    /// asks for no provider call at all. Everything else is decided by the
    /// reducer — no committed document, not terminal yet, or an empty turn all
    /// mean no request, and therefore no provider call to skip afterwards.
    pub fn terminal_formatter_request(&self) -> Option<TerminalFormatterRequest> {
        if self.literal_delivery() {
            return None;
        }
        self.session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .terminal_formatter_request()
    }

    /// Admit only a successful formatter result into the existing revision
    /// corridor. Failure, skipped policy, and healthy no-op are visible
    /// refusals and therefore mint no ledger/history/Copy-last evidence.
    pub fn apply_formatter_revision(
        &self,
        session_id: String,
        source_revision: u64,
        result: AiFormatResult,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let rendered_text = match result.status {
            AiFormatStatus::Applied => result.text,
            AiFormatStatus::Failed => return Err(UserRevisionRefusal::FormatterFailed),
            AiFormatStatus::Skipped => return Err(UserRevisionRefusal::FormatterUnavailable),
            AiFormatStatus::AiNoop => return Err(UserRevisionRefusal::FormatterNoop),
        };
        self.apply_user_revision(UserRevisionIntent {
            session_id,
            source_revision,
            rendered_text,
            provenance: DocumentRevisionProvenance::Formatter,
        })
    }
}

impl Drop for PresentationEmitter {
    /// Close the cmd channel and abort emitter worker tasks to avoid leaks.
    fn drop(&mut self) {
        // Close command channel first (lets cmd worker exit naturally).
        if let Ok(mut guard) = self.cmd_tx.lock() {
            let _ = guard.take();
        }
        // Abort the detached worker as a hard stop fallback to avoid leaks.
        if let Some(handle) = self.cmd_handle.take() {
            handle.abort();
        }
    }
}

impl EventSink for PresentationEmitter {
    fn consultation_destinations(&self) -> usize {
        1
    }

    fn on_consultation_completed(&self, completed: &ConsultationGroupAnswer) -> anyhow::Result<()> {
        self.apply_consultation_presentation(completed)
            .map(|_| ())
            .map_err(anyhow::Error::new)
    }

    fn on_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        if session_id.is_empty()
            || capture_epoch == 0
            || self
                .transcript_bus
                .as_ref()
                .is_some_and(|bus| bus.session_id() != session_id)
        {
            return;
        }
        // One emitter belongs to one opened capture. A late lifecycle callback
        // cannot rebind it to a successor or erase its warning evidence.
        if self
            .cursor_capture
            .set((session_id.to_owned(), capture_epoch))
            .is_ok()
        {
            self.repaint_cursor();
        }
    }

    /// Route an `EngineEvent` into reducer state and ordered delta delivery.
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::SpeechIntegrity { evidence } => {
                if !self.cursor_capture.get().is_some_and(|(session, epoch)| {
                    session == &evidence.session_id && *epoch == evidence.capture_epoch
                }) {
                    return;
                }
                {
                    let mut current = self
                        .cursor_integrity
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current.as_ref().is_some_and(|old| {
                        old.session_id != evidence.session_id
                            || old.capture_epoch != evidence.capture_epoch
                            || old.sequence >= evidence.sequence
                    }) {
                        return;
                    }
                    *current = Some(evidence.clone());
                }
                self.repaint_cursor();
            }
            EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } => {
                let Some(ledger) = &self.acoustic_ledger else {
                    return;
                };
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                let unanchored = matches!(receipt, MutationReceipt::KeepVisibleUnanchored { .. });
                let mut state = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let was_stop_revision = state
                    .manual_document_revision_receipt
                    .as_deref()
                    .is_some_and(|id| id.starts_with("light-plus-"));
                let revision = state.apply_ledger_mutation(&ledger, observation, receipt);
                let visible = state.visible_projection();
                drop(state);
                if unanchored {
                    drop(ledger);
                    if !visible.trim().is_empty() {
                        self.send_cmd(EmitterCmd::PaintEphemeralPreview(visible));
                    }
                    return;
                }
                if let Some(revision) = revision {
                    if !self.authenticates_revision(&revision, &ledger) {
                        return;
                    }
                    if let Some(bus) = &self.transcript_bus {
                        let events = bus.publish_revision(&revision, &ledger);
                        if events.is_empty() {
                            return;
                        }
                        if let Some(callback) = &self.projection_callback {
                            for event in &events {
                                self.with_active_presentation(|| callback(event));
                            }
                        }
                    }
                    self.send_cmd(
                        EmitterCmd::PublishCommittedRevision {
                            paint: visible,
                            delivery: revision.rendered_text,
                        },
                    );
                    if was_stop_revision {
                        self.mint_light_plus_revision(&mut ledger);
                    } else {
                        self.mint_incremental_light_plus(&mut ledger, &[]);
                    }
                }
            }
            EngineEvent::ContextMarker { position, label } => {
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .record_context_marker(*position, label);
                if let Some(revision) = revision {
                    self.publish_revision(revision);
                }
            }
            EngineEvent::LedgerSeal { receipt } => {
                let Some(ledger) = &self.acoustic_ledger else {
                    return;
                };
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                if !ledger.authenticates_seal(receipt) {
                    return;
                }
                let (revision, evidence_closed) = {
                    let mut state = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let before = state.unanchored_evidence.len();
                    let revision = state.apply_ledger_seal(receipt);
                    (revision, state.unanchored_evidence.len() != before)
                };
                // Evidence the seal closed leaves the paint now, not at the
                // next paint that happens to follow.
                if evidence_closed {
                    self.repaint_cursor();
                }
                let Some(revision) = revision else {
                    return;
                };
                if !self.authenticates_revision(&revision, &ledger) {
                    return;
                }
                let terminal = matches!(
                    revision.action,
                    ReducerAction::RecordLedgerSeal { terminal: true, .. }
                );
                if let Some(bus) = &self.transcript_bus {
                    let events = bus.publish_revision(&revision, &ledger);
                    if events.is_empty() {
                        return;
                    }
                    if let Some(callback) = &self.projection_callback {
                        for event in &events {
                            self.with_active_presentation(|| callback(event));
                        }
                    }
                }
                // The terminal seal closes the ledger's word authority; the
                // Light+ document revision follows immediately, before the controller
                // publishes `session_ended`, so the terminal projection Swift
                // holds already carries the shaped bytes and revision number.
                if terminal {
                    self.mint_light_plus_revision(&mut ledger);
                } else {
                    // An occurrence seal closes exactly those words and nothing
                    // else. Retry presentation for any committed span that was
                    // not shaped at admission; the lifecycle stays open.
                    self.mint_incremental_light_plus(&mut ledger, &receipt.sealed_occurrences);
                }
            }
            EngineEvent::SealCoverage {
                receipt,
                comparison,
            } => {
                let Some(ledger) = &self.acoustic_ledger else {
                    return;
                };
                let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                // Coverage is ledger evidence, never an unauthenticated event
                // payload that may overwrite the reducer's refusal diagnostics.
                if ledger.latest_seal_coverage() != Some(receipt) {
                    return;
                }
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .apply_seal_coverage(receipt, comparison.as_ref());
                if let Some(bus) = &self.transcript_bus {
                    let events = bus.publish_revision(&revision, &ledger);
                    if let Some(callback) = &self.projection_callback {
                        for event in &events {
                            self.with_active_presentation(|| callback(event));
                        }
                    }
                }
            }
            EngineEvent::OccurrenceLabelProposal { proposal } => {
                let Some(ledger) = &self.acoustic_ledger else {
                    return;
                };
                let occurrence = OccurrenceIdentity::new(
                    proposal.session.clone(),
                    proposal.capture_epoch,
                    proposal.sample_start,
                    proposal.sample_end,
                );
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                let (proposal_revision, seal_revision, evidence_closed) = {
                    let mut reducer = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let before = reducer.unanchored_evidence.len();
                    let (formatter_returned, proposal_revision) =
                        reducer.apply_occurrence_label_proposal(&mut ledger, proposal);
                    let seal_revision = formatter_returned
                        .then(|| ledger.seal(&occurrence).ok().cloned())
                        .flatten()
                        .and_then(|receipt| reducer.apply_ledger_seal(&receipt));
                    let evidence_closed = reducer.unanchored_evidence.len() != before;
                    (proposal_revision, seal_revision, evidence_closed)
                };
                if evidence_closed {
                    self.repaint_cursor();
                }
                for (is_label_revision, revision) in
                    [(true, proposal_revision), (false, seal_revision)]
                        .into_iter()
                        .filter_map(|(is_label_revision, revision)| {
                            revision.map(|revision| (is_label_revision, revision))
                        })
                {
                    if !self.authenticates_revision(&revision, &ledger) {
                        continue;
                    }
                    if let Some(bus) = &self.transcript_bus {
                        let events = bus.publish_revision(&revision, &ledger);
                        if events.is_empty() {
                            continue;
                        }
                        if let Some(callback) = &self.projection_callback {
                            for event in &events {
                                self.with_active_presentation(|| callback(event));
                            }
                        }
                    }
                    if is_label_revision {
                        self.send_committed_paint(revision.rendered_text);
                    }
                }
                // The proposal corridor can admit a new committed label.
                // Retry presentation; the reducer refuses an already-shaped
                // occurrence, so the same words mint at most one revision.
                self.mint_incremental_light_plus(&mut ledger, std::slice::from_ref(&occurrence));
            }
            EngineEvent::VadStart { .. } | EngineEvent::VadEnd { .. } => {}
            EngineEvent::SidebandEvidence { evidence } => {
                debug!(
                    sequence = evidence.sequence,
                    sample_start = evidence.range.sample_start,
                    sample_end = evidence.range.sample_end,
                    "PresentationEmitter observed sideband evidence without mutating text"
                );
            }
            EngineEvent::PreviewDisposition {
                superseded_through_rev,
                final_disposition: _,
                refused_evidence,
            } => {
                let mut phrase = self.phrase_preview_paint.lock()
                    .unwrap_or_else(|error| error.into_inner());
                let closed_revisions = phrase.open_revisions.iter().copied()
                    .filter(|rev| rev <= superseded_through_rev).collect::<Vec<_>>();
                phrase.open_revisions.retain(|rev| rev > superseded_through_rev);
                for rev in closed_revisions {
                    // Preserve the first replacement: a stopped rev 5 replaced
                    // by partial 6 remains attributed to partial 6 after final 6.
                    phrase.supersessions.entry(rev)
                        .or_insert(PreviewSupersession::Final(*superseded_through_rev));
                }
                let closes_current = phrase.current_rev
                    .is_some_and(|rev| rev <= *superseded_through_rev);
                let preview_range = if closes_current { phrase.current_evidence.clone() } else { None };
                for refused in refused_evidence {
                    let (sample_start, sample_end) = refused.range.as_ref()
                        .map(|range| (range.sample_start, range.sample_end))
                        .or_else(|| preview_range.as_ref().map(|range| (range.sample_start, range.sample_end)))
                        .unwrap_or((0, 0));
                    phrase.retained.push(UnanchoredEvidence {
                        sample_start, sample_end, text: refused.text.clone(), reason: refused.reason.clone(),
                    });
                }
                if closes_current {
                    phrase.current_rev = None;
                    phrase.current_evidence = None;
                }
                drop(phrase);
                if closes_current {
                    *self.cursor_unanchored_preview.lock()
                        .unwrap_or_else(|error| error.into_inner()) = None;
                }
                let paint = {
                    let mut state = self.session_state.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if closes_current { state.clear_ephemeral_preview(); }
                    state.ephemeral_visual_text()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(paint));
            }
            EngineEvent::Preview { rev, text, pin } => {
                {
                    let mut phrase = self.phrase_preview_paint.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if let Some(previous) = phrase.current_rev
                        && previous != *rev
                    {
                        phrase.supersessions.entry(previous)
                            .or_insert(PreviewSupersession::Partial(*rev));
                    }
                    phrase.current_rev = Some(*rev);
                    phrase.current_evidence = Some(UnanchoredEvidence {
                        sample_start: pin.range.sample_start,
                        sample_end: pin.range.sample_end,
                        text: text.clone(),
                        reason: pin.receipt.as_str().to_string(),
                    });
                    phrase.open_revisions.push(*rev);
                }
                // One line per L0 paint, so a take traces partial -> paint on
                // the capture clock. Text is counted, never logged.
                info!(
                    rev = *rev,
                    session = %pin.range.session,
                    capture_epoch = pin.range.capture_epoch,
                    sample_start = pin.range.sample_start,
                    sample_end = pin.range.sample_end,
                    grain = ?pin.grain,
                    receipt = pin.receipt.as_str(),
                    text_chars = text.chars().count(),
                    "L0 preview painted"
                );
                if pin.receipt == PreviewPinReceipt::UnanchoredZeroWidth {
                    *self
                        .cursor_unanchored_preview
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(UnanchoredEvidence {
                        sample_start: pin.range.sample_start,
                        sample_end: pin.range.sample_end,
                        text: text.clone(),
                        reason: pin.receipt.as_str().to_string(),
                    });
                    let canonical = {
                        let mut state =
                            self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.clear_ephemeral_preview();
                        state.visible_projection()
                    };
                    self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical));
                    self.repaint_cursor();
                    return;
                }
                *self
                    .cursor_unanchored_preview
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                self.repaint_cursor();
                let visual_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.set_ephemeral_preview(text);
                    state.ephemeral_visual_text()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(visual_text));
            }
            EngineEvent::UtteranceFinal { utterance_id, .. } => {
                debug!(
                    utterance_id = *utterance_id,
                    "PresentationEmitter observed raw final without mutating product text"
                );
            }
            EngineEvent::Correction { .. }
            | EngineEvent::ReplaceRange { .. }
            | EngineEvent::InsertAnnotation { .. } => {
                debug!(
                    "PresentationEmitter observed diagnostic text event without mutating product text"
                );
            }
            EngineEvent::NoSpeech { reason } => {
                if self
                    .stop_snapshot_pending
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    // Lane failure is not a retraction of words already shown.
                    self.session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .mark_terminal_lifecycle();
                    info!("Engine reported no speech during stop: {}", reason);
                    return;
                }
                *self
                    .cursor_unanchored_preview
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                self.repaint_cursor();
                {
                    let mut phrase = self.phrase_preview_paint.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    phrase.current_rev = None;
                    phrase.current_evidence = None;
                }
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
                    state.clear_ephemeral_preview();
                    state.visible_projection()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                info!("Engine reported no speech: {}", reason);
            }
            EngineEvent::Drop { kind, text, reason } => {
                debug!(
                    "Engine dropped: {:?} — {} (text: '{}')",
                    kind,
                    reason,
                    text.chars().take(50).collect::<String>()
                );
            }
            EngineEvent::Stats {
                hallucination_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                dropped_audio_chunks,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => {
                info!(
                    "Session stats: utterances={}, hallucinations={}, filtered_empty={}, corrections={}, dropped_chunks={}, partial_runs={} (utterance={}, speech={}, watchdog={}, stale={}, coalesced={}, dropped={})",
                    total_utterances,
                    hallucination_drops,
                    filtered_empty_drops,
                    corrections_applied,
                    dropped_audio_chunks,
                    partial_runs_total,
                    trigger_utterance_count,
                    trigger_speech_count,
                    trigger_timer_count,
                    partial_stale_count,
                    partial_coalesced_count,
                    partial_dropped_count,
                );
                if self
                    .stop_snapshot_pending
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    return;
                }
                {
                    let mut phrase = self.phrase_preview_paint.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    phrase.current_rev = None;
                    phrase.current_evidence = None;
                }
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.clear_ephemeral_preview();
                    state.visible_projection()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                // Capture is over, but terminal presentation authority stays
                // alive for an explicit overlay revision. The controller
                // replaces or drops it at the next take; `finish()` is for an
                // owner that truly retires the presentation.
            }
            EngineEvent::Warning { code, message } => {
                tracing::warn!("Engine warning [{}]: {}", code, message);
            }
            EngineEvent::SessionFinalised { .. } => {
                if self
                    .stop_snapshot_pending
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .mark_terminal_lifecycle();
                    if let Some(ledger) = &self.acoustic_ledger {
                        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                        self.mint_light_plus_revision(&mut ledger);
                    }
                    return;
                }
                *self
                    .cursor_unanchored_preview
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = None;
                self.repaint_cursor();
                {
                    let mut phrase = self.phrase_preview_paint.lock()
                        .unwrap_or_else(|error| error.into_inner());
                    phrase.current_rev = None;
                    phrase.current_evidence = None;
                }
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
                    state.clear_ephemeral_preview();
                    state.visible_projection()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                // Lifecycle end closes edit admission independently of the
                // seal verdict. Light+ is idempotent: a document the terminal
                // seal already shaped yields no second intent.
                if let Some(ledger) = &self.acoustic_ledger {
                    let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                    self.mint_light_plus_revision(&mut ledger);
                }
            }
        }
    }
}

/// Authority-bound presentation tests. These are preregistered for C12; C11
/// does not compile or execute them under the W2 embargo.
#[cfg(test)]
mod tests {
    use super::{
        IncrementalShapingRefusal, PresentationEmitter, TerminalFormatterRequest,
        TranscriptReducer, UserRevisionCommit, UserRevisionIntent, UserRevisionRefusal,
    };
    use crate::presentation::transcript_bus::{
        TranscriptBus, TranscriptBusEvidenceEvent, TranscriptDelivery, TranscriptMode,
        TranscriptProjectionPhase, TranscriptSession, TranscriptSessionEndReason,
    };
    use crate::presentation::transcript_projection::TranscriptProjectionReader;
    use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
    use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticLedger, ConsultationPresentationInput,
        DocumentRevisionProvenance, EnergyCalibration, IncrementalShapingReceipt, MutationReceipt,
        ObservationIdentity, ObservationProducer, OccurrenceIdentity, SealRefusal,
    };
    use codescribe_core::pipeline::contracts::{
        AnnotationKind, DeltaSink, EngineEvent, EventSink, LayerSource, LayerSummary, PreviewPin,
        TranscriptDelta,
    };
    use codescribe_core::stt::tail_provider::TailSampleRange;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    /// One L0 preview pinned to an open occurrence of `take`/7.
    fn preview(rev: u64, text: &str) -> EngineEvent {
        EngineEvent::Preview {
            rev,
            text: text.to_string(),
            pin: PreviewPin::open_occurrence(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 16_000,
            }),
        }
    }

    #[derive(Default)]
    struct RecordingDeltaSink {
        deltas: StdMutex<Vec<TranscriptDelta>>,
    }

    #[tokio::test]
    async fn pending_recovery_does_not_hide_new_live_words() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let delivery = Arc::new(Mutex::new(String::new()));
        let emitter = super::PresentationEmitter::new(delivery.clone(), None, None)
            .with_cursor_observer(Arc::new(move |paint| {
                observed.lock().unwrap().push(paint.clone());
            }));
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: codescribe_core::pipeline::contracts::SpeechIntegrity {
                session_id: "take".into(),
                capture_epoch: 7,
                sequence: 1,
                acoustic_speech_ms_since_text_advance: 0,
                pending_occurrences: 1,
                phase: codescribe_core::pipeline::contracts::SpeechIntegrityPhase::Recovering,
            },
        });
        assert_eq!(paints.lock().unwrap().last().unwrap().text, "…");
        emitter.on_event(&preview(1, "nowe słowa na żywo"));
        let paint = paints.lock().unwrap().last().unwrap().clone();
        assert_eq!(paint.text, "nowe słowa na żywo");
        assert!(
            paint.degraded,
            "new words cannot certify the earlier missing speech"
        );
        assert!(delivery.lock().await.is_empty());
    }

    #[tokio::test]
    async fn compact_projection_keeps_capture_identity_and_publication_order() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let emitter =
            super::PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None)
                .with_cursor_observer(Arc::new(move |paint| {
                    observed.lock().unwrap().push(paint.clone());
                }));
        emitter.on_capture_opened("take", 7);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..10 {
                        emitter.paint_cursor("jeden dwa trzy cztery pięć sześć");
                    }
                });
            }
        });
        let paints = paints.lock().unwrap();
        assert_eq!(paints.len(), 81);
        assert!(paints[0].text.is_empty());
        for (index, paint) in paints.iter().enumerate() {
            assert_eq!(paint.sequence, index as u64 + 1);
            assert_eq!(paint.session_id, "take");
            assert_eq!(paint.capture_epoch, 7);
            assert!(paint.text.split_whitespace().count() <= 5);
            let json = serde_json::to_string(paint).unwrap();
            assert_eq!(
                *paint,
                serde_json::from_str::<super::CompactProjection>(&json).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn cursor_projects_five_words_and_acoustic_debt_without_delivery() {
        use codescribe_core::pipeline::contracts::{SpeechIntegrity, SpeechIntegrityPhase};
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "take".into(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                },
                temp.path().join("cursor.jsonl"),
                None,
            )
            .unwrap(),
        );
        let emitter = super::PresentationEmitter::new_with_transcript_bus(
            delivery.clone(),
            None,
            None,
            Some(bus),
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed
                .lock()
                .unwrap()
                .push((projection.text.clone(), projection.degraded));
        }));
        emitter.on_event(&preview(1, "zero jeden dwa trzy cztery pięć"));
        assert!(paints.lock().unwrap().is_empty());
        let mut evidence = SpeechIntegrity {
            session_id: "take".into(),
            capture_epoch: 7,
            sequence: 1,
            acoustic_speech_ms_since_text_advance: 2400,
            pending_occurrences: 1,
            phase: SpeechIntegrityPhase::Stalled,
        };
        let mut foreign_first = evidence.clone();
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: evidence.clone(),
        });
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        emitter.on_capture_opened("take", 0);
        emitter.on_capture_opened("previous take", 7);
        assert!(emitter.cursor_capture.get().is_none());
        emitter.on_capture_opened("take", 7);
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("jeden dwa trzy cztery pięć".into(), false)
        );
        emitter.on_capture_opened("take", 8);
        foreign_first.capture_epoch = 6;
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: foreign_first.clone(),
        });
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        foreign_first.session_id = "previous take".into();
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: foreign_first,
        });
        assert!(!paints.lock().unwrap().last().unwrap().1);
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        for phase in [
            SpeechIntegrityPhase::Stalled,
            SpeechIntegrityPhase::Recovering,
            SpeechIntegrityPhase::Unresolved,
        ] {
            evidence.phase = phase;
            emitter.on_event(&EngineEvent::SpeechIntegrity {
                evidence: evidence.clone(),
            });
            assert_eq!(
                paints.lock().unwrap().last().unwrap(),
                &("jeden dwa trzy cztery pięć".into(), true)
            );
            emitter.on_event(&preview(evidence.sequence + 1, "nowe słowa na żywo"));
            assert_eq!(
                paints.lock().unwrap().last().unwrap(),
                &("nowe słowa na żywo".into(), true)
            );
            emitter.paint_cursor("");
            assert_eq!(paints.lock().unwrap().last().unwrap(), &("…".into(), true));
            emitter.paint_cursor("jeden dwa trzy cztery pięć");
            evidence.sequence += 1;
        }
        let before = paints.lock().unwrap().len();
        for (epoch, sequence) in [(7, evidence.sequence - 1), (8, u64::MAX)] {
            let mut stale = evidence.clone();
            stale.capture_epoch = epoch;
            stale.sequence = sequence;
            stale.phase = SpeechIntegrityPhase::Tracking;
            emitter.on_event(&EngineEvent::SpeechIntegrity { evidence: stale });
            assert_eq!(paints.lock().unwrap().len(), before);
            assert!(paints.lock().unwrap().last().unwrap().1);
        }
        let mut stale = evidence.clone();
        stale.session_id = "old take".into();
        emitter.on_event(&EngineEvent::SpeechIntegrity { evidence: stale });
        assert_eq!(paints.lock().unwrap().len(), before);
        // A canonical repair paints while amber. Clearing the warning must
        // reveal that repair, not resurrect the earlier provisional preview.
        emitter.paint_cursor("odzyskany cały fragment");
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("odzyskany cały fragment".into(), true)
        );
        evidence.phase = SpeechIntegrityPhase::Tracking;
        emitter.on_event(&EngineEvent::SpeechIntegrity { evidence });
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("odzyskany cały fragment".into(), false)
        );
        assert!(delivery.lock().await.is_empty());
    }

    impl DeltaSink for RecordingDeltaSink {
        fn apply(&self, delta: &TranscriptDelta) {
            self.deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(delta.clone());
        }
    }

    /// Qualify one occurrence through the ledger's calibrated energy predicate,
    /// then admit an Apple observation over exactly that span.
    ///
    /// Both steps are load-bearing. `admit` alone records a layer decision whose
    /// serial list is copied from `evidence`, so an occurrence that never
    /// cleared `qualify` produces a decision that is not evidence-backed — and
    /// the reducer must refuse it. Authenticating here is what makes the
    /// assertions downstream about repetition and revision projection, rather
    /// than about the admission gate itself.
    fn admitted_mutation(
        ledger: &mut AcousticLedger,
        occurrence: OccurrenceIdentity,
        request: u64,
        label: &str,
    ) -> EngineEvent {
        let calibration = EnergyCalibration {
            version: "emitter-test".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, request, 0, occurrence);
        let receipt = ledger.admit(&observation, label);
        EngineEvent::LedgerMutation {
            observation,
            label: label.to_string(),
            receipt,
        }
    }

    #[test]
    fn group_answer_preserves_later_speech_and_authenticates_bus_publication() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let mut members = Vec::new();
        for (index, label) in ["iwo", "iwo", "dalsze słowa"].into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(
                "group-live",
                1,
                index as u64 * 16_000,
                (index as u64 + 1) * 16_000,
            );
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = admitted_mutation(&mut ledger, occurrence.clone(), index as u64, label)
            else {
                unreachable!()
            };
            reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .unwrap();
            if index < 2 {
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
                let seal = ledger.seal(&occurrence).unwrap().clone();
                reducer.apply_ledger_seal(&seal).unwrap();
                members.push(ConsultationPresentationMember {
                    occurrence: occurrence.clone(),
                    source_label: label.into(),
                    seal_receipt: seal.receipt_id,
                });
                let _ = reducer.apply_incremental_shaping(&mut ledger, &occurrence);
            }
        }
        let revision = reducer
            .apply_consultation_presentation(
                &mut ledger,
                ConsultationPresentationInput {
                    consultation_id: "Max",
                    turn_id: "first",
                    source_revision: 0,
                    revision: 1,
                    members: &members,
                    rendered_text: "Gotowe polecenie",
                },
            )
            .unwrap();
        assert_eq!(revision.rendered_text, "Gotowe polecenie dalsze słowa");
        assert_eq!(revision.entries.len(), 3);
        assert!(revision.authenticates_publication(&ledger, "group-live"));
        assert!(
            revision.entries[..2]
                .iter()
                .all(|entry| entry.presentation_receipt.is_none())
        );
        assert_eq!(ledger.text_of(&members[0].occurrence), Some("iwo"));
        assert_eq!(ledger.text_of(&members[1].occurrence), Some("iwo"));
        assert!(!reducer.terminal);

        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "group-live".into(),
                mode: TranscriptMode::Dictation,
                has_latched_target: true,
                latched_target_is_self: false,
            },
            temp.path().join("groups.jsonl"),
            None,
        )
        .unwrap();
        let mut forged = revision.clone();
        forged.consultation_presentations[0].rendered_text = "forged".into();
        forged.publication_digest = forged.digest();
        assert!(bus.publish_revision(&forged, &ledger).is_empty());
        let events = bus.publish_revision(&revision, &ledger);
        assert_eq!(events.len(), 3);
        for event in &events {
            assert_eq!(event.reducer_action, "apply_consultation_presentation");
            assert_eq!(event.rendered_text, revision.rendered_text);
            assert_eq!(event.consultation_presentations[0].members.len(), 2);
            assert_eq!(event.consultation_presentations[0].turn_id, "first");
            assert!(!event.terminal && !event.lifecycle_terminal);
            let encoded = serde_json::to_string(event).unwrap();
            assert_eq!(
                serde_json::from_str::<TranscriptBusEvidenceEvent>(&encoded).unwrap(),
                *event
            );
        }
        assert!(bus.publish_revision(&revision, &ledger).is_empty());
        assert!(
            reducer
                .apply_consultation_presentation(
                    &mut ledger,
                    ConsultationPresentationInput {
                        consultation_id: "Max",
                        turn_id: "first",
                        source_revision: 0,
                        revision: 1,
                        members: &members,
                        rendered_text: "duplicate",
                    }
                )
                .is_err()
        );
        assert_eq!(ledger.consultation_presentations().len(), 1);

        let later = OccurrenceIdentity::new("group-live", 1, 48_000, 64_000);
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted_mutation(&mut ledger, later, 4, "jutro")
        else {
            unreachable!()
        };
        let suffix = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();
        assert_eq!(suffix.rendered_text, "Gotowe polecenie dalsze słowa jutro");
        assert!(suffix.authenticates_publication(&ledger, "group-live"));
        assert_eq!(bus.publish_revision(&suffix, &ledger).len(), 4);
        assert_eq!(ledger.len(), 4);
    }

    fn raw_final(text: &str) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn preview_paints_overlay_without_writing_delivery() {
        let delivery = Arc::new(Mutex::new("ledger truth".to_string()));
        let deltas = Arc::new(RecordingDeltaSink::default());
        let mut emitter =
            PresentationEmitter::new(Arc::clone(&delivery), Some(deltas.clone()), None);

        emitter.on_event(&preview(1, "volatile words"));
        assert!(emitter.wait_paint_published().await);

        assert_eq!(delivery.lock().await.as_str(), "ledger truth");
        assert!(
            !deltas
                .deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        emitter.finish().await;
    }

    /// App-side drain proof for the armed worker's pre-finish event sequence.
    /// Worker ordering is pinned in the core tests; here authentic mutation and
    /// seal events must reach the reducer before the acknowledgement is read.
    #[tokio::test]
    async fn last_window_ack_observes_committed_sealed_partial_before_finish_event() {
        let mut take = live_take("stop-ack");
        take.emitter.on_capture_opened("stop-ack", 7);
        let occurrence = OccurrenceIdentity::new("stop-ack", 7, 0, 16_000);
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let finish_events = AtomicUsize::new(0);
        let producer = async {
            take.admit(&occurrence, 1, "Iwo at stop");
            take.seal(&occurrence);
            ack_tx.send(()).unwrap();
            finish_rx.await.unwrap();
            finish_events.fetch_add(1, Ordering::SeqCst);
            take.emitter.on_event(&raw_final("Iwo after finish"));
        };
        let consumer = async {
            ack_rx.await.unwrap();
            assert_eq!(finish_events.load(Ordering::SeqCst), 0);
            {
                let reducer = take.emitter.session_state.lock().unwrap();
                assert!(reducer.committed_rendered_text().contains("Iwo at stop"));
                let entry = reducer.document_by_occurrence.get(&occurrence).unwrap();
                assert_eq!(entry.label, "Iwo at stop");
                assert!(entry.seal_receipt.is_some());
            }
            let frozen = take.emitter.visible_canvas_snapshot().unwrap();
            assert!(frozen.has_committed_document);
            assert_eq!(frozen.preview_only_words, 0);
            assert_eq!(frozen.qualified_occurrences, 1);
            finish_tx.send(()).unwrap();
        };
        tokio::join!(producer, consumer);
        assert_eq!(finish_events.load(Ordering::SeqCst), 1);
        take.emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_canvas_keeps_preview_only_words_raw_without_a_user_revision() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&preview(1, "one two three four five"));
        let frozen = emitter.visible_canvas_snapshot().expect("opened take");
        assert_eq!(frozen.session_id, "take");
        assert_eq!(frozen.capture_epoch, 7);
        assert_eq!(frozen.revision, 0);
        assert_eq!(frozen.text, "one two three four five");
        assert_eq!(frozen.preview_only_words, 5);
        assert!(!frozen.has_committed_document);
        assert_eq!(frozen.qualified_occurrences, 0);
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
        assert!(delivery.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stop_canvas_with_committed_and_preview_words_skips_light_plus() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "committed words",
        );
        emitter.on_event(&mutation);
        let committed = emitter.visible_canvas_snapshot().unwrap();
        emitter.on_event(&preview(2, "preview words"));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.text, format!("{} preview words", committed.text));
        assert_eq!(frozen.preview_only_words, 2);
        assert!(frozen.has_committed_document);
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), committed.text);
    }

    #[tokio::test]
    async fn stop_canvas_counts_only_visible_unanchored_words_as_preview() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            delivery,
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "committed words",
        );
        emitter.on_event(&mutation);
        for (request, start, end, label) in [
            (2, 0, 8_000, "covered evidence"),
            (3, 16_000, 32_000, "visible evidence"),
        ] {
            let observation = ObservationIdentity::new(
                ObservationProducer::Apple,
                request,
                0,
                OccurrenceIdentity::new("take", 7, start, end),
            );
            let receipt = ledger.lock().unwrap().keep_visible_unanchored(
                &observation,
                label,
                codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
            );
            emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: label.to_string(),
                receipt,
            });
        }
        emitter.on_event(&preview(4, "last preview"));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 4);
        assert!(frozen.text.ends_with("visible evidence last preview"));
        assert!(!frozen.text.contains("covered evidence"));
        assert!(emitter.session_state.lock().unwrap().unanchored_evidence("take", 7)
            .iter().any(|item| item.text == "covered evidence"));
        let accounted = frozen.missing_words_from(&frozen);
        assert_eq!(accounted.len(), 2);
        assert!(accounted.iter().all(|word| word.reason.starts_with("covered_by_committed occurrence=")));
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_canvas_speech_count_is_capture_filtered_without_document_text() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        for (session, epoch) in [("take", 7), ("take", 8), ("another-take", 7)] {
            let _ = admitted_mutation(
                &mut ledger.lock().unwrap(),
                OccurrenceIdentity::new(session, epoch, 0, 16_000),
                1,
                "speech",
            );
        }
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert!(frozen.text.is_empty());
        assert!(!frozen.has_committed_document);
        assert_eq!(frozen.qualified_occurrences, 1);
        assert_eq!(ledger.lock().unwrap().qualified_occurrences().count(), 3);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn frozen_stop_canvas_publishes_the_exact_light_plus_paste() {
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("paste.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "paste-take".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let occurrence = OccurrenceIdentity::new("paste-take", 7, 0, 16_000);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            occurrence,
            1,
            "to działa bo jest proste",
        );
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("paste-take", 7);
        emitter.on_event(&mutation);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        let paste = emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap();
        assert_eq!(
            paste.text,
            codescribe_core::pipeline::light_plus::apply(&frozen.text)
        );
        assert!(paste.revision > frozen.revision);
        assert_eq!(
            ledger.lock().unwrap().manual_document_revisions()[0].provenance,
            "light-plus"
        );
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), paste.text);
        let bus_text = std::fs::read_to_string(bus_path).unwrap();
        assert!(bus_text.contains("light-plus"));
        assert!(bus_text.contains(&paste.text));
    }

    #[tokio::test]
    async fn late_mutation_after_frozen_paste_remains_a_bus_revision() {
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("late.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "late-paste".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("late-paste", 7);
        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("late-paste", 7, 0, 16_000),
            1,
            "pierwsze słowa",
        );
        emitter.on_event(&first);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        let paste = emitter.shape_frozen_canvas_at_stop(frozen).unwrap();
        assert_eq!(paste.text, "Pierwsze słowa.");

        let late = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("late-paste", 7, 16_000, 32_000),
            2,
            "drugie słowa",
        );
        emitter.on_event(&late);
        let revised = emitter.visible_canvas_snapshot().unwrap();
        assert!(revised.revision > paste.revision);
        assert_eq!(revised.text, "Pierwsze słowa drugie słowa.");
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), revised.text);
        let bus_text = std::fs::read_to_string(bus_path).unwrap();
        assert!(bus_text.contains(&paste.text));
        assert!(bus_text.contains(&revised.text));
    }

    #[tokio::test]
    async fn frozen_stop_canvas_remains_a_prefix_of_five_occurrence_revision() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        for index in 0..5 {
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                admitted_mutation(
                    &mut ledger,
                    OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                    index + 1,
                    "Iwo",
                )
            };
            emitter.on_event(&mutation);
            if index == 0 {
                let frozen = emitter.visible_canvas_snapshot().unwrap();
                assert_eq!(frozen.revision, 1);
                assert_eq!(frozen.text, "Iwo");
            }
        }
        let revised = emitter.visible_canvas_snapshot().unwrap();
        emitter.finish().await;
        assert_eq!(revised.text, "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(revised.revision, 5);
        assert_eq!(revised.qualified_occurrences, 5);
        assert_eq!(delivery.lock().await.as_str(), revised.text);
        assert_eq!(ledger.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn stop_canvas_includes_the_third_occurrence_open_at_release() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        for index in 0..2 {
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                admitted_mutation(
                    &mut ledger,
                    OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                    index + 1,
                    "Iwo",
                )
            };
            emitter.on_event(&mutation);
        }
        emitter.on_event(&preview(3, "Iwo"));
        // Stop paste v2 (R3): the open third word is overlay-visible at stop,
        // but only as preview, without occurrence authority.
        let open = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(open.text, "Iwo Iwo Iwo");
        assert_eq!(open.preview_only_words, 1);
        let last = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("take", 7, 32_000, 48_000),
                3,
                "Iwo",
            )
        };
        emitter.on_event(&last);
        emitter.on_event(&preview_disposition(3, PreviewFinalDisposition::Admitted));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 0);
        assert_eq!(
            frozen.text, "Iwo Iwo Iwo",
            "last occurrence missing at stop"
        );
        assert_eq!(frozen.revision, 3);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn single_occurrence_closed_at_release_is_the_first_deliverable_canvas() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&preview(1, "last words"));
        // Stop paste v2 (R3): preview-only words are visible at stop, with no
        // committed document behind them.
        let open = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(open.text, "last words");
        assert_eq!(open.preview_only_words, 2);
        assert!(!open.has_committed_document);
        let last = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("take", 7, 0, 16_000),
                1,
                "last words",
            )
        };
        emitter.on_event(&last);
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 0);
        assert_eq!(frozen.text, "Last words");
        assert_eq!(frozen.revision, 2);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_snapshot_equals_the_last_overlay_paint() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let mut paint_count = 0;
        let mut assert_paint = |expected_text: &str, expected_preview_words: usize| {
            let frozen = emitter.visible_canvas_snapshot().unwrap();
            let paints = emitter.paint_commands.lock().unwrap();
            assert!(paints.len() > paint_count, "event must dispatch a paint");
            paint_count = paints.len();
            assert_eq!(&frozen.text, paints.last().unwrap());
            assert_eq!(frozen.text, expected_text);
            assert_eq!(frozen.preview_only_words, expected_preview_words);
        };

        // Longer than the compact cursor's five-word tail: compare full paints.
        emitter.on_event(&preview(1, "One two three four five six"));
        assert_paint("One two three four five six", 6);

        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "One two three four five six",
        );
        emitter.on_event(&first);
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        assert_paint("One two three four five six", 0);

        emitter.on_event(&preview(2, "open tail"));
        assert_paint("One two three four five six open tail", 2);

        let tail = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, tail.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "unanchored words",
            codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "unanchored words".into(),
            receipt,
        });
        emitter.on_event(&preview_disposition(2, PreviewFinalDisposition::KeptUnanchored));
        assert_paint("One two three four five six unanchored words", 2);

        let last = admitted_mutation(&mut ledger.lock().unwrap(), tail, 3, "settled tail");
        emitter.on_event(&last);
        assert_paint("One two three four five six settled tail", 0);
        emitter.finish().await;
        assert_eq!(
            delivery.lock().await.as_str(),
            "One two three four five six settled tail"
        );
    }

    #[tokio::test]
    async fn mutation_without_paint_keeps_the_preview_in_the_stop_snapshot() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let first = admitted_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), 1, "Iwo");
        emitter.on_event(&first);
        let seal = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            ledger.seal(&occurrence).unwrap().clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "next words".into(),
            pin: PreviewPin::open_occurrence(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 16_000,
                sample_end: 32_000,
            }),
        });
        let before = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(before.text, "Iwo next words");
        assert_eq!(before.preview_only_words, 2);
        let paint_count = emitter.paint_commands.lock().unwrap().len();

        // A late machine observation over a sealed occurrence has no mutation
        // authority. This is a ledger-issued refusal, not injected reducer state.
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence);
        let receipt = ledger.lock().unwrap().admit(&observation, "changed words");
        assert!(!receipt.grants_mutation());
        assert!(!matches!(
            receipt,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "changed words".into(),
            receipt,
        });
        assert_eq!(emitter.paint_commands.lock().unwrap().len(), paint_count);
        let after = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(after, before);
        assert_eq!(
            &after.text,
            emitter.paint_commands.lock().unwrap().last().unwrap()
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_snapshot_zero_width_paint_includes_unanchored_words() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "Iwo",
        );
        emitter.on_event(&first);
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            2,
            0,
            OccurrenceIdentity::new("take", 7, 16_000, 32_000),
        );
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "unanchored words",
            codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "unanchored words".into(),
            receipt,
        });
        assert_eq!(
            emitter
                .visible_canvas_snapshot()
                .unwrap()
                .preview_only_words,
            2
        );
        emitter.on_event(&EngineEvent::Preview {
            rev: 3,
            text: "side evidence".into(),
            pin: PreviewPin::from_segments(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 32_000,
                sample_end: 32_000,
            }),
        });
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(
            &frozen.text,
            emitter.paint_commands.lock().unwrap().last().unwrap()
        );
        assert_eq!(frozen.text, "Iwo unanchored words side evidence");
        assert_eq!(frozen.preview_only_words, 4);
        // A committed paint at EOF must still record the visible evidence.
        emitter.send_committed_paint("Iwo".into());
        assert_eq!(emitter.visible_canvas_snapshot().unwrap(), frozen);
        emitter.finish().await;
    }

    fn zero_width_preview(rev: u64, text: &str) -> EngineEvent {
        EngineEvent::Preview {
            rev,
            text: text.to_string(),
            pin: PreviewPin::from_segments(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: rev * 16_000,
                sample_end: rev * 16_000,
            }),
        }
    }

    fn preview_disposition(rev: u64, disposition: super::PreviewFinalDisposition) -> EngineEvent {
        EngineEvent::PreviewDisposition {
            superseded_through_rev: rev,
            final_disposition: disposition,
            refused_evidence: Vec::new(),
        }
    }

    fn refused_final(rev: u64, text: &str) -> EngineEvent {
        EngineEvent::PreviewDisposition {
            superseded_through_rev: rev,
            final_disposition: PreviewFinalDisposition::Refused { reason: "untimed".into() },
            refused_evidence: vec![codescribe_core::pipeline::contracts::RefusedPreviewEvidence {
                range: None, text: text.into(), reason: "untimed".into(),
            }],
        }
    }

    #[tokio::test]
    async fn admitted_phrase_clears_only_its_zero_width_preview() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&zero_width_preview(1, "Iwo Iwo"));
        let before = emitter.begin_stop_canvas().unwrap();
        let admitted = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000), 1, "Iwo");
        emitter.on_event(&admitted);
        emitter.on_event(&preview_disposition(1, super::PreviewFinalDisposition::Admitted));
        let after = emitter.finish_stop_canvas().unwrap();
        assert_eq!(after.text, "Iwo");
        assert_eq!(after.preview_only_words, 0);
        let missing = before.missing_words_from(&after);
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].word, "Iwo");
        assert!(missing.iter().all(|word| word.reason == "superseded_by_final rev=1"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn shortened_refused_and_pending_phrases_have_zero_unaccounted_words() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&zero_width_preview(1, "refused words"));
        emitter.on_event(&refused_final(1, "refused words"));
        emitter.on_event(&zero_width_preview(2, "Iwo Iwo"));
        let at_stop = emitter.begin_stop_canvas().unwrap();
        assert!(at_stop.text.contains("refused words"));
        let admitted = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000), 1, "Iwo");
        emitter.on_event(&admitted);
        emitter.on_event(&preview_disposition(2, super::PreviewFinalDisposition::Admitted));
        emitter.on_event(&zero_width_preview(3, "pending words"));
        let deadline = emitter.finish_stop_canvas().unwrap();
        assert_eq!(deadline.text, "Iwo refused words pending words");
        let missing = at_stop.missing_words_from(&deadline);
        assert_eq!(missing.len(), 2);
        assert!(missing.iter().all(|word| word.reason == "superseded_by_final rev=2"));
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        // The earlier phrase keeps its own final receipt.
        assert_eq!(deadline.preview_supersessions.get(&1), Some(&super::PreviewSupersession::Final(1)));
        assert!(!deadline.preview_supersessions.contains_key(&3));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn partial_shortening_after_stop_replaces_stopped_revision_once() {
        for zero_width in [false, true] {
            let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
            let mut emitter = PresentationEmitter::new_with_authority(
                Arc::new(Mutex::new(String::new())), None, None, None,
                Some(Arc::clone(&ledger)), None,
            );
            emitter.set_literal_delivery(true);
            emitter.on_capture_opened("take", 7);
            emitter.on_event(&zero_width_preview(1, "refused earlier"));
            emitter.on_event(&refused_final(1, "refused earlier"));
            let partial = if zero_width { zero_width_preview } else { preview };
            emitter.on_event(&partial(2, "one two three"));
            let stopped = emitter.begin_stop_canvas().unwrap();
            emitter.on_event(&partial(3, "one"));
            let deadline = emitter.visible_canvas_snapshot().unwrap();
            assert_eq!(deadline.text, "refused earlier one");
            let missing = stopped.missing_words_from(&deadline);
            assert_eq!(missing.len(), 3);
            assert!(missing.iter().all(|word| word.reason == "superseded_by_partial rev=3"));
            emitter.on_event(&partial(4, "one"));
            assert_eq!(emitter.visible_canvas_snapshot().unwrap().text, deadline.text);
            let admitted = admitted_mutation(&mut ledger.lock().unwrap(),
                OccurrenceIdentity::new("take", 7, 0, 16_000), 1, "one");
            emitter.on_event(&admitted);
            emitter.on_event(&preview_disposition(4, super::PreviewFinalDisposition::Admitted));
            let settled = emitter.finish_stop_canvas().unwrap();
            assert_eq!(settled.text, "one refused earlier");
            let missing = stopped.missing_words_from(&settled);
            assert_eq!(missing.len(), 3);
            assert!(missing.iter().all(|word| word.reason == "superseded_by_partial rev=3"));
            assert_eq!(settled.preview_supersessions.get(&1), Some(&super::PreviewSupersession::Final(1)));
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn kept_unanchored_phrase_replaces_preview_without_losing_evidence() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&zero_width_preview(1, "unanchored words"));
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0,
            OccurrenceIdentity::new("take", 7, 0, 16_000));
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(&observation,
            "unanchored words", super::NoAuthorityReason::NoRange);
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation, label: "unanchored words".to_string(), receipt,
        });
        emitter.on_event(&preview_disposition(1, super::PreviewFinalDisposition::KeptUnanchored));
        let snapshot = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(snapshot.text, "unanchored words");
        assert_eq!(snapshot.preview_only_words, 2);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_word_accounting_does_not_borrow_identical_words_from_another_phrase() {
        let mut emitter = PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None);
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&zero_width_preview(1, "Iwo Iwo"));
        let before = emitter.begin_stop_canvas().unwrap();
        let mut pasted = before.clone();
        pasted.visible_words[1].source = super::VisibleWordSource::Preview(99);
        let missing = before.missing_words_from(&pasted);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].word, "Iwo");
        assert_eq!(missing[0].reason, "unaccounted");
        emitter.finish().await;
    }

    #[tokio::test]
    async fn pending_latest_partial_survives_an_unrelated_committed_paint() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&preview(5, "pending old suffix"));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&preview(6, "latest"));
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000), 1, "earlier");
        emitter.on_event(&mutation);
        let deadline = emitter.finish_stop_canvas().unwrap();
        assert_eq!(deadline.text, "earlier latest");
        assert_eq!(deadline.preview_only_words, 1);
        assert!(stopped.missing_words_from(&deadline).iter()
            .all(|word| word.reason == "superseded_by_partial rev=6"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn relabel_in_place_during_the_wait_is_accounted_not_a_defect() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), 1, "before");
        emitter.on_event(&mutation);
        let stopped = emitter.begin_stop_canvas().unwrap();
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence.clone());
        let receipt = ledger.lock().unwrap().admit(&observation, "after");
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation { observation, label: "after".into(), receipt });
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "after");
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].reason, format!("relabeled_in_place occurrence={occurrence:?}"));
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_wait_accounts_for_shaping_relabel_and_untouched_occurrences() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        // Hold presentation until the seal arrives during the STOP wait.
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let occurrences = (0..3).map(|index|
            OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000)
        ).collect::<Vec<_>>();
        for (index, label) in ["first phrase", "old label words", "untouched words"].iter().enumerate() {
            let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
                occurrences[index].clone(), index as u64 + 1, label);
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.set_literal_delivery(false);
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrences[0].clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrences[0], ObservationProducer::Apple));
            ledger.seal(&occurrences[0]).unwrap().clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
        let observation = ObservationIdentity::new(
            ObservationProducer::Whisper, 4, 0, occurrences[1].clone(),
        );
        let receipt = ledger.lock().unwrap().admit(&observation, "new label");
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation { observation, label: "new label".into(), receipt });
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "First phrase new label untouched words");
        assert_eq!(frozen.text, *emitter.paint_commands.lock().unwrap().last().unwrap());
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 5);
        assert_eq!(missing.iter().filter(|word|
            word.reason == format!("reshaped_in_place occurrence={:?}", occurrences[0])).count(), 2);
        assert_eq!(missing.iter().filter(|word|
            word.reason == format!("relabeled_in_place occurrence={:?}", occurrences[1])).count(), 3);
        assert_eq!(stopped.visible_words.len() - missing.len(), 2);
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn removed_committed_occurrence_without_successor_is_unaccounted() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let removed = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let survivor = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        for (index, occurrence) in [&removed, &survivor].iter().enumerate() {
            let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
                (*occurrence).clone(), index as u64 + 1, "same words");
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        // Fault injection: lose a reducer entry while identical words survive
        // at a disjoint PCM range. Text must not conceal the loss.
        emitter.session_state.lock().unwrap().document_by_occurrence.remove(&removed);
        emitter.send_committed_paint("same words".into());
        let frozen = emitter.finish_stop_canvas().unwrap();
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 2);
        assert!(missing.iter().all(|word| word.reason == "unaccounted"));

        // A different committed owner may account for that range only when
        // its PCM actually contains it on the same capture clock.
        let owner = OccurrenceIdentity::new("take", 7, 0, 32_000);
        {
            let mut state = emitter.session_state.lock().unwrap();
            let mut entry = state.document_by_occurrence.remove(&survivor).unwrap();
            entry.occurrence = owner.clone();
            state.document_by_occurrence.insert(owner.clone(), entry);
        }
        emitter.send_committed_paint("same words".into());
        let covered = emitter.visible_canvas_snapshot().unwrap();
        let accounted = stopped.missing_words_from(&covered);
        assert_eq!(accounted.len(), 4);
        assert!(accounted.iter().all(|word|
            word.reason == format!("covered_by_committed occurrence={owner:?}")));
        let mut foreign = covered.clone();
        foreign.capture_epoch += 1;
        assert!(stopped.missing_words_from(&foreign).iter().all(|word| word.reason == "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn document_revision_accounts_for_member_occurrences() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let first = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let second = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        for (index, occurrence) in [&first, &second].iter().enumerate() {
            let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
                (*occurrence).clone(), index as u64 + 1, "old words");
            emitter.on_event(&mutation);
        }
        let plain = emitter.visible_canvas_snapshot().unwrap();
        emitter.on_event(&EngineEvent::ContextMarker { position: 0, label: "{selection_1}".into() });
        let stopped = emitter.begin_stop_canvas().unwrap();
        assert!(plain.missing_words_from(&stopped).is_empty());
        let observation = ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, first.clone());
        let receipt = ledger.lock().unwrap().admit(&observation, "new label");
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation { observation, label: "new label".into(), receipt });
        let relabeled = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(relabeled.text, "{selection_1} new label old words");
        assert!(stopped.missing_words_from(&relabeled).iter().all(|word|
            word.reason == format!("relabeled_in_place occurrence={first:?}")));

        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "take".into(), layer_summary: LayerSummary::default(),
        });
        emitter.apply_user_revision(UserRevisionIntent {
            session_id: "take".into(), source_revision: relabeled.revision,
            rendered_text: "Revised document.".into(),
            provenance: DocumentRevisionProvenance::UserEdit,
        }).unwrap();
        let revised = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(revised.committed_sources.len(), 2);
        let reshaped = relabeled.missing_words_from(&revised);
        assert_eq!(reshaped.len(), relabeled.visible_words.len());
        assert!(reshaped.iter().all(|word|
            word.reason.contains(&format!("reshaped_in_place occurrence={first:?}"))
                && word.reason.contains(&format!("reshaped_in_place occurrence={second:?}"))));
        assert!(revised.missing_words_from(&revised).is_empty());

        emitter.apply_user_revision(UserRevisionIntent {
            session_id: "take".into(), source_revision: revised.revision,
            rendered_text: "Another document revision.".into(),
            provenance: DocumentRevisionProvenance::UserEdit,
        }).unwrap();
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "Another document revision.");
        assert!(revised.missing_words_from(&frozen).iter().all(|word|
            word.reason.contains(&format!("reshaped_in_place occurrence={first:?}"))
                && word.reason.contains(&format!("reshaped_in_place occurrence={second:?}"))));

        // Joint document provenance is not permission to lose a member.
        let mut removed = frozen.clone();
        removed.committed_sources.remove(&second);
        assert!(revised.missing_words_from(&removed).iter().all(|word| word.reason == "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn frozen_shaping_accounts_for_every_committed_member() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        for index in 0..2 {
            let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
                OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                index + 1, "some words");
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.set_literal_delivery(false);
        let frozen = emitter.shape_frozen_canvas_at_stop(stopped.clone()).unwrap();
        assert_eq!(frozen.text, "Some words some words.");
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 4);
        for occurrence in stopped.committed_sources.keys() {
            assert_eq!(missing.iter().filter(|word|
                word.reason == format!("reshaped_in_place occurrence={occurrence:?}")).count(), 2);
        }
        let painted = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.visible_words, painted.visible_words);
        assert_eq!(frozen.committed_sources, painted.committed_sources);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_wait_accounts_for_all_consultation_members() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let mut members = Vec::new();
        for index in 0..2 {
            let occurrence = OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000);
            let mutation = admitted_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), index + 1, "old words");
            emitter.on_event(&mutation);
            let seal = {
                let mut ledger = ledger.lock().unwrap();
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                ledger.seal(&occurrence).unwrap().clone()
            };
            emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal.clone() });
            members.push(ConsultationPresentationMember {
                occurrence, source_label: "old words".into(), seal_receipt: seal.receipt_id,
            });
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        let revision = {
            let mut ledger = ledger.lock().unwrap();
            emitter.session_state.lock().unwrap().apply_consultation_presentation(
                &mut ledger,
                ConsultationPresentationInput {
                    consultation_id: "Max", turn_id: "stop-wait", source_revision: 0, revision: 1,
                    members: &members, rendered_text: "Combined answer",
                },
            ).unwrap()
        };
        emitter.publish_revision(revision);
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "Combined answer");
        assert_eq!(frozen.committed_sources.len(), 2);
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 4);
        for member in &members {
            assert_eq!(missing.iter().filter(|word|
                word.reason == format!("reshaped_in_place occurrence={:?}", member.occurrence)).count(), 2);
        }
        {
            let mut state = emitter.session_state.lock().unwrap();
            state.document_by_occurrence.remove(&members[1].occurrence);
            state.invalidate_stale_shapes();
        }
        emitter.send_committed_paint("old words".into());
        let removed = emitter.visible_canvas_snapshot().unwrap();
        let lost = frozen.missing_words_from(&removed);
        assert_eq!(lost.len(), 2);
        assert!(lost.iter().all(|word| word.reason == "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn whole_refusal_pastes_final_text_once_and_supersedes_preview() {
        let mut emitter = PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None);
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&zero_width_preview(1, "old preview words"));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&refused_final(1, "final evidence"));
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "final evidence");
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 3);
        assert!(missing.iter().all(|word| word.reason == "superseded_by_final rev=1"));
        let phrase = emitter.phrase_preview_paint.lock().unwrap();
        assert!(phrase.current_rev.is_none());
        assert_eq!(phrase.retained[0].sample_start, 16_000);
        drop(phrase);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn combined_stop_visible_accounting() {
        use codescribe_core::pipeline::contracts::RefusedPreviewEvidence;
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let projections = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&projections);
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())), None, None, None,
            Some(Arc::clone(&ledger)), None,
        ).with_cursor_observer(Arc::new(move |projection| observed.lock().unwrap().push(projection.clone())));
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        let owner = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(), owner.clone(), 1, "kept");
        emitter.on_event(&mutation);
        let evidence = ObservationIdentity::new(ObservationProducer::Apple, 2, 0,
            OccurrenceIdentity::new("take", 7, 0, 8_000));
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(&evidence, "covered hypothesis",
            codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange);
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation: evidence, label: "covered hypothesis".into(), receipt,
        });

        // Preserve is an already-visible final, never a retained preview.
        emitter.on_event(&zero_width_preview(1, "old kept preview"));
        let before_preserve = emitter.visible_canvas_snapshot().unwrap();
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 3, 0, owner);
        let receipt = ledger.lock().unwrap().admit(&observation, "kept");
        assert!(matches!(receipt, MutationReceipt::Preserve { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation { observation, label: "kept".into(), receipt });
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        let after_preserve = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(after_preserve.text, "kept");
        assert!(before_preserve.missing_words_from(&after_preserve).iter().all(|word|
            word.reason == "superseded_by_final rev=1" || word.reason.starts_with("covered_by_committed")));

        // The admitted part and refused part both survive, once, in PCM order.
        emitter.on_event(&zero_width_preview(2, "partly guessed preview"));
        let before_refusal = emitter.visible_canvas_snapshot().unwrap();
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 16_000, 32_000), 4, "admitted");
        emitter.on_event(&mutation);
        emitter.on_event(&EngineEvent::PreviewDisposition {
            superseded_through_rev: 2,
            final_disposition: PreviewFinalDisposition::Refused { reason: "slice_refused".into() },
            refused_evidence: vec![RefusedPreviewEvidence {
                range: Some(OccurrenceIdentity::new("take", 7, 32_000, 48_000)),
                text: "refused label".into(), reason: "slice_refused".into(),
            }],
        });
        let after_refusal = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(after_refusal.text, "kept admitted refused label");
        assert!(before_refusal.missing_words_from(&after_refusal).iter().all(|word|
            word.reason == "superseded_by_final rev=2" || word.reason.starts_with("covered_by_committed")));

        emitter.on_event(&zero_width_preview(5, "newest discarded suffix"));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&zero_width_preview(6, "newest"));
        let at_bound = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(at_bound.text, "kept admitted refused label newest");
        let missing = stopped.missing_words_from(&at_bound);
        assert_eq!(missing.iter().filter(|word| word.reason == "superseded_by_partial rev=6").count(), 3);
        assert_eq!(missing.iter().filter(|word| word.reason.starts_with("covered_by_committed occurrence=")).count(), 2);
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 48_000, 64_000), 5, "newest");
        emitter.on_event(&mutation);
        emitter.on_event(&preview_disposition(6, PreviewFinalDisposition::Admitted));
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, at_bound.text);
        assert_eq!(stopped.missing_words_from(&frozen), missing);
        assert!(!frozen.text.contains("covered hypothesis"));
        let projections = projections.lock().unwrap();
        let last = projections.last().unwrap();
        assert!(last.evidence.iter().any(|item| item.text == "covered hypothesis"));
        assert!(last.evidence.iter().any(|item| item.text == "refused label"));
        assert!(!last.evidence.iter().any(|item| item.text == "newest discarded suffix"));
        drop(projections);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn retired_presentation_still_updates_its_own_delivery_buffer() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let delivery = Arc::new(Mutex::new(String::new()));
        let deltas = Arc::new(RecordingDeltaSink::default());
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery), Some(deltas.clone()), None, None,
            Some(Arc::clone(&ledger)), None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        emitter.retire_presentation();
        let admitted = admitted_mutation(&mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000), 1, "late words");
        emitter.on_event(&admitted);
        emitter.finish().await;
        assert!(deltas.deltas.lock().unwrap().is_empty());
        assert_eq!(*delivery.lock().await, "late words");
    }

    #[tokio::test]
    async fn ledger_mutation_paints_overlay_and_writes_exact_revision_to_delivery() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let deltas = Arc::new(RecordingDeltaSink::default());
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("ledger.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let projection_count = Arc::new(AtomicUsize::new(0));
        let projection_count_for_callback = Arc::clone(&projection_count);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mutation = {
            let mut guard = ledger.lock().unwrap_or_else(|error| error.into_inner());
            admitted_mutation(
                &mut guard,
                OccurrenceIdentity::new("session", 3, 0, 16_000),
                1,
                "Iwo",
            )
        };
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            Some(deltas.clone()),
            None,
            Some(Arc::clone(&bus)),
            Some(ledger),
            Some(Arc::new(move |_| {
                projection_count_for_callback.fetch_add(1, Ordering::SeqCst);
            })),
        );

        emitter.on_event(&mutation);
        emitter.finish().await;
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("committed book must produce a terminal projection");

        assert_eq!(delivery.lock().await.as_str(), "Iwo");
        assert!(
            !deltas
                .deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        assert_eq!(projection_count.load(Ordering::SeqCst), 1);
        assert_eq!(terminal.rendered_text, "Iwo");
        assert_eq!(terminal.phase, TranscriptProjectionPhase::Formatted);
        assert!(terminal.can_paste);
        assert!(terminal.can_insert);
        assert!(terminal.can_copy);
        assert!(terminal.can_retranscribe);
        assert!(terminal.can_format);
        assert!(terminal.terminal);
        let bus_bytes = std::fs::read(bus_path).unwrap();
        assert!(
            std::str::from_utf8(&bus_bytes)
                .unwrap()
                .contains("codescribe.transcript-evidence.v1")
        );
        let mut reader = TranscriptProjectionReader::new();
        let tail_projections = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("projection tail must parse");
        let tail_terminal = tail_projections.last().expect("terminal tail projection");
        assert_eq!(tail_terminal.rendered_text, "Iwo");
        assert_eq!(tail_terminal.phase, TranscriptProjectionPhase::Formatted);
        assert!(tail_terminal.can_paste);
        assert!(tail_terminal.terminal);
    }

    #[tokio::test]
    async fn raw_text_lane_failure_and_session_close_cannot_change_delivery() {
        let delivery = Arc::new(Mutex::new("last ledger revision".to_string()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("raw-events.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let projection_count = Arc::new(AtomicUsize::new(0));
        let projection_count_for_callback = Arc::clone(&projection_count);
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(bus),
            Some(Arc::new(StdMutex::new(AcousticLedger::new()))),
            Some(Arc::new(move |_| {
                projection_count_for_callback.fetch_add(1, Ordering::SeqCst);
            })),
        );

        emitter.on_event(&preview(1, "volatile"));
        emitter.on_event(&raw_final("raw final"));
        emitter.on_event(&EngineEvent::Correction {
            rev: 2,
            text: "correction".to_string(),
            previous_text: "raw final".to_string(),
        });
        emitter.on_event(&EngineEvent::ReplaceRange {
            utterance_id: 1,
            start: 0,
            end: 1,
            text: "replacement".to_string(),
            source: LayerSource::TailPatch,
        });
        emitter.on_event(&EngineEvent::InsertAnnotation {
            utterance_id: 1,
            position: 0,
            text: "annotation".to_string(),
            kind: AnnotationKind::HesitationPause,
        });
        emitter.on_event(&EngineEvent::Warning {
            code: "agent_lane_transport_failed".to_string(),
            message: "Tool-enabled response failed (ConnectError: gateway unavailable)".to_string(),
        });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        emitter.finish().await;

        assert_eq!(delivery.lock().await.as_str(), "last ledger revision");
        assert!(!delivery.lock().await.contains("ConnectError"));
        assert_eq!(projection_count.load(Ordering::SeqCst), 0);
        assert!(std::fs::read_to_string(bus_path).unwrap().is_empty());
    }

    /// Effect witness for W4-T15: the request names a session/revision rather
    /// than text identity, Rust mints the next document revision and a
    /// `user-edit` ledger receipt, the Bus persists it after microphone
    /// lifecycle end, and replay returns the same terminal bytes.
    #[tokio::test]
    async fn explicit_revisions_commit_after_refused_terminal_seal() {
        let temp = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "refused-take".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                },
                temp.path().join("refused.jsonl"),
                None,
            )
            .unwrap(),
        );
        let occurrence = OccurrenceIdentity::new("refused-take", 7, 0, 16_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap_or_else(|error| error.into_inner()),
            occurrence,
            1,
            "Pierwsza wersja",
        );
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_event(&mutation);
        let coverage = {
            use codescribe_core::audio::capture_receipt::{
                AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
            };
            let mut ledger = ledger.lock().unwrap();
            let receipt = ledger.assess_seal_coverage(
                "refused-take",
                7,
                &AcousticSpeechEvidence::measured(
                    CaptureEvidenceIdentity::new("refused-take", 7),
                    "capture_energy",
                    AcousticAvailability::Observed {
                        observed_samples: 48_000,
                    },
                    vec![TailSampleRange {
                        session: "refused-take".into(),
                        capture_epoch: 7,
                        sample_start: 0,
                        sample_end: 48_000,
                    }],
                ),
                8_000,
            );
            assert!(!receipt.status.is_complete());
            assert!(ledger.record_seal_coverage(receipt.clone()));
            assert_eq!(
                ledger.seal_terminal("refused-take", 7),
                Err(SealRefusal::CoverageIncomplete)
            );
            receipt
        };
        emitter.on_event(&EngineEvent::SealCoverage {
            receipt: coverage,
            comparison: None,
        });
        let open_revision = emitter.session_state.lock().unwrap().revision;
        assert_eq!(
            emitter.terminal_revision_source("refused-take", open_revision),
            Err(UserRevisionRefusal::NotTerminal),
        );
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "refused-take".to_string(),
            layer_summary: LayerSummary::default(),
        });
        assert_eq!(
            ledger.lock().unwrap().manual_document_revisions()[0].provenance,
            "light-plus",
            "a refused acoustic terminal still permits a presentation revision"
        );
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::CoverageRefused,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let first = crate::presentation::transcript_bus::document_history_at(
            &temp.path().join("refused.jsonl"),
            "refused-take",
        )
        .unwrap()
        .first()
        .unwrap()
        .clone();
        let formatted = emitter
            .apply_formatter_revision(
                "refused-take".to_string(),
                terminal.reducer_revision,
                AiFormatResult {
                    text: "Druga wersja".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("formatter result needs lifecycle end, not a seal");
        assert!(formatted.provenance_receipt.starts_with("formatter-"));
        let edit = UserRevisionIntent {
            session_id: "refused-take".to_string(),
            source_revision: formatted.revision,
            rendered_text: first.rendered_text.clone(),
            provenance: DocumentRevisionProvenance::UserEdit,
        };
        let user_edit = emitter
            .apply_user_revision(edit.clone())
            .expect("history restore needs lifecycle end, not a seal");
        assert_eq!(user_edit.rendered_text, first.rendered_text);
        assert!(user_edit.provenance_receipt.starts_with("user-edit-"));
        let committed = emitter
            .apply_user_revision(UserRevisionIntent {
                source_revision: user_edit.revision,
                rendered_text: "Trzecia wersja".to_string(),
                provenance: DocumentRevisionProvenance::Retranscribe,
                ..edit
            })
            .expect("explicit button pass revises the refused take");
        assert_eq!(committed.rendered_text, "Trzecia wersja");
        assert!(committed.provenance_receipt.starts_with("retranscribe-"));
        assert!(
            ledger
                .lock()
                .unwrap()
                .terminal_finality("refused-take", 7)
                .into_refusal()
                .is_some()
        );
        let rows = std::fs::read_to_string(temp.path().join("refused.jsonl")).unwrap();
        assert!(rows.contains("\"phase\":\"coverage_refused\""));
        assert!(rows.contains("\"reducer_action\":\"apply_manual_edit\""));
        assert!(rows.contains("\"seal_coverage\""));
        let ended_at = rows
            .lines()
            .position(|row| row.contains("\"status\":\"session_ended\""))
            .expect("refused lifecycle row is published");
        let first_edit_after_end = rows
            .lines()
            .skip(ended_at + 1)
            .find(|row| row.contains("\"reducer_action\":\"apply_manual_edit\""))
            .expect("formatter is the first edit after session_ended");
        assert!(first_edit_after_end.contains("\"phase\":\"coverage_refused\""));
        assert!(
            first_edit_after_end.contains(&format!("\"reducer_revision\":{}", formatted.revision))
        );
        assert!(
            rows.lines().any(|row| {
                row.contains(&format!("\"reducer_revision\":{}", formatted.revision))
                    && row.contains("\"phase\":\"coverage_refused\"")
                    && row.contains("\"reducer_action\":\"apply_manual_edit\"")
            }),
            "formatter revision keeps the refused phase"
        );
        for row in rows
            .lines()
            .filter(|row| row.contains("\"reducer_action\":\"apply_manual_edit\""))
        {
            assert!(row.contains("\"phase\":\"coverage_refused\""));
            assert!(row.contains("\"seal_receipt\":null"));
        }
        assert_eq!(
            crate::presentation::transcript_bus::document_history_at(
                &temp.path().join("refused.jsonl"),
                "refused-take"
            )
            .unwrap()
            .last()
            .unwrap()
            .rendered_text,
            "Trzecia wersja"
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn terminal_user_revision_is_ledger_stamped_and_replayable() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("user-revision.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "revision-session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let occurrence = OccurrenceIdentity::new("revision-session", 4, 0, 16_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "Tekst bazowy");
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("revision-session", 4)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "revision-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");

        let commit = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "revision-session".to_string(),
                source_revision: terminal.reducer_revision,
                rendered_text: "Tekst poprawiony przez użytkownika".to_string(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .expect("current terminal revision intent must commit");
        assert_eq!(commit.revision, terminal.reducer_revision + 1);
        assert_eq!(commit.rendered_text, "Tekst poprawiony przez użytkownika");
        assert!(commit.provenance_receipt.starts_with("user-edit-"));
        let stale = emitter.apply_user_revision(UserRevisionIntent {
            session_id: "revision-session".to_string(),
            source_revision: terminal.reducer_revision,
            rendered_text: "Spóźniona edycja".to_string(),
            provenance: DocumentRevisionProvenance::UserEdit,
        });
        assert!(matches!(
            stale,
            Err(UserRevisionRefusal::StaleRevision { .. })
        ));

        emitter.finish().await;
        assert_eq!(
            delivery.lock().await.as_str(),
            "Tekst poprawiony przez użytkownika"
        );
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        // Two document revisions: the terminal Light+ floor ("Tekst bazowy."),
        // then the user edit on top of it.
        assert_eq!(ledger.manual_document_revisions().len(), 2);
        assert_eq!(
            ledger.manual_document_revisions()[0].provenance,
            "light-plus"
        );
        let receipt = &ledger.manual_document_revisions()[1];
        assert_eq!(receipt.provenance, "user-edit");
        assert_eq!(receipt.rendered_text, commit.rendered_text);
        assert_eq!(receipt.source_occurrences, vec![occurrence]);
        drop(ledger);

        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let revision_projection = projected
            .iter()
            .rev()
            .find(|event| event.reducer_revision == commit.revision)
            .expect("user revision projection callback");
        assert_eq!(revision_projection.reducer_action, "apply_manual_edit");
        assert_eq!(
            revision_projection.phase,
            TranscriptProjectionPhase::Formatted
        );
        assert!(revision_projection.terminal);
        assert_eq!(revision_projection.rendered_text, commit.rendered_text);
        assert_eq!(
            revision_projection.acoustic_receipts[0]
                .manual_edit_receipt
                .as_deref(),
            Some(commit.provenance_receipt.as_str())
        );
        drop(projected);

        let bus_bytes = std::fs::read(bus_path).unwrap();
        let mut reader = TranscriptProjectionReader::new();
        let replay = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("revision Bus bytes must replay");
        let replayed_revision = replay.last().expect("replayed user revision");
        assert_eq!(replayed_revision.reducer_revision, commit.revision);
        assert_eq!(replayed_revision.reducer_action, "apply_manual_edit");
        assert_eq!(replayed_revision.rendered_text, commit.rendered_text);
        assert!(replayed_revision.terminal);
    }

    #[tokio::test]
    async fn restoring_first_bus_version_appends_new_document_version() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("versions.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "version-take".into(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                },
                path.clone(),
                None,
            )
            .unwrap(),
        );
        let occurrence = OccurrenceIdentity::new("version-take", 3, 0, 16_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mutation = {
            let mut ledger = ledger.lock().unwrap();
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "First words");
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_event(&mutation);
        let seal = ledger
            .lock()
            .unwrap()
            .seal_terminal("version-take", 3)
            .unwrap();
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "version-take".into(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let initial =
            crate::presentation::transcript_bus::document_history_at(&path, "version-take")
                .unwrap();
        let first = initial.first().unwrap().clone();
        let second = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: terminal.reducer_revision,
                rendered_text: "Second words".into(),
                provenance: DocumentRevisionProvenance::Formatter,
            })
            .unwrap();
        let third = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: second.revision,
                rendered_text: "Third words".into(),
                provenance: DocumentRevisionProvenance::Retranscribe,
            })
            .unwrap();
        let before =
            crate::presentation::transcript_bus::document_history_at(&path, "version-take")
                .unwrap();
        assert_eq!(before[before.len() - 2].provenance, "formatter");
        assert_eq!(before.last().unwrap().provenance, "retranscribe");
        let restored = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: third.revision,
                rendered_text: first.rendered_text.clone(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .unwrap();
        let after = crate::presentation::transcript_bus::document_history_at(&path, "version-take")
            .unwrap();
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(after.last().unwrap().revision, restored.revision);
        assert_eq!(after.last().unwrap().rendered_text, first.rendered_text);
        assert_eq!(after.last().unwrap().provenance, "user-edit");
        emitter.finish().await;
    }

    /// I4m effect witness: a failed formatter result cannot touch ledger, Bus,
    /// projection, or delivery. An applied result then mints formatter
    /// provenance and repaints only through the committed projection callback.
    #[tokio::test]
    async fn terminal_formatter_revision_commits_effect_and_failure_is_pure() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("formatter-revision.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "formatter-session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let occurrence = OccurrenceIdentity::new("formatter-session", 5, 0, 16_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(
                &mut ledger,
                occurrence.clone(),
                1,
                "to jest tekst wymagający formatowania",
            );
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("formatter-session", 5)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "formatter-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");

        let source = emitter
            .terminal_revision_source("formatter-session", terminal.reducer_revision)
            .expect("current terminal source");
        // Light+ ran at the terminal seal, so the formatter is fed shaped text
        // (Light+ before LLM, as the controller always ordered it).
        assert_eq!(source, "To jest tekst wymagający formatowania.");
        let revisions_before_failure = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .manual_document_revisions()
            .len();
        let bus_before_failure = std::fs::read(&bus_path).unwrap();
        let delivery_before_failure = delivery.lock().await.clone();
        let projection_count_before_failure = projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        let failure = emitter.apply_formatter_revision(
            "formatter-session".to_string(),
            terminal.reducer_revision,
            AiFormatResult {
                text: source.clone(),
                reasoning_text: None,
                status: AiFormatStatus::Failed,
            },
        );
        assert_eq!(failure, Err(UserRevisionRefusal::FormatterFailed));
        assert_eq!(
            ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .len(),
            revisions_before_failure
        );
        assert_eq!(std::fs::read(&bus_path).unwrap(), bus_before_failure);
        assert_eq!(
            projected
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            projection_count_before_failure
        );
        assert_eq!(*delivery.lock().await, delivery_before_failure);

        let formatted = "To jest tekst, który wymaga formatowania.".to_string();
        let commit = emitter
            .apply_formatter_revision(
                "formatter-session".to_string(),
                terminal.reducer_revision,
                AiFormatResult {
                    text: formatted.clone(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("applied formatter result must enter revision corridor");
        assert_eq!(commit.rendered_text, formatted);
        assert!(commit.provenance_receipt.starts_with("formatter-"));

        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), formatted);
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.manual_document_revisions().len(), 2);
        assert_eq!(
            ledger.manual_document_revisions()[0].provenance,
            "light-plus"
        );
        assert_eq!(
            ledger.manual_document_revisions()[1].provenance,
            "formatter"
        );
        drop(ledger);
        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let revision_projection = projected
            .iter()
            .rev()
            .find(|event| event.reducer_revision == commit.revision)
            .expect("formatter revision projection callback");
        assert_eq!(revision_projection.reducer_action, "apply_manual_edit");
        assert_eq!(revision_projection.rendered_text, formatted);
        assert_eq!(
            revision_projection.acoustic_receipts[0]
                .manual_edit_receipt
                .as_deref(),
            Some(commit.provenance_receipt.as_str())
        );
    }

    /// Light+ standard in live: an unpunctuated ledger document gains sentence
    /// shape at the terminal seal, as one `light-plus` document revision that
    /// the Bus persists, the delivery buffer carries, the formatter CAS sees
    /// as its source, and a replay of the Bus bytes reproduces. The ledger's
    /// occurrence labels stay untouched — Rust remains the only author.
    #[tokio::test]
    async fn terminal_seal_mints_light_plus_revision_before_session_end() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("light-plus.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "light-plus-session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        // Preserve the two-occurrence terminal beside the single-occurrence
        // falsifier. Explicit scope, never cardinality, identifies both.
        let occurrence = OccurrenceIdentity::new("light-plus-session", 6, 0, 16_000);
        let tail_occurrence = OccurrenceIdentity::new("light-plus-session", 6, 16_000, 32_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let raw_words = "to jest tekst bez interpunkcji yyy i koniec";
        let tail_words = "a to jest ogon";
        let (mutation, tail_mutation) = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, raw_words);
            let tail_mutation =
                admitted_mutation(&mut ledger, tail_occurrence.clone(), 2, tail_words);
            for closed in [&occurrence, &tail_occurrence] {
                ledger.schedule_frontier(closed.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(closed, ObservationProducer::Apple));
            }
            (mutation, tail_mutation)
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        emitter.on_event(&tail_mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("light-plus-session", 6)
            .expect("closed qualified occurrence must produce terminal seal");
        assert!(
            !terminal_seal.is_occurrence_seal(),
            "this test's subject is the terminal seal, not an occurrence seal"
        );
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "light-plus-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        emitter.finish().await;

        let shaped = "To jest tekst bez interpunkcji i koniec a to jest ogon.";
        assert_eq!(delivery.lock().await.as_str(), shaped);

        // The ledger words are untouched; the shaping is a document revision.
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.text_of(&occurrence), Some(raw_words));
        assert_eq!(ledger.text_of(&tail_occurrence), Some(tail_words));
        assert_eq!(ledger.manual_document_revisions().len(), 1);
        let receipt = &ledger.manual_document_revisions()[0];
        assert_eq!(receipt.provenance, "light-plus");
        assert!(receipt.receipt_id.starts_with("light-plus-"));
        assert_eq!(receipt.rendered_text, shaped);
        assert_eq!(
            receipt.source_occurrences,
            vec![occurrence.clone(), tail_occurrence.clone()]
        );
        // Presentation can precede any acoustic seal; terminal Light+ closes it.
        assert!(!ledger.incremental_shapings().is_empty());
        drop(ledger);

        // `session_ended` copies the Light+ revision: Swift's terminal CAS
        // source is the shaped document, and so is the formatter's input.
        assert_eq!(terminal.rendered_text, shaped);
        assert_eq!(terminal.reducer_revision, receipt_revision(&projected));
        assert_eq!(
            emitter
                .terminal_revision_source("light-plus-session", terminal.reducer_revision)
                .expect("light-plus revision is the current terminal source"),
            shaped
        );

        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let light_plus_projection = projected
            .iter()
            .rev()
            .find(|event| event.reducer_action == "apply_manual_edit")
            .expect("light-plus revision projection callback");
        assert_eq!(light_plus_projection.rendered_text, shaped);
        assert_eq!(light_plus_projection.label, tail_words);
        assert!(
            projected.iter().any(
                |event| event.reducer_action == "apply_manual_edit" && event.label == raw_words
            ),
            "every occurrence keeps its own spoken label in the revision"
        );
        assert!(light_plus_projection.terminal);
        assert!(
            light_plus_projection.acoustic_receipts[0]
                .manual_edit_receipt
                .as_deref()
                .is_some_and(|receipt| receipt.starts_with("light-plus-"))
        );
        drop(projected);

        let bus_bytes = std::fs::read(bus_path).unwrap();
        let mut reader = TranscriptProjectionReader::new();
        let replay = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("light-plus Bus bytes must replay");
        let replayed = replay.last().expect("replayed light-plus revision");
        assert_eq!(replayed.rendered_text, shaped);
        assert!(replayed.terminal);
    }

    fn receipt_revision(projected: &StdMutex<Vec<TranscriptBusEvidenceEvent>>) -> u64 {
        projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .expect("at least one projection")
            .reducer_revision
    }

    /// The literal contract (Ctrl-hold `force_raw`): with literal delivery
    /// declared, the terminal seal mints no Light+ revision and the words
    /// reach delivery exactly as the ledger holds them.
    #[tokio::test]
    async fn literal_delivery_keeps_terminal_words_untouched() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("literal.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "literal-session".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path,
                None,
            )
            .unwrap(),
        );
        let occurrence = OccurrenceIdentity::new("literal-session", 7, 0, 16_000);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let raw_words = "słowa literalne bez kropki";
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, raw_words);
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        bus.publish_started();
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("literal-session", 7)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        emitter.finish().await;

        assert_eq!(delivery.lock().await.as_str(), raw_words);
        assert_eq!(terminal.rendered_text, raw_words);
        assert!(
            ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .is_empty()
        );
    }

    #[test]
    fn equal_labels_on_disjoint_occurrences_remain_two_document_entries() {
        let mut ledger = AcousticLedger::new();
        let first = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 7, 0, 8_000),
            1,
            "Iwo",
        );
        let second = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 7, 16_000, 24_000),
            2,
            "Iwo",
        );
        let mut reducer = TranscriptReducer::default();

        for event in [first, second] {
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = event
            else {
                unreachable!();
            };
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
        }

        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(reducer.committed_rendered_text(), "Iwo Iwo");

        // The two entries are kept apart by their PCM span, not by their text.
        // Asserting the exact keys stops a future dedup-by-string from passing
        // this test with one entry and a doubled render.
        let spans = reducer
            .document_by_occurrence
            .keys()
            .map(|occurrence| (occurrence.sample_start, occurrence.sample_end))
            .collect::<Vec<_>>();
        assert_eq!(spans, vec![(0, 8_000), (16_000, 24_000)]);
        for entry in reducer.document_by_occurrence.values() {
            assert_eq!(entry.label, "Iwo");
        }
    }

    #[test]
    fn context_marker_rendered_into_document() {
        fn reducer_with_text(text: &str) -> TranscriptReducer {
            let mut ledger = AcousticLedger::new();
            let event = admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("session", 1, 0, 16_000),
                1,
                text,
            );
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = event
            else {
                unreachable!();
            };
            let mut reducer = TranscriptReducer::default();
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
            reducer
        }

        let mut boundary = reducer_with_text("alpha beta");
        boundary.record_context_marker(5, "{selection_1}");
        assert_eq!(
            boundary.committed_rendered_text(),
            "alpha {selection_1} beta"
        );

        let mut inside_word = reducer_with_text("bardzo mnie drażni");
        inside_word.record_context_marker(9, "{selection_1}");
        assert_eq!(
            inside_word.committed_rendered_text(),
            "bardzo mn{selection_1}ie drażni"
        );

        let mut ordered = reducer_with_text("alpha");
        ordered.record_context_marker(5, "{selection_1}");
        ordered.record_context_marker(5, "{selection_2}");
        ordered.record_context_marker(5, "{selection_3}");
        assert_eq!(
            ordered.committed_rendered_text(),
            "alpha {selection_1} {selection_2} {selection_3}"
        );

        let mut anchored_before_text = TranscriptReducer::default();
        anchored_before_text.record_context_marker(5, "{selection_1}");
        let mut ledger = AcousticLedger::new();
        let event = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 2, 0, 16_000),
            2,
            "alpha beta",
        );
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = event
        else {
            unreachable!();
        };
        anchored_before_text.apply_ledger_mutation(&ledger, &observation, &receipt);
        assert_eq!(
            anchored_before_text.committed_rendered_text(),
            "alpha {selection_1} beta"
        );
    }

    /// A terminal seal closes committed truth. A later non-manual observation
    /// replaying the same occurrence must move neither the label nor the
    /// document, and must not mint a revision.
    #[test]
    fn sealed_occurrence_refuses_a_later_machine_observation() {
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("session", 11, 0, 16_000);
        let admitted = admitted_mutation(&mut ledger, occurrence.clone(), 1, "Iwo");
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted
        else {
            unreachable!();
        };
        let mut reducer = TranscriptReducer::default();
        let first = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("an authenticated occurrence commits");

        // A seal is only mintable once the scheduled observer frontier has
        // actually closed; there is no arbitrary text seal.
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        // Apple is the only scheduled observer, so its return is the exact
        // open -> closed transition.
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let sealed = ledger
            .seal(&occurrence)
            .expect("a closed frontier over qualified audio seals")
            .clone();
        assert!(reducer.apply_ledger_seal(&sealed).is_some());
        let revision_after_seal = reducer.revision;

        // Same occurrence, later generation, different text: refused outright.
        let replay = ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence.clone());
        let replay_receipt = ledger.admit(&replay, "Iwo drugie");
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &replay, &replay_receipt)
                .is_none()
        );

        assert_eq!(reducer.revision, revision_after_seal);
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
        assert_eq!(reducer.committed_rendered_text(), "Iwo");
        assert_eq!(reducer.document_by_occurrence.len(), 1);
        assert_eq!(first.entries.len(), 1);
    }

    fn open_formatter_frontier() -> (AcousticLedger, TranscriptReducer, OccurrenceIdentity) {
        let occurrence = OccurrenceIdentity::new("formatter-session", 9, 0, 16_000);
        let calibration = EnergyCalibration {
            version: "formatter-emitter-test".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        let mut ledger = AcousticLedger::new();
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(
            occurrence.clone(),
            [ObservationProducer::Apple, ObservationProducer::Lexicon],
        );
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let apple_receipt = ledger.admit(&apple, "Iwo");
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let mut reducer = TranscriptReducer::default();
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &apple, &apple_receipt)
                .is_some()
        );

        let lexicon =
            ObservationIdentity::new(ObservationProducer::Lexicon, 1, 0, occurrence.clone());
        let _ = ledger.admit(&lexicon, "Iwo");
        assert!(ledger.schedule_observer(occurrence.clone(), ObservationProducer::Formatter,));
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
        (ledger, reducer, occurrence)
    }

    /// rc-w2-composer-turn: a one-turn take is offered to the provider exactly
    /// once, and an empty turn is never offered at all.
    ///
    /// The count that matters is the number of *requests*, so this asserts the
    /// request itself rather than a call counter bolted beside it: no request
    /// means the controller issues no provider call.
    #[test]
    fn terminal_formatter_request_is_one_exact_cas_pair_for_a_nonempty_turn() {
        let (_ledger, mut reducer, _occurrence) = open_formatter_frontier();

        assert!(
            reducer.terminal_formatter_request().is_none(),
            "a take that has not reached terminal owes no formatting"
        );

        reducer.mark_terminal_lifecycle();
        let request = reducer
            .terminal_formatter_request()
            .expect("a terminal nonempty turn is owed exactly one formatting pass");
        assert_eq!(request.session_id, "formatter-session");
        assert_eq!(request.source_revision, reducer.revision);
        assert_eq!(request.source_text, reducer.committed_rendered_text());
        assert_eq!(
            reducer
                .terminal_revision_source(&request.session_id, request.source_revision)
                .expect("the request must authenticate against the CAS pair it carries"),
            request.source_text
        );
    }

    #[test]
    fn terminal_formatter_request_is_absent_for_an_empty_turn() {
        let mut reducer = TranscriptReducer::default();
        reducer.mark_terminal_lifecycle();

        assert!(
            reducer.terminal_formatter_request().is_none(),
            "an empty turn must produce no provider call to refuse afterwards"
        );
    }

    #[test]
    fn preserve_refuse_and_empty_propose_return_formatter_without_fake_observation() {
        for (disposition, proposed_label) in [
            (LabelProposalDisposition::PreserveExisting, ""),
            (LabelProposalDisposition::Refuse, ""),
            (LabelProposalDisposition::Propose, "   "),
        ] {
            let (mut ledger, mut reducer, occurrence) = open_formatter_frontier();
            let trail_before = ledger.layer_trail_for(&occurrence).count();
            let proposal = OccurrenceLabelProposal::for_existing_occurrence(
                occurrence.session.clone(),
                occurrence.capture_epoch,
                occurrence.sample_start,
                occurrence.sample_end,
                proposed_label,
                disposition,
            );

            let (formatter_returned, revision) =
                reducer.apply_occurrence_label_proposal(&mut ledger, &proposal);
            assert!(formatter_returned);
            assert!(revision.is_none());
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), trail_before);
            assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
            assert!(ledger.seal(&occurrence).is_ok());
            assert_eq!(reducer.committed_rendered_text(), "Iwo");
        }
    }

    #[test]
    fn formatter_proposal_can_only_relabel_one_existing_open_occurrence_once() {
        let (mut ledger, mut reducer, occurrence) = open_formatter_frontier();
        let qualified_before = ledger.qualified_occurrences().count();
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Iwo!",
            LabelProposalDisposition::Propose,
        );

        let (formatter_returned, revision) =
            reducer.apply_occurrence_label_proposal(&mut ledger, &proposal);
        assert!(formatter_returned);
        assert!(revision.is_some());
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo!"));
        assert_eq!(ledger.qualified_occurrences().count(), qualified_before);
        assert_eq!(reducer.document_by_occurrence.len(), 1);
        assert!(ledger.seal(&occurrence).is_ok());

        let trail_after_seal = ledger.layer_trail_for(&occurrence).count();
        let (formatter_returned, revision) =
            reducer.apply_occurrence_label_proposal(&mut ledger, &proposal);
        assert!(!formatter_returned);
        assert!(revision.is_none());
        assert_eq!(
            ledger.layer_trail_for(&occurrence).count(),
            trail_after_seal
        );
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo!"));
        assert_eq!(ledger.qualified_occurrences().count(), qualified_before);
        assert_eq!(reducer.document_by_occurrence.len(), 1);
    }

    /// Acceptance: both emitter request/acknowledgement types keep the derives
    /// their doc comments promise.
    ///
    /// An attribute inserted between a doc comment and its struct stranded
    /// `UserRevisionCommit`'s derives onto the type that followed it, which also
    /// gave that type two identical `derive` attributes. Neither defect is
    /// visible by reading either declaration alone. This exercises `Debug`,
    /// `Clone` and `PartialEq` on both, so the compiler answers the question.
    #[test]
    fn the_emitter_request_and_commit_types_keep_their_declared_derives() {
        let request = TerminalFormatterRequest {
            session_id: "derive-session".to_string(),
            source_revision: 3,
            source_text: "one turn".to_string(),
        };
        assert_eq!(request.clone(), request);
        assert!(format!("{request:?}").contains("derive-session"));

        let commit = UserRevisionCommit {
            session_id: "derive-session".to_string(),
            source_revision: 3,
            revision: 4,
            rendered_text: "one turn.".to_string(),
            provenance_receipt: "formatter-derive".to_string(),
        };
        assert_eq!(commit.clone(), commit);
        assert!(format!("{commit:?}").contains("formatter-derive"));
    }

    // -----------------------------------------------------------------------
    // Live (incremental) Light+ — shaping closed occurrences during capture
    // -----------------------------------------------------------------------

    /// One take wired exactly as the controller wires it: the reducer, the
    /// ledger it is bound to, a real Bus file and a synchronous projection
    /// observer. Nothing here is a stand-in — every assertion below runs
    /// against admitted ledger receipts and published Bus events.
    struct LiveTake {
        delivery: Arc<Mutex<String>>,
        bus: Arc<TranscriptBus>,
        bus_path: std::path::PathBuf,
        ledger: Arc<StdMutex<AcousticLedger>>,
        projected: Arc<StdMutex<Vec<TranscriptBusEvidenceEvent>>>,
        emitter: PresentationEmitter,
        _temp: tempfile::TempDir,
    }

    impl LiveTake {
        /// Qualify and admit one Apple observation, then hand it to the emitter.
        fn admit(&self, occurrence: &OccurrenceIdentity, request: u64, label: &str) {
            let mutation = {
                let mut ledger = self
                    .ledger
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                admitted_mutation(&mut ledger, occurrence.clone(), request, label)
            };
            self.emitter.on_event(&mutation);
        }

        /// Close the occurrence's frontier, seal it in the ledger, and deliver
        /// the resulting occurrence seal to the emitter. Returns the event so a
        /// test can replay the exact same observation.
        fn seal(&self, occurrence: &OccurrenceIdentity) -> EngineEvent {
            let receipt = {
                let mut ledger = self
                    .ledger
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
                ledger
                    .seal(occurrence)
                    .expect("a closed qualified occurrence must seal")
                    .clone()
            };
            assert!(
                receipt.is_occurrence_seal(),
                "this helper produces occurrence seals, never a lifecycle end"
            );
            let event = EngineEvent::LedgerSeal { receipt };
            self.emitter.on_event(&event);
            event
        }

        fn shapings(&self) -> Vec<IncrementalShapingReceipt> {
            self.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .incremental_shapings()
                .to_vec()
        }

        fn shaping_projections(&self) -> Vec<TranscriptBusEvidenceEvent> {
            self.projected
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .filter(|event| event.reducer_action == "apply_incremental_shaping")
                .cloned()
                .collect()
        }
    }

    fn live_take(session_id: &str) -> LiveTake {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("incremental-light.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session_id.to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event: &TranscriptBusEvidenceEvent| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        LiveTake {
            delivery,
            bus,
            bus_path,
            ledger,
            projected,
            emitter,
            _temp: temp,
        }
    }

    #[tokio::test]
    async fn pcm_gap_sets_live_sentence_boundary_without_sealing_occurrences() {
        let mut take = live_take("pause-session");
        let first = OccurrenceIdentity::new("pause-session", 21, 0, 16_000);
        let short_gap = OccurrenceIdentity::new("pause-session", 21, 19_200, 35_200);
        let long_gap = OccurrenceIdentity::new("pause-session", 21, 48_000, 64_000);
        take.admit(&first, 1, "pierwsze slowa");
        take.admit(&short_gap, 2, "drugie slowa");
        take.admit(&long_gap, 3, "trzecie slowa");
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze slowa drugie slowa. Trzecie slowa"
        );
        let ledger = take.ledger.lock().unwrap();
        assert!(ledger.seal_of(&first).is_none());
        assert!(ledger.seal_of(&short_gap).is_none());
        assert!(ledger.seal_of(&long_gap).is_none());
        assert_eq!(ledger.incremental_shapings().len(), 2);
        assert!(!ledger.incremental_shapings()[0].sentence_break_before);
        assert!(ledger.incremental_shapings()[1].sentence_break_before);
        assert!(
            ledger.incremental_shapings()[1]
                .source_seal_receipt
                .is_none()
        );
    }

    /// Acceptance: an actual occurrence seal, arriving long before any
    /// lifecycle end, shapes exactly those words and publishes the shaped
    /// bytes to the Bus, the projection callback and the delivery buffer —
    /// while the session stays open.
    ///
    /// This is the claim the whole cut exists for, so it is checked against the
    /// real ledger receipt (provenance, seal, source label) rather than against
    /// a string a helper produced.
    #[tokio::test]
    async fn occurrence_seal_shapes_committed_words_while_the_session_stays_open() {
        let mut take = live_take("live-session");
        let occurrence = OccurrenceIdentity::new("live-session", 11, 0, 16_000);
        let spoken = "to jest pierwsze zdanie";

        take.admit(&occurrence, 1, spoken);
        let seal_event = take.seal(&occurrence);
        let EngineEvent::LedgerSeal { receipt: seal } = &seal_event else {
            panic!("seal helper must produce a ledger seal event");
        };
        let seal_receipt_id = seal.receipt_id.clone();
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "To jest pierwsze zdanie",
            "the closed occurrence reaches delivery shaped, during capture"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1, "exactly one shaping for one seal");
        let shaping = &shapings[0];
        assert_eq!(shaping.provenance, "light-plus");
        assert_eq!(shaping.occurrence, occurrence);
        assert_eq!(shaping.source_label, spoken);
        assert_eq!(shaping.shaped_text, "To jest pierwsze zdanie");
        assert!(shaping.source_seal_receipt.is_none());
        assert_eq!(shaping.revision, shaping.source_revision + 1);

        // The acoustic label is untouched: shaping is presentation, not words.
        let ledger = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.text_of(&occurrence), Some(spoken));
        assert!(
            ledger.manual_document_revisions().is_empty(),
            "a live shape is not a whole-document edit"
        );
        drop(ledger);

        let projections = take.shaping_projections();
        assert_eq!(projections.len(), 1);
        let projection = &projections[0];
        assert_eq!(projection.rendered_text, "To jest pierwsze zdanie");
        assert_eq!(projection.label, spoken, "the projected label stays spoken");
        assert_eq!(projection.phase, TranscriptProjectionPhase::Listening);
        assert!(!projection.terminal, "a shape is not a terminal revision");
        assert!(
            !projection.lifecycle_terminal,
            "a shape never ends the session"
        );
        assert_eq!(projection.delivery, TranscriptDelivery::Unattempted);
        assert!(
            projection.acoustic_receipts[0]
                .manual_edit_receipt
                .is_none(),
            "a shape must not masquerade as a human correction"
        );
        assert_eq!(
            projection.acoustic_receipts[0].seal_receipt.as_deref(),
            None
        );
        assert_eq!(
            take.ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .map(|seal| seal.receipt_id.as_str().to_owned()),
            Some(seal_receipt_id)
        );

        let bus_bytes = std::fs::read(&take.bus_path).unwrap();
        let text = std::str::from_utf8(&bus_bytes).unwrap();
        assert!(text.contains("apply_incremental_shaping"));
        assert!(
            !text.contains("session_ended"),
            "the lifecycle is still open"
        );
    }

    /// Acceptance: the next occurrence keeps the earlier shape and carries its
    /// own exact words. No stale whole-document override, no prefix loss, and
    /// no text-based deduplication.
    #[tokio::test]
    async fn later_speech_preserves_the_earlier_shape_and_its_own_exact_words() {
        let mut take = live_take("join-session");
        let first = OccurrenceIdentity::new("join-session", 12, 0, 16_000);
        let second = OccurrenceIdentity::new("join-session", 12, 16_000, 32_000);

        take.admit(&first, 1, "to jest pierwsze zdanie");
        take.seal(&first);
        let after_first = take.shapings();

        // Later speech arrives unsealed: the shaped prefix must survive it and
        // the new words must appear exactly as the ledger holds them.
        take.admit(&second, 2, "a to jest drugie");
        let open_projection = take
            .projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .cloned()
            .expect("the insert publishes a revision");
        assert_eq!(
            open_projection.rendered_text, "To jest pierwsze zdanie a to jest drugie",
            "shaped prefix preserved, open suffix byte-exact"
        );

        let retained = take
            .projected
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| {
                event.reducer_revision == open_projection.reducer_revision
                    && event.document_index == 0
            })
            .unwrap()
            .acoustic_receipts[0]
            .presentation_receipt
            .clone()
            .unwrap();
        assert_eq!(retained.receipt_id, after_first[0].receipt_id);
        assert_eq!(retained.source_revision, after_first[0].source_revision);
        assert_eq!(retained.shaped_text, after_first[0].shaped_text);
        assert!(
            open_projection.acoustic_receipts[0]
                .presentation_receipt
                .is_none()
        );
        take.seal(&second);
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "To jest pierwsze zdanie a to jest drugie",
            "the second span capitalises because its left context closed"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1);
        assert_eq!(
            shapings[0], after_first[0],
            "the first shaping receipt is never rewritten"
        );
        assert_eq!(
            take.ledger.lock().unwrap().text_of(&second),
            Some("a to jest drugie")
        );
    }

    /// Acceptance: an unsealed suffix is never shaped and never appears in a
    /// sealed source receipt. Closed geometry and seal history stay immutable.
    #[tokio::test]
    async fn an_open_suffix_is_shaped_without_claiming_a_seal() {
        let mut take = live_take("open-suffix-session");
        let closed = OccurrenceIdentity::new("open-suffix-session", 13, 0, 16_000);
        let open = OccurrenceIdentity::new("open-suffix-session", 13, 16_000, 32_000);
        let open_words = "  jeszcze mowie i nie skonczylem\n\t";

        take.admit(&closed, 1, "pierwsza czesc juz zamknieta");
        take.seal(&closed);
        take.admit(&open, 2, open_words);
        take.emitter.finish().await;

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 2, "both committed occurrences were shaped");
        assert_eq!(shapings[0].occurrence, closed);
        assert!(
            shapings.iter().any(|shaping| shaping.occurrence == open),
            "an open committed occurrence has presentation authority"
        );

        let rendered = take.delivery.lock().await.clone();
        assert!(
            rendered.ends_with("jeszcze mowie i nie skonczylem"),
            "the open suffix keeps its words: {rendered}"
        );
        assert!(rendered.starts_with("Pierwsza czesc juz zamknieta"));

        // The seal the shaping cites is still the one the ledger holds.
        let ledger = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(ledger.seal_of(&closed).is_some());
        assert!(shapings[0].source_seal_receipt.is_none());
        assert!(shapings[1].source_seal_receipt.is_none());
        assert!(ledger.seal_of(&open).is_none());
    }

    /// Acceptance: equal words in distinct occurrences remain intentional
    /// repetition. Shaping is per-occurrence, so nothing can collapse them.
    #[tokio::test]
    async fn equal_words_in_distinct_occurrences_stay_two_document_entries() {
        let mut take = live_take("iwo-session");
        let first = OccurrenceIdentity::new("iwo-session", 14, 0, 16_000);
        let second = OccurrenceIdentity::new("iwo-session", 14, 16_000, 32_000);

        take.admit(&first, 1, "Iwo");
        take.seal(&first);
        take.admit(&second, 2, "Iwo");
        take.seal(&second);
        take.emitter.finish().await;

        assert_eq!(take.delivery.lock().await.as_str(), "Iwo Iwo");
        let shapings = take.shapings();
        assert!(
            shapings.is_empty(),
            "unchanged text needs no shaping receipt"
        );
        let ledger = take.ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&first), Some("Iwo"));
        assert_eq!(ledger.text_of(&second), Some("Iwo"));
        assert!(take.shaping_projections().is_empty());
    }

    /// Acceptance: a duplicate seal observation mints no second revision and no
    /// second receipt. Replaying the identical event must be a no-op.
    #[tokio::test]
    async fn a_replayed_seal_mints_no_duplicate_shaping_revision() {
        let mut take = live_take("replay-session");
        let occurrence = OccurrenceIdentity::new("replay-session", 15, 0, 16_000);

        take.admit(&occurrence, 1, "raz powiedziane zdanie");
        let seal_event = take.seal(&occurrence);
        let revision_before = take.emitter.session_state.lock().unwrap().revision;
        let callbacks_before = take.projected.lock().unwrap().len();
        let bus_before = std::fs::read(&take.bus_path).unwrap();

        take.emitter.on_event(&seal_event);
        take.emitter.on_event(&seal_event);
        take.emitter.finish().await;

        assert_eq!(
            take.emitter.session_state.lock().unwrap().revision,
            revision_before
        );
        assert_eq!(take.projected.lock().unwrap().len(), callbacks_before);
        assert_eq!(std::fs::read(&take.bus_path).unwrap(), bus_before);
        assert_eq!(take.shapings().len(), 1, "one occurrence, one shaping");
        assert_eq!(
            take.shaping_projections().len(),
            1,
            "a replayed seal publishes no second shaping"
        );
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Raz powiedziane zdanie"
        );
    }

    /// Acceptance: empty shaping cannot erase valid words. A hesitation-only
    /// utterance shapes to nothing, so the shaping is refused and the spoken
    /// label stays visible.
    #[tokio::test]
    async fn a_hesitation_only_occurrence_keeps_its_spoken_words() {
        let mut take = live_take("hesitation-session");
        let occurrence = OccurrenceIdentity::new("hesitation-session", 16, 0, 16_000);

        take.admit(&occurrence, 1, "yyy");
        take.seal(&occurrence);
        take.emitter.finish().await;

        assert!(
            take.shapings().is_empty(),
            "a shape that deletes every word is refused"
        );
        assert_eq!(take.delivery.lock().await.as_str(), "yyy");
        assert_eq!(
            take.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .text_of(&occurrence),
            Some("yyy")
        );
    }

    /// Acceptance: the raw/literal lane stays byte-exact during capture, not
    /// only at the terminal boundary.
    #[tokio::test]
    async fn literal_delivery_keeps_live_occurrences_byte_exact() {
        let mut take = live_take("literal-live-session");
        let occurrence = OccurrenceIdentity::new("literal-live-session", 17, 0, 16_000);
        let spoken = "  surowe  słowa\n\t";

        take.emitter.set_literal_delivery(true);
        take.admit(&occurrence, 1, spoken);
        take.seal(&occurrence);
        take.emitter.finish().await;

        assert!(take.shapings().is_empty());
        assert!(take.shaping_projections().is_empty());
        assert_eq!(take.delivery.lock().await.as_str(), spoken);
    }

    /// Acceptance: a shape describes exactly the words it was taken from. When
    /// a human supersedes the sealed label, the stale shape is dropped and the
    /// human's exact words are rendered — never the old shaped bytes.
    #[tokio::test]
    async fn a_human_relabel_drops_the_stale_shape_instead_of_rendering_it() {
        let mut take = live_take("relabel-session");
        let occurrence = OccurrenceIdentity::new("relabel-session", 18, 0, 16_000);

        take.admit(&occurrence, 1, "iwo");
        take.seal(&occurrence);
        assert_eq!(take.shapings().len(), 1);

        let manual = {
            let mut ledger = take
                .ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let observation = ObservationIdentity::new(
                ObservationProducer::ManualHuman,
                9,
                0,
                occurrence.clone(),
            );
            let receipt = ledger.admit(&observation, "Iwona");
            assert!(
                receipt.grants_mutation(),
                "an explicit human edit may supersede a seal"
            );
            EngineEvent::LedgerMutation {
                observation,
                label: "Iwona".to_string(),
                receipt,
            }
        };
        take.emitter.on_event(&manual);
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Iwona",
            "the stale shape must not survive the words it described"
        );
        assert_eq!(
            take.shapings().len(),
            1,
            "dropping a stale shape rewrites no receipt history"
        );
    }

    /// Recovery falsifier: a real terminal is not identified by cardinality.
    /// UNRUN under W1; a whole-session seal opens edit CAS before lifecycle end.
    #[tokio::test]
    async fn a_single_occurrence_terminal_opens_the_current_shaped_cas_source() {
        let mut take = live_take("single-terminal");
        let occurrence = OccurrenceIdentity::new("single-terminal", 19, 0, 16_000);
        take.admit(&occurrence, 1, "jedno zdanie");
        take.seal(&occurrence);
        let seal = take
            .ledger
            .lock()
            .unwrap()
            .seal_terminal("single-terminal", 19)
            .unwrap();
        assert_ne!(
            seal.receipt_id,
            take.ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .unwrap()
                .receipt_id
        );
        take.emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: seal.clone(),
        });
        let revision = take.emitter.session_state.lock().unwrap().revision;
        assert_eq!(
            take.emitter
                .terminal_revision_source("single-terminal", revision),
            Ok("Jedno zdanie.".to_string()),
            "genuine terminal CAS opens before lifecycle end"
        );
        let callbacks = take.projected.lock().unwrap().len();
        take.emitter
            .on_event(&EngineEvent::LedgerSeal { receipt: seal });
        assert_eq!(
            take.emitter.session_state.lock().unwrap().revision,
            revision
        );
        assert_eq!(take.projected.lock().unwrap().len(), callbacks);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "single-terminal".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        take.emitter.finish().await;
        assert_eq!(take.delivery.lock().await.as_str(), "Jedno zdanie.");
        assert_eq!(
            take.emitter
                .terminal_revision_source("single-terminal", terminal.reducer_revision),
            Ok("Jedno zdanie.".to_string()),
            "one occurrence must permit the same authenticated terminal CAS as two"
        );
    }

    #[tokio::test]
    async fn sealed_take_formats_and_restores_before_and_after_lifecycle_end() {
        let mut take = live_take("sealed-revisions");
        let occurrence = OccurrenceIdentity::new("sealed-revisions", 19, 0, 16_000);
        take.admit(&occurrence, 1, "jedno zdanie");
        take.seal(&occurrence);
        let seal = take
            .ledger
            .lock()
            .unwrap()
            .seal_terminal("sealed-revisions", 19)
            .unwrap();
        assert!(!seal.is_occurrence_seal());
        take.emitter
            .on_event(&EngineEvent::LedgerSeal { receipt: seal });

        let source = take.emitter.terminal_formatter_request().unwrap();
        let original = source.source_text.clone();
        let before = take
            .emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                source.source_revision,
                AiFormatResult {
                    text: "Format before end".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("a sealed take admits Format before lifecycle end");
        assert!(before.provenance_receipt.starts_with("formatter-"));
        let restored = take
            .emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: source.session_id.clone(),
                source_revision: before.revision,
                rendered_text: original.clone(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .expect("a sealed take admits Restore before lifecycle end");
        assert_eq!(restored.rendered_text, original);
        assert!(restored.provenance_receipt.starts_with("user-edit-"));

        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: source.session_id.clone(),
            layer_summary: LayerSummary::default(),
        });
        let ended = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let after = take
            .emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                ended.reducer_revision,
                AiFormatResult {
                    text: "Format after end".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("a sealed take admits Format after lifecycle end");
        assert!(after.provenance_receipt.starts_with("formatter-"));
        let restored = take
            .emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: source.session_id,
                source_revision: after.revision,
                rendered_text: original.clone(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .expect("a sealed take admits Restore after lifecycle end");
        assert_eq!(restored.rendered_text, original);
        assert!(restored.provenance_receipt.starts_with("user-edit-"));
        take.emitter.finish().await;
    }

    #[tokio::test]
    async fn late_ledger_words_after_lifecycle_end_still_commit() {
        let mut take = live_take("late-l1");
        let first = OccurrenceIdentity::new("late-l1", 19, 0, 16_000);
        let second = OccurrenceIdentity::new("late-l1", 19, 16_000, 32_000);
        take.admit(&first, 1, "pierwsze zdanie");
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "late-l1".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let before = take.emitter.session_state.lock().unwrap().revision;
        take.admit(&second, 2, "drugie zdanie");
        take.emitter.finish().await;
        assert!(take.emitter.session_state.lock().unwrap().revision > before);
        assert!(take.delivery.lock().await.contains("drugie zdanie"));
    }

    /// Acceptance: Stop delivers the current canonical shaped document exactly
    /// once. A document that live shaping already settled mints no second
    /// whole-document Light+ revision, and the terminal CAS source is the same
    /// bytes Swift already holds.
    #[tokio::test]
    async fn stop_closes_live_presentation_with_one_light_plus_revision() {
        let mut take = live_take("stop-session");
        let first = OccurrenceIdentity::new("stop-session", 19, 0, 16_000);
        let second = OccurrenceIdentity::new("stop-session", 19, 16_000, 32_000);

        take.admit(&first, 1, "pierwsze zdanie tutaj");
        take.seal(&first);
        take.admit(&second, 2, "a potem drugie");
        take.seal(&second);

        let terminal_seal = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("stop-session", 19)
            .expect("every occurrence is sealed, so the epoch seals");
        assert!(!terminal_seal.is_occurrence_seal());
        take.emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "stop-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        take.emitter.finish().await;

        let shaped = "Pierwsze zdanie tutaj a potem drugie.";
        assert_eq!(take.delivery.lock().await.as_str(), shaped);
        assert_eq!(terminal.rendered_text, shaped);
        assert!(terminal.terminal);
        assert_eq!(
            take.shapings().len(),
            1,
            "only changed presentation mints a live receipt"
        );
        assert!(
            take.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .len()
                == 1,
            "an already-shaped document mints no duplicate terminal revision"
        );
        assert_eq!(
            take.emitter
                .terminal_revision_source("stop-session", terminal.reducer_revision)
                .expect("the shaped document is the terminal CAS source"),
            shaped
        );
        assert_eq!(
            take.emitter
                .terminal_formatter_request()
                .expect("a non-empty terminal turn is owed one formatter pass")
                .source_text,
            shaped
        );
        assert!(
            take.bus
                .publish_ended(
                    TranscriptSessionEndReason::Completed,
                    true,
                    TranscriptDelivery::Unattempted
                )
                .is_none()
        );
        take.emitter.finish().await;
        assert_eq!(take.delivery.lock().await.as_str(), shaped);
        assert!(
            take.shaping_projections()
                .iter()
                .all(|event| !event.terminal && !event.lifecycle_terminal),
            "no live revision may claim the lifecycle end"
        );
    }

    /// Acceptance: foreign, stale and unsealed sources refuse, and a
    /// whole-document revision keeps ownership of presentation once it exists.
    /// Exercised against the real reducer and ledger, one refusal at a time.
    #[test]
    fn foreign_and_owned_documents_refuse_incremental_shaping() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("refusal-session", 20, 0, 16_000);
        let stranger = OccurrenceIdentity::new("other-session", 20, 0, 16_000);

        // Nothing committed at all.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::UnknownOccurrence)
        );

        let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "jakies slowa");
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = &mutation
        else {
            panic!("admitted_mutation must produce a ledger mutation");
        };
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .is_some()
        );

        // A committed label can be presented before acoustic finality.
        let live = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .expect("committed words can be shaped before a seal");
        assert_eq!(live.rendered_text, "Jakies slowa");

        // A foreign occurrence never reaches the ledger at all.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &stranger),
            Err(IncrementalShapingRefusal::UnknownOccurrence)
        );

        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        ledger.seal(&occurrence).expect("closed occurrence seals");
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::AlreadyShaped)
        );
        assert!(
            reducer
                .shaping_receipt_of(&occurrence)
                .is_some_and(|receipt| receipt.starts_with("light-plus-incremental-"))
        );

        // Idempotent: the same sealed label is not shaped twice.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::AlreadyShaped)
        );
    }

    /// Acceptance: a terminal whole-document revision keeps ownership of the
    /// presentation, and an occurrence seal never opens the terminal corridor.
    ///
    /// The explicitly terminal epoch seal is the only door to a user edit.
    /// This two-occurrence case complements the single-occurrence falsifier. Once that edit lands, a live shape addressed
    /// at one of its source occurrences is refused rather than allowed to
    /// overwrite part of the human's document.
    #[test]
    fn a_terminal_document_revision_keeps_ownership_of_the_presentation() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        let first = OccurrenceIdentity::new("owned-session", 21, 0, 16_000);
        let second = OccurrenceIdentity::new("owned-session", 21, 16_000, 32_000);

        let mut latest_revision = 0;
        for (index, (occurrence, label)) in
            [(&first, "surowe slowa"), (&second, "i jeszcze wiecej")]
                .into_iter()
                .enumerate()
        {
            let mutation =
                admitted_mutation(&mut ledger, occurrence.clone(), index as u64 + 1, label);
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = &mutation
            else {
                panic!("admitted_mutation must produce a ledger mutation");
            };
            latest_revision = reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .expect("qualified observation commits")
                .revision;
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
        }

        // Before the terminal seal, the corridor is closed by law.
        assert_eq!(
            reducer.apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: "owned-session".to_string(),
                    source_revision: latest_revision,
                    rendered_text: "Za wczesnie.".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                }
            ),
            Err(UserRevisionRefusal::NotTerminal),
            "an open document must never admit a whole-document edit"
        );

        let terminal = ledger
            .seal_terminal("owned-session", 21)
            .expect("the epoch seals");
        assert!(!terminal.is_occurrence_seal());
        latest_revision = reducer
            .apply_ledger_seal(&terminal)
            .expect("the terminal seal projects")
            .revision;

        let committed = reducer
            .apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: "owned-session".to_string(),
                    source_revision: latest_revision,
                    rendered_text: "Slowa poprawione przez czlowieka.".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                },
            )
            .expect("a terminal document admits an authenticated user edit");
        assert_eq!(committed.rendered_text, "Slowa poprawione przez czlowieka.");

        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &first),
            Err(IncrementalShapingRefusal::DocumentRevisionOwnsPresentation),
            "a live shape must not overwrite part of a committed document edit"
        );
        assert!(ledger.incremental_shapings().is_empty());
    }

    /// Acceptance: an occurrence from another session is refused even when it
    /// somehow shares a reducer. The document's session is the one the first
    /// committed occurrence named; nothing else may shape into it.
    #[test]
    fn an_occurrence_from_a_foreign_session_is_refused() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        // `OccurrenceIdentity` orders by session first, so `aaa-` is the
        // document's session and `zzz-` is unambiguously the foreigner.
        let native = OccurrenceIdentity::new("aaa-session", 22, 0, 16_000);
        let foreign = OccurrenceIdentity::new("zzz-session", 22, 0, 16_000);

        for (index, (occurrence, label)) in [(&native, "swoje slowa"), (&foreign, "obce slowa")]
            .into_iter()
            .enumerate()
        {
            let mutation =
                admitted_mutation(&mut ledger, occurrence.clone(), index as u64 + 1, label);
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = &mutation
            else {
                panic!("admitted_mutation must produce a ledger mutation");
            };
            let admitted = reducer.apply_ledger_mutation(&ledger, observation, receipt);
            if occurrence == &native {
                assert!(admitted.is_some());
            } else {
                assert!(
                    admitted.is_none(),
                    "a qualified foreign entry is refused at reducer admission"
                );
                // Preserve the predecessor's contaminated-document falsifier
                // explicitly, without weakening production session admission.
                let mut foreign_reducer = TranscriptReducer::default();
                let foreign_revision = foreign_reducer
                    .apply_ledger_mutation(&ledger, observation, receipt)
                    .unwrap();
                reducer
                    .document_by_occurrence
                    .insert(foreign.clone(), foreign_revision.entries[0].clone());
            }
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
            ledger.seal(occurrence).expect("closed occurrence seals");
        }

        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &foreign),
            Err(IncrementalShapingRefusal::ForeignSession)
        );
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &native),
            Err(IncrementalShapingRefusal::ForeignSession),
            "the entire contaminated document refuses"
        );
        reducer.document_by_occurrence.remove(&foreign);
        assert!(
            reducer
                .apply_incremental_shaping(&mut ledger, &native)
                .is_ok(),
            "the document's own session still shapes"
        );
        let shapings = ledger.incremental_shapings();
        assert_eq!(shapings.len(), 1);
        assert_eq!(shapings[0].occurrence, native);
    }
    #[tokio::test]
    async fn late_predecessor_closure_shapes_in_document_order_with_immutable_context() {
        let mut take = live_take("closure-order");
        let first = OccurrenceIdentity::new("closure-order", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("closure-order", 1, 16_000, 32_000);
        take.admit(&first, 1, "pierwsze zdanie");
        take.admit(&second, 2, "drugie zdanie");
        take.seal(&second);
        assert!(
            take.shapings().len() == 1,
            "an open predecessor does not veto committed presentation"
        );
        take.seal(&first);
        take.emitter.finish().await;
        let receipts = take.shapings();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].occurrence, first);
        assert_eq!(
            take.ledger.lock().unwrap().text_of(&second),
            Some("drugie zdanie")
        );
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie drugie zdanie"
        );
    }

    #[tokio::test]
    async fn late_predecessor_insertion_invalidates_retained_shape_without_rewriting_receipt() {
        let mut take = live_take("late-insert");
        let first = OccurrenceIdentity::new("late-insert", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("late-insert", 1, 16_000, 32_000);
        take.admit(&second, 2, "drugie zdanie");
        take.seal(&second);
        let original = take.shapings()[0].clone();
        take.admit(&first, 1, "pierwsze zdanie");
        let revision = take.projected.lock().unwrap().last().unwrap().clone();
        assert_eq!(revision.rendered_text, "Pierwsze zdanie drugie zdanie");
        assert!(revision.acoustic_receipts[0].presentation_receipt.is_none());
        take.seal(&first);
        take.emitter.finish().await;
        assert_eq!(
            take.shapings()[0],
            original,
            "historical provenance never changes"
        );
        let receipts = take.shapings();
        let current = receipts.last().unwrap();
        assert_eq!(current.occurrence, first);
        assert_ne!(current.receipt_id, original.receipt_id);
        assert!(current.left_context.is_empty());
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie drugie zdanie"
        );
    }

    #[tokio::test]
    async fn unchanged_shape_and_late_observer_do_not_mint_publication() {
        let mut take = live_take("unchanged-observer");
        let occurrence = OccurrenceIdentity::new("unchanged-observer", 1, 0, 16_000);
        take.admit(&occurrence, 1, "Already shaped.");
        let seal = take.seal(&occurrence);
        let before = take.emitter.session_state.lock().unwrap().revision;
        let rows = take.projected.lock().unwrap().len();
        let bytes = std::fs::read(&take.bus_path).unwrap();
        let (observation, receipt) = {
            let mut ledger = take.ledger.lock().unwrap();
            let observation =
                ObservationIdentity::new(ObservationProducer::Whisper, 99, 0, occurrence);
            let receipt = ledger.admit(&observation, "late rewrite");
            assert!(!receipt.grants_mutation());
            (observation, receipt)
        };
        take.emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            receipt,
            label: "late rewrite".to_string(),
        });
        take.emitter.on_event(&seal);
        take.emitter.finish().await;
        assert_eq!(take.emitter.session_state.lock().unwrap().revision, before);
        assert_eq!(take.projected.lock().unwrap().len(), rows);
        assert_eq!(std::fs::read(&take.bus_path).unwrap(), bytes);
        assert!(take.shapings().is_empty());
        assert_eq!(take.delivery.lock().await.as_str(), "Already shaped.");
    }

    #[tokio::test]
    async fn forged_snapshot_never_reaches_the_actual_delivery_worker() {
        let mut take = live_take("delivery-forgery");
        let occurrence = OccurrenceIdentity::new("delivery-forgery", 1, 0, 16_000);
        take.admit(&occurrence, 1, "real words");
        take.seal(&occurrence);
        let mut revision = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .record_context_marker(0, "context")
            .unwrap();
        revision.rendered_text = "forged delivery".to_string();
        let rows = take.projected.lock().unwrap().len();
        take.emitter.publish_revision(revision);
        take.emitter.finish().await;
        assert_eq!(take.projected.lock().unwrap().len(), rows);
        assert_eq!(take.delivery.lock().await.as_str(), "Real words");
    }

    #[tokio::test]
    async fn bus_refusal_cannot_update_only_the_delivery_buffer() {
        let mut take = live_take("ended-publication");
        let occurrence = OccurrenceIdentity::new("ended-publication", 1, 0, 16_000);
        take.admit(&occurrence, 1, "real words");
        take.seal(&occurrence);
        take.bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        let revision = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .record_context_marker(0, "late context")
            .unwrap();
        let callbacks = take.projected.lock().unwrap().len();
        take.emitter.publish_revision(revision);
        take.emitter.finish().await;
        assert_eq!(take.projected.lock().unwrap().len(), callbacks);
        assert_eq!(take.delivery.lock().await.as_str(), "Real words");
    }

    /// Unanchored overlap stays on the visible projection at its PCM position,
    /// leaves the neighbour's committed label alone, and does not become a
    /// second document token. A pin wholly inside an admitted range is counted
    /// and is not painted a second time.
    #[test]
    fn unanchored_overlap_is_visible_without_replacing_or_duplicating_a_neighbour() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let beta = OccurrenceIdentity::new("overlap-visible", 1, 24_000, 48_000);
        let gamma = OccurrenceIdentity::new("overlap-visible", 1, 48_000, 72_000);
        for (request, occurrence, label) in [(1, beta.clone(), "beta"), (2, gamma.clone(), "gamma")]
        {
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = admitted_mutation(&mut ledger, occurrence, request, label)
            else {
                unreachable!()
            };
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
        }
        let straddling = OccurrenceIdentity::new("overlap-visible", 1, 40_000, 56_000);
        let straddle_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, straddling.clone());
        let straddle = ledger.admit(&straddle_observation, "przez granice");
        assert!(matches!(
            &straddle,
            MutationReceipt::KeepVisibleUnanchored { label, occurrence, .. }
                if label == "przez granice" && occurrence == &straddling
        ));
        assert!(!straddle.grants_mutation());
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &straddle_observation, &straddle)
                .is_none()
        );
        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(
            reducer.document_by_occurrence.get(&beta).unwrap().label,
            "beta"
        );
        assert_eq!(
            reducer.document_by_occurrence.get(&gamma).unwrap().label,
            "gamma"
        );
        assert_eq!(
            reducer.visible_projection(),
            "beta przez granice gamma",
            "the straddling phrase stays visible between its neighbours"
        );
        assert_eq!(ledger.text_of(&beta), Some("beta"));
        assert_eq!(ledger.text_of(&gamma), Some("gamma"));

        let covered = OccurrenceIdentity::new("overlap-visible", 1, 32_000, 40_000);
        let covered_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 4, 0, covered);
        let duplicate = ledger.admit(&covered_observation, "beta");
        assert!(matches!(
            duplicate,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &covered_observation, &duplicate)
                .is_none()
        );
        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(
            reducer.visible_projection(),
            "beta przez granice gamma",
            "a word already admitted on the overlapping range is not painted twice"
        );
        let tally = ledger.conservation();
        assert_eq!(tally.observations_in, tally.receipts_out);
        assert_eq!(tally.kept_visible_unanchored, 2);
        assert_eq!(tally.occurrences_held, 2);
    }

    /// A later committed paint keeps unanchored evidence that no committed
    /// token covers. Delivery stays the committed words.
    #[tokio::test]
    async fn later_committed_paint_keeps_uncovered_unanchored_evidence() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = Arc::clone(&paints);
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let session = "overlap-paint";
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session.to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                temp.path().join("paint.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = super::PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            None,
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed.lock().unwrap().push(projection.text.clone());
        }));
        emitter.on_capture_opened(session, 1);
        let beta = OccurrenceIdentity::new(session, 1, 24_000, 48_000);
        let gamma = OccurrenceIdentity::new(session, 1, 48_000, 72_000);
        let delta = OccurrenceIdentity::new(session, 1, 80_000, 96_000);
        for (request, occurrence, label) in [(1, beta.clone(), "beta"), (2, gamma.clone(), "gamma")]
        {
            let event = {
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                admitted_mutation(&mut ledger, occurrence, request, label)
            };
            emitter.on_event(&event);
        }
        let straddling = OccurrenceIdentity::new(session, 1, 40_000, 56_000);
        let straddle_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, straddling);
        let straddle = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            ledger.admit(&straddle_observation, "przez granice")
        };
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation: straddle_observation,
            label: "przez granice".into(),
            receipt: straddle,
        });
        let later = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            admitted_mutation(&mut ledger, delta, 4, "delta")
        };
        emitter.on_event(&later);
        let painted = paints.lock().unwrap().last().cloned().unwrap_or_default();
        assert!(
            painted.split_whitespace().any(|word| word == "przez"),
            "later paint dropped uncovered evidence: {painted}"
        );
        emitter.finish().await;
        let delivered = delivery.lock().await.clone();
        assert!(
            !delivered.split_whitespace().any(|word| word == "przez"),
            "delivery must stay committed-only, got {delivered}"
        );
        assert!(delivered.split_whitespace().any(|word| word == "delta"));
    }

    /// Counterexample A (Roman, 2026-09-24): a refused whole-span Whisper
    /// replacement keeps its exclusive text as `KeepVisibleUnanchored` wholly
    /// inside the Apple occurrence. The text differs from the Apple label, so
    /// hiding it is not duplicate suppression — the receipt exists and the
    /// words must reach a paint. They never reach the canvas string, the Bus,
    /// or delivery.
    #[tokio::test]
    async fn refused_whole_span_whisper_text_reaches_a_paint_as_evidence() {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;

        let session = "refused-span";
        let whisper = "Whisper mówi inaczej";
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let deltas = Arc::new(RecordingDeltaSink::default());
        let projected = Arc::new(StdMutex::new(Vec::<TranscriptBusEvidenceEvent>::new()));
        let projected_for_callback = Arc::clone(&projected);
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("refused-span.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session.to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = super::PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            Some(Arc::clone(&deltas) as Arc<dyn DeltaSink>),
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event: &TranscriptBusEvidenceEvent| {
                projected_for_callback.lock().unwrap().push(event.clone());
            })),
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed.lock().unwrap().push(projection.clone());
        }));
        emitter.on_capture_opened(session, 1);
        let apple = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let committed = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(&mut ledger, apple.clone(), 1, "Apple mówi tak")
        };
        emitter.on_event(&committed);
        let pin = OccurrenceIdentity::new(session, 1, 16_000, 32_000);
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 2, 1_000, pin.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            whisper,
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: whisper.into(),
            receipt,
        });
        emitter.finish().await;

        let compact = paints
            .lock()
            .unwrap()
            .iter()
            .map(|paint| serde_json::to_string(paint).unwrap())
            .collect::<Vec<_>>();
        let painted = deltas
            .deltas
            .lock()
            .unwrap()
            .iter()
            .map(|delta| delta.delta.clone())
            .collect::<String>();
        assert!(
            compact.iter().any(|json| json.contains(whisper)),
            "the refused Whisper text never reached a paint.\ncompact paints: {compact:#?}\n\
             canvas deltas: {painted:?}"
        );
        assert!(
            !painted.contains(whisper),
            "evidence must stay off the canvas string: {painted:?}"
        );
        assert!(
            projected
                .lock()
                .unwrap()
                .iter()
                .all(|event| !event.rendered_text.contains(whisper)),
            "evidence must never reach a Bus projection"
        );
        let bus_bytes = std::fs::read_to_string(&bus_path).unwrap();
        assert!(
            !bus_bytes.contains(whisper),
            "evidence entered the Bus file"
        );
        assert_eq!(delivery.lock().await.as_str(), "Apple mówi tak");
        assert_eq!(
            ledger.lock().unwrap().text_of(&apple),
            Some("Apple mówi tak")
        );

        let last = paints.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.evidence,
            vec![super::UnanchoredEvidence {
                sample_start: 16_000,
                sample_end: 32_000,
                text: whisper.into(),
                reason: "exclusive_tail_awaiting_whole_span".into(),
            }],
            "the evidence names its PCM range and why it has no authority"
        );
        assert_eq!(
            last.text, "Apple mówi tak",
            "the canvas tail stays canvas-only"
        );
    }

    /// Qualify, admit and hand one Apple occurrence to `emitter`.
    fn admit_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        occurrence: &OccurrenceIdentity,
        request: u64,
        label: &str,
    ) {
        let event = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(&mut ledger, occurrence.clone(), request, label)
        };
        emitter.on_event(&event);
    }

    /// Close the Apple frontier, seal the occurrence, deliver the receipt.
    fn seal_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        occurrence: &OccurrenceIdentity,
    ) {
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
            ledger
                .seal(occurrence)
                .expect("closed qualified occurrence")
                .clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
    }

    /// Keep one Whisper pin visible without authority and hand it to `emitter`.
    fn keep_visible_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        pin: &OccurrenceIdentity,
        request: u64,
        text: &str,
    ) {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, request, 1_000, pin.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            text,
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: text.into(),
            receipt,
        });
    }

    /// Evidence survives later L0 and committed paints. A seal of a token that
    /// does not cover it leaves it; the seal of the token over its range
    /// closes it. Evidence no token covers stays until the lifecycle ends.
    #[tokio::test]
    async fn unanchored_evidence_lives_until_a_sealed_token_covers_it_or_the_session_ends() {
        let session = "take";
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session.to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                temp.path().join("evidence-life.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = super::PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            None,
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed.lock().unwrap().push(projection.clone());
        }));
        // Literal takes mint no live shape, so no committed paint follows a
        // seal: the evidence the seal closes must leave the paint on its own.
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened(session, 7);
        let evidence_texts = || {
            paints
                .lock()
                .unwrap()
                .last()
                .map(|paint| {
                    paint
                        .evidence
                        .iter()
                        .map(|item| item.text.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let apple = OccurrenceIdentity::new(session, 7, 0, 48_000);
        let later = OccurrenceIdentity::new(session, 7, 96_000, 112_000);
        admit_into(&emitter, &ledger, &apple, 1, "pierwsze zdanie");
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new(session, 7, 16_000, 32_000),
            2,
            "inna wersja",
        );
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new(session, 7, 60_000, 72_000),
            3,
            "między tokenami",
        );
        assert_eq!(evidence_texts(), ["inna wersja", "między tokenami"]);

        emitter.on_event(&preview(1, "dalej mówię"));
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "an L0 paint keeps the evidence"
        );
        admit_into(&emitter, &ledger, &later, 4, "drugie zdanie");
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "a committed paint keeps the evidence"
        );
        seal_into(&emitter, &ledger, &later);
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "a seal elsewhere does not close it"
        );
        seal_into(&emitter, &ledger, &apple);
        assert_eq!(
            evidence_texts(),
            ["między tokenami"],
            "the seal of the token over its range closes it"
        );

        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: session.into(),
            layer_summary: LayerSummary::default(),
        });
        assert!(evidence_texts().is_empty(), "the lifecycle end closes it");
        emitter.finish().await;
        let delivered = delivery.lock().await.clone();
        assert!(
            !delivered.contains("inna") && !delivered.contains("między"),
            "evidence never reaches delivery: {delivered}"
        );
    }

    /// Each L0 paint leaves one diagnostic line naming rev, PCM range, grain
    /// and receipt, so a take traces partial -> paint. The words stay out.
    #[tokio::test]
    async fn preview_paint_logs_one_trace_line_without_its_words() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("preview.log");
        let log_file = std::fs::File::create(&log_path).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || log_file.try_clone().unwrap())
            .finish();
        let emitter =
            super::PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None);
        tracing::subscriber::with_default(subscriber, || {
            emitter.on_event(&EngineEvent::Preview {
                rev: 3,
                text: "tajne słowa".into(),
                pin: PreviewPin::from_segments(TailSampleRange {
                    session: "traced".into(),
                    capture_epoch: 2,
                    sample_start: 4_000,
                    sample_end: 12_000,
                }),
            });
        });
        let log = std::fs::read_to_string(log_path).unwrap();
        let lines = log
            .lines()
            .filter(|line| line.contains("L0 preview painted"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1, "one line per preview: {log}");
        for field in [
            "rev=3",
            "session=traced",
            "capture_epoch=2",
            "sample_start=4000",
            "sample_end=12000",
            "grain=Word",
            "segments_on_capture_clock",
        ] {
            assert!(lines[0].contains(field), "missing {field}: {}", lines[0]);
        }
        assert!(!log.contains("tajne"), "preview words must not be logged");
    }

    #[tokio::test]
    async fn collapsed_preview_is_read_only_evidence_beside_the_canvas() {
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let delivery = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(Arc::clone(&delivery), None, None)
            .with_cursor_observer(Arc::new(move |projection| {
                observed.lock().unwrap().push(projection.clone());
            }));
        emitter.on_capture_opened("collapsed", 3);
        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "preview only".to_string(),
            pin: PreviewPin::from_segments(TailSampleRange {
                session: "collapsed".to_string(),
                capture_epoch: 3,
                sample_start: 400,
                sample_end: 400,
            }),
        });
        let paint = paints.lock().unwrap().last().cloned().unwrap();
        assert!(paint.text.is_empty());
        assert_eq!(paint.evidence.len(), 1);
        assert_eq!(paint.evidence[0].text, "preview only");
        assert_eq!(paint.evidence[0].reason, "unanchored_zero_width");
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.text, "preview only");
        assert_eq!(frozen.preview_only_words, 2);
        assert_eq!(frozen.text.split_whitespace().count(), 2);
        assert!(!frozen.has_committed_document);
        assert_eq!(emitter.begin_stop_canvas().unwrap(), frozen);
        emitter.on_event(&EngineEvent::NoSpeech {
            reason: "lane lost".into(),
        });
        assert_eq!(emitter.finish_stop_canvas().unwrap(), frozen);
        emitter.finish().await;
        assert!(delivery.lock().await.is_empty());
    }

    /// Evidence painted under one capture never carries another capture's
    /// ranges: the range would name different audio.
    #[tokio::test]
    async fn evidence_paint_is_bound_to_the_opened_capture() {
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let emitter = super::PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed.lock().unwrap().push(projection.clone());
        }));
        emitter.on_capture_opened("take", 7);
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new("take", 8, 0, 16_000),
            1,
            "inna epoka",
        );
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new("take", 7, 0, 16_000),
            2,
            "ta epoka",
        );
        let last = paints.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.evidence
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["ta epoka"]
        );
    }
}
