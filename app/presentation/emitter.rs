//! Event-driven presentation emitter.
//!
//! Converts `EngineEvent`s into user-facing output through the one canonical
//! reducer and an ordered delta-delivery worker.
//!
//! Uses an ordered mpsc channel to guarantee that target updates and finish
//! arrive in the exact order they were emitted,
//! eliminating the fire-and-forget tokio::spawn ordering race.

use std::{collections::BTreeMap, io::Write as _, sync::Arc};

use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, DocumentRevisionProvenance, IncrementalShapingInput,
    IncrementalShapingReceipt, LedgerSealReceipt, ManualDocumentRevisionReceipt, MutationReceipt,
    ObservationIdentity, ObservationProducer, OccurrenceIdentity, SealCoverageReceipt,
    TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::{
    DeltaSink, EngineEvent, EventSink, SpeechIntegrity, SpeechIntegrityPhase, TranscriptDelta,
};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::transcript_bus::{TranscriptBus, TranscriptBusEvidenceEvent};

/// Read-only cursor paint observer: bounded text and acoustic warning state.
pub type CursorObserver = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// Commands sent through the ordered channel to the emitter worker.
enum EmitterCmd {
    /// Publish ledger-authenticated text to both overlay paint and delivery.
    PublishCommittedRevision(String),
    /// Paint volatile text without touching delivery or any committed sink.
    PaintEphemeralPreview(String),
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
            let presentation = if let Some(receipt) = &entry.presentation_receipt {
                if receipt.occurrence != entry.occurrence
                    || receipt.session_id != session
                    || receipt.source_label != entry.label
                    || receipt.revision > self.revision
                    || receipt.source_revision.checked_add(1) != Some(receipt.revision)
                    || receipt.left_context != left_context
                    || receipt.left_context_sha256
                        != format!("{:x}", Sha256::digest(left_context.as_bytes()))
                    || !ledger.incremental_shapings().contains(receipt)
                    || ledger
                        .seal_of(&entry.occurrence)
                        .is_none_or(|seal| seal.receipt_id != receipt.source_seal_receipt)
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
    revision: u64,
    ephemeral_preview: String,
    latest_seal_coverage: Option<SealCoverageReceipt>,
    latest_comparison: Option<TranscriptComparisonReceipt>,
    context_markers: Vec<DocumentContextMarker>,
    manual_rendered_text: Option<String>,
    manual_document_revision_receipt: Option<String>,
    terminal: bool,
    observed_seals: std::collections::BTreeSet<String>,
    applied_observations: Vec<ObservationIdentity>,
}

/// Preserve fragment bytes; only an absent inter-occurrence separator is added.
fn append_exact_fragment(rendered: &mut String, fragment: &str) {
    if !rendered.is_empty()
        && !fragment.is_empty()
        && !rendered.ends_with(char::is_whitespace)
        && !fragment.starts_with(char::is_whitespace)
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
            publication_digest: [0; 32],
        };
        snapshot.publication_digest = snapshot.digest();
        snapshot
    }

    /// Apply only the mutation authority granted by the shared ledger. An
    /// unsigned or unqualified occurrence fails closed and creates no document
    /// entry, even when an engine supplied visible text.
    pub fn apply_ledger_mutation(
        &mut self,
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        receipt: &MutationReceipt,
    ) -> Option<TranscriptRevision> {
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
        let occurrence = receipt.sealed_occurrences.first()?.clone();
        self.observed_seals.insert(receipt.receipt_id.clone());
        let terminal = !receipt.is_occurrence_seal();
        if terminal {
            self.terminal = true;
        }
        Some(self.revision_for_action(ReducerAction::RecordLedgerSeal {
            occurrence,
            seal_receipt: receipt.receipt_id.clone(),
            terminal: !receipt.is_occurrence_seal(),
        }))
    }

    /// Commit a whole-document user edit without fabricating per-word acoustic
    /// ownership. The ledger authenticates the exact sealed source occurrence
    /// set, then this reducer mints the only new document revision.
    pub fn apply_user_revision(
        &mut self,
        ledger: &mut AcousticLedger,
        intent: &UserRevisionIntent,
    ) -> Result<TranscriptRevision, UserRevisionRefusal> {
        if intent.rendered_text.trim().is_empty() {
            return Err(UserRevisionRefusal::EmptyText);
        }
        let source_occurrences =
            self.authenticated_revision_occurrences(&intent.session_id, intent.source_revision)?;
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
        self.manual_document_revision_receipt = Some(receipt.receipt_id.clone());
        Ok(self.revision_for_action(ReducerAction::ApplyUserRevision { receipt }))
    }

    fn authenticated_revision_occurrences(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<Vec<OccurrenceIdentity>, UserRevisionRefusal> {
        if self.document_by_occurrence.is_empty() {
            return Err(UserRevisionRefusal::NoCommittedDocument);
        }
        if !self.terminal {
            return Err(UserRevisionRefusal::NotTerminal);
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

    /// Shape one closed occurrence's presentation while the session lifecycle
    /// is still open.
    ///
    /// This is the live half of the Light+ floor and it is deliberately narrow.
    /// It shapes exactly the occurrence a seal just closed, using the committed
    /// text to its left as casing context, and it authenticates that single
    /// occurrence through the ledger. It does not touch the ledger label, does
    /// not claim the document, does not read the ephemeral preview, and cannot
    /// make the reducer terminal — an occurrence seal is not a lifecycle end.
    ///
    /// An open suffix is therefore never shaped and never appears in a sealed
    /// source receipt; it keeps rendering the spoken words until its own seal
    /// arrives.
    pub fn apply_incremental_shaping(
        &mut self,
        ledger: &mut AcousticLedger,
        occurrence: &OccurrenceIdentity,
    ) -> Result<TranscriptRevision, IncrementalShapingRefusal> {
        // A whole-document revision (user edit, formatter, terminal Light+)
        // already owns every visible byte. A per-occurrence shape must not
        // fight it for the same document.
        if self.manual_rendered_text.is_some() {
            return Err(IncrementalShapingRefusal::DocumentRevisionOwnsPresentation);
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
        // Do not shape against a known open predecessor: it can still change.
        // On every closure the emitter revisits the complete sealed prefix.
        if self
            .document_by_occurrence
            .keys()
            .take_while(|key| *key < occurrence)
            .any(|key| !ledger.is_sealed(key))
        {
            return Err(IncrementalShapingRefusal::LedgerRefusal(
                "incremental_shaping_left_context_open",
            ));
        }
        let left_context = self.rendered_occurrence_span(Some(occurrence));
        let shaped = codescribe_core::pipeline::light_plus::apply_with_left_context(
            &left_context,
            &source_label,
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
        if !self.terminal {
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

    /// The Light+ revision this terminal document is owed, or `None` when
    /// there is nothing to shape: no committed document, not yet terminal, or
    /// the shaped text is byte-identical (Light+ is idempotent, so a second
    /// pass — or a document a formatter already shaped — mints nothing).
    /// Read-only: the intent enters the same corridor as a user edit.
    pub fn light_plus_intent(&self) -> Option<UserRevisionIntent> {
        if !self.terminal {
            return None;
        }
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
    /// Ledger seal scope independently opens terminal CAS before this event;
    /// cardinality has no finality meaning. Edit admission still checks seals.
    fn mark_terminal_lifecycle(&mut self) {
        self.terminal = true;
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
        let mut left = String::new();
        for (occurrence, entry) in &self.document_by_occurrence {
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
        let mut rendered = self.committed_rendered_text();
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

/// All target mutations are serialized through one mpsc worker, guaranteeing
/// that overlay deltas and the shared transcript snapshot see identical order.
pub struct PresentationEmitter {
    cursor_observer: Option<CursorObserver>,
    cursor_capture: std::sync::OnceLock<(String, u64)>,
    cursor_integrity: std::sync::Mutex<Option<SpeechIntegrity>>,
    /// Last bounded paint, not a document or independently reconstructed delta.
    cursor_tail: std::sync::Mutex<String>,
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
}

impl PresentationEmitter {
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
        let cmd_handle = Some(tokio::spawn(async move {
            let mut painted_text = String::new();
            while let Some(cmd) = rx.recv().await {
                let (target, commits_delivery) = match cmd {
                    EmitterCmd::PublishCommittedRevision(target) => (target, true),
                    EmitterCmd::PaintEphemeralPreview(target) => (target, false),
                    EmitterCmd::Finish => break,
                };
                if let Some(delta) = TranscriptDelta::from_diff(&painted_text, &target) {
                    if let Some(sink) = &delta_callback {
                        sink.apply(&delta);
                    }
                    if let Some(path) = stream_log_path.as_deref()
                        && let Err(error) = append_stream_delta(path, &delta.delta)
                    {
                        tracing::warn!(%error, path = %path.display(), "stream delta log append failed");
                    }
                }
                painted_text.clone_from(&target);
                if commits_delivery {
                    *transcript_buffer.lock().await = target;
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
            cursor_observer: None,
            cursor_capture: std::sync::OnceLock::new(),
            cursor_integrity: std::sync::Mutex::new(None),
            cursor_tail: std::sync::Mutex::new(String::new()),
        }
    }

    /// Declare the literal contract for this take. `true` is the Ctrl-hold
    /// `force_raw` lane: the terminal seal then delivers the ledger words
    /// untouched. Default `false`: Light+ shapes the terminal document.
    pub fn set_literal_delivery(&self, literal: bool) {
        self.literal_delivery
            .store(literal, std::sync::atomic::Ordering::SeqCst);
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
        observer(if degraded { "…" } else { &tail }, degraded);
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
    fn send_cmd(&self, cmd: EmitterCmd) {
        match &cmd {
            EmitterCmd::PublishCommittedRevision(text)
            | EmitterCmd::PaintEphemeralPreview(text) => self.paint_cursor(text),
            EmitterCmd::Finish => {}
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
                        callback(event);
                    }
                }
            }
        } else {
            return;
        }
        self.send_cmd(EmitterCmd::PublishCommittedRevision(revision.rendered_text));
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
                    callback(event);
                }
            }
        }
        self.send_cmd(EmitterCmd::PublishCommittedRevision(
            revision.rendered_text.clone(),
        ));
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

    /// Light+ floor at the terminal seal. Deterministic sentence shape for the
    /// sealed document — capital at sentence starts, a closing period,
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

    /// The live half of the Light+ floor. Every occurrence an arriving seal
    /// just closed gains its deterministic presentation immediately, so a long
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
            if !ledger.is_sealed(occurrence) {
                break;
            }
            let shaped = {
                let mut reducer = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                reducer.apply_incremental_shaping(ledger, occurrence)
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
                                callback(event);
                            }
                        }
                    }
                    self.send_cmd(EmitterCmd::PublishCommittedRevision(revision.rendered_text));
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
        let _ = self
            .cursor_capture
            .set((session_id.to_owned(), capture_epoch));
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
                let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .apply_ledger_mutation(&ledger, observation, receipt);
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
                                callback(event);
                            }
                        }
                    }
                    self.send_cmd(EmitterCmd::PublishCommittedRevision(revision.rendered_text));
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
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .apply_ledger_seal(receipt);
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
                            callback(event);
                        }
                    }
                }
                // The terminal seal closes the ledger's word authority; the
                // Light+ floor follows immediately, before the controller
                // publishes `session_ended`, so the terminal projection Swift
                // holds already carries the shaped bytes and revision number.
                if terminal {
                    self.mint_light_plus_revision(&mut ledger);
                } else {
                    // An occurrence seal closes exactly those words and nothing
                    // else. Shape them now: the closed part of the take becomes
                    // readable during capture, the open suffix keeps its spoken
                    // form, and the lifecycle stays open.
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
                            callback(event);
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
                let (proposal_revision, seal_revision) = {
                    let mut reducer = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let (formatter_returned, proposal_revision) =
                        reducer.apply_occurrence_label_proposal(&mut ledger, proposal);
                    let seal_revision = formatter_returned
                        .then(|| ledger.seal(&occurrence).ok().cloned())
                        .flatten()
                        .and_then(|receipt| reducer.apply_ledger_seal(&receipt));
                    (proposal_revision, seal_revision)
                };
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
                                callback(event);
                            }
                        }
                    }
                    if is_label_revision {
                        self.send_cmd(EmitterCmd::PublishCommittedRevision(revision.rendered_text));
                    }
                }
                // The proposal corridor is the second place an occurrence can
                // close. One law for both: a sealed occurrence is shaped. The
                // reducer refuses a still-open or already-shaped one, so this
                // cannot mint a second presentation for the same words.
                if ledger.is_sealed(&occurrence) {
                    self.mint_incremental_light_plus(
                        &mut ledger,
                        std::slice::from_ref(&occurrence),
                    );
                }
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
            EngineEvent::Preview { text, .. } => {
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
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
                    state.clear_ephemeral_preview();
                    state.committed_rendered_text()
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
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.clear_ephemeral_preview();
                    state.committed_rendered_text()
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
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
                    state.clear_ephemeral_preview();
                    state.committed_rendered_text()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                // Lifecycle end is the second Light+ gate: a one-occurrence
                // session's whole-session seal is indistinguishable from its
                // sole occurrence seal, so the reducer becomes terminal only
                // here. Idempotent — a document the terminal seal already
                // shaped yields no intent, so nothing is minted twice.
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
        AcousticEvidence, AcousticLedger, DocumentRevisionProvenance, EnergyCalibration,
        IncrementalShapingReceipt, ObservationIdentity, ObservationProducer, OccurrenceIdentity,
    };
    use codescribe_core::pipeline::contracts::{
        AnnotationKind, DeltaSink, EngineEvent, EventSink, LayerSource, LayerSummary,
        TranscriptDelta,
    };
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct RecordingDeltaSink {
        deltas: StdMutex<Vec<TranscriptDelta>>,
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
        .with_cursor_observer(Arc::new(move |text, amber| {
            observed.lock().unwrap().push((text.to_owned(), amber));
        }));
        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "zero jeden dwa trzy cztery pięć".into(),
        });
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("jeden dwa trzy cztery pięć".into(), false)
        );
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
            assert_eq!(paints.lock().unwrap().last().unwrap(), &("…".into(), true));
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

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "volatile words".to_string(),
        });
        emitter.finish().await;

        assert_eq!(delivery.lock().await.as_str(), "ledger truth");
        assert!(
            !deltas
                .deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
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

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "volatile".to_string(),
        });
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
        // No occurrence seal reached the emitter, so nothing was shaped live:
        // this document really was shaped once, at the terminal boundary.
        assert!(ledger.incremental_shapings().is_empty());
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
            "To jest pierwsze zdanie.",
            "the closed occurrence reaches delivery shaped, during capture"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1, "exactly one shaping for one seal");
        let shaping = &shapings[0];
        assert_eq!(shaping.provenance, "light-plus");
        assert_eq!(shaping.occurrence, occurrence);
        assert_eq!(shaping.source_label, spoken);
        assert_eq!(shaping.shaped_text, "To jest pierwsze zdanie.");
        assert_eq!(shaping.source_seal_receipt, seal_receipt_id);
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
        assert_eq!(projection.rendered_text, "To jest pierwsze zdanie.");
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
            Some(seal_receipt_id.as_str())
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
            open_projection.rendered_text, "To jest pierwsze zdanie. a to jest drugie",
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
            "To jest pierwsze zdanie. A to jest drugie.",
            "the second span capitalises because its left context closed"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 2);
        assert_eq!(
            shapings[0], after_first[0],
            "the first shaping receipt is never rewritten"
        );
        assert_eq!(shapings[1].occurrence, second);
        assert_eq!(shapings[1].shaped_text, "A to jest drugie.");
        assert_ne!(shapings[0].receipt_id, shapings[1].receipt_id);
        assert_ne!(
            shapings[0].left_context_sha256, shapings[1].left_context_sha256,
            "the second shape saw a different left neighbourhood"
        );
    }

    /// Acceptance: an unsealed suffix is never shaped and never appears in a
    /// sealed source receipt. Closed geometry and seal history stay immutable.
    #[tokio::test]
    async fn an_open_suffix_is_never_shaped_nor_claimed_as_a_sealed_source() {
        let mut take = live_take("open-suffix-session");
        let closed = OccurrenceIdentity::new("open-suffix-session", 13, 0, 16_000);
        let open = OccurrenceIdentity::new("open-suffix-session", 13, 16_000, 32_000);
        let open_words = "  jeszcze mowie i nie skonczylem\n\t";

        take.admit(&closed, 1, "pierwsza czesc juz zamknieta");
        take.seal(&closed);
        take.admit(&open, 2, open_words);
        take.emitter.finish().await;

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1, "only the sealed occurrence was shaped");
        assert_eq!(shapings[0].occurrence, closed);
        assert!(
            !shapings.iter().any(|shaping| shaping.occurrence == open),
            "an open occurrence must never enter a shaping receipt"
        );

        let rendered = take.delivery.lock().await.clone();
        assert!(
            rendered.ends_with(open_words),
            "the open suffix is delivered word-for-word: {rendered}"
        );
        assert!(rendered.starts_with("Pierwsza czesc juz zamknieta."));

        // The seal the shaping cites is still the one the ledger holds.
        let ledger = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            ledger.seal_of(&closed).map(|seal| seal.receipt_id.as_str()),
            Some(shapings[0].source_seal_receipt.as_str())
        );
        assert!(ledger.seal_of(&open).is_none());
    }

    /// Acceptance: equal words in distinct occurrences remain intentional
    /// repetition. Shaping is per-occurrence, so nothing can collapse them.
    #[tokio::test]
    async fn equal_words_in_distinct_occurrences_stay_two_shaped_entries() {
        let mut take = live_take("iwo-session");
        let first = OccurrenceIdentity::new("iwo-session", 14, 0, 16_000);
        let second = OccurrenceIdentity::new("iwo-session", 14, 16_000, 32_000);

        take.admit(&first, 1, "Iwo");
        take.seal(&first);
        take.admit(&second, 2, "Iwo");
        take.seal(&second);
        take.emitter.finish().await;

        assert_eq!(take.delivery.lock().await.as_str(), "Iwo. Iwo.");
        let shapings = take.shapings();
        assert_eq!(shapings.len(), 2, "same text, two physical events");
        assert_eq!(shapings[0].occurrence, first);
        assert_eq!(shapings[1].occurrence, second);
        assert_ne!(shapings[0].receipt_id, shapings[1].receipt_id);
        let rows = take.shaping_projections();
        assert_eq!(
            rows.len(),
            3,
            "one first-entry row, then two document-entry rows"
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.reducer_revision)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            2
        );
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
            "Raz powiedziane zdanie."
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

        take.admit(&occurrence, 1, "Iwo");
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
    /// UNRUN under W2; both receipt scope and pre-lifecycle CAS are exercised.
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

    /// Acceptance: Stop delivers the current canonical shaped document exactly
    /// once. A document that live shaping already settled mints no second
    /// whole-document Light+ revision, and the terminal CAS source is the same
    /// bytes Swift already holds.
    #[tokio::test]
    async fn stop_delivers_the_live_shaped_document_without_a_duplicate_revision() {
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

        let shaped = "Pierwsze zdanie tutaj. A potem drugie.";
        assert_eq!(take.delivery.lock().await.as_str(), shaped);
        assert_eq!(terminal.rendered_text, shaped);
        assert!(terminal.terminal);
        assert_eq!(
            take.shapings().len(),
            2,
            "two occurrence seals, two shapings"
        );
        assert!(
            take.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .is_empty(),
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
    fn foreign_unsealed_and_owned_documents_refuse_incremental_shaping() {
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

        // Committed but not sealed: the ledger, not the reducer, says no.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::LedgerRefusal(
                "incremental_shaping_occurrence_not_sealed"
            ))
        );

        // A foreign occurrence never reaches the ledger at all.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &stranger),
            Err(IncrementalShapingRefusal::UnknownOccurrence)
        );

        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        ledger.seal(&occurrence).expect("closed occurrence seals");
        let revision = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .expect("a sealed committed occurrence shapes");
        assert_eq!(revision.rendered_text, "Jakies slowa.");
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
            take.shapings().is_empty(),
            "known open left context cannot authorize casing"
        );
        take.seal(&first);
        take.emitter.finish().await;
        let receipts = take.shapings();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].occurrence, first);
        assert_eq!(receipts[1].occurrence, second);
        assert_eq!(receipts[1].left_context, "Pierwsze zdanie.");
        assert_eq!(
            receipts[1].left_context_sha256,
            format!("{:x}", Sha256::digest(b"Pierwsze zdanie."))
        );
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie. Drugie zdanie."
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
        assert_eq!(revision.rendered_text, "pierwsze zdanie drugie zdanie");
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
        assert_eq!(current.occurrence, second);
        assert_ne!(current.receipt_id, original.receipt_id);
        assert_eq!(current.left_context, "Pierwsze zdanie.");
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie. Drugie zdanie."
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
        assert_eq!(take.delivery.lock().await.as_str(), "Real words.");
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
        assert_eq!(take.delivery.lock().await.as_str(), "Real words.");
    }
}
