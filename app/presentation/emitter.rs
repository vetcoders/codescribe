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
use codescribe_core::config::FormattingPolicy;
use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, ConsultationPresentationInput, ConsultationPresentationReceipt,
    DocumentRevisionProvenance, IncrementalShapingInput, IncrementalShapingReceipt,
    LedgerSealReceipt, ManualDocumentCaptureReceipt, ManualDocumentRevisionReceipt,
    MutationReceipt, NoAuthorityReason, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity, RefuseReason, SealCoverageReceipt, SlotAlternative,
    TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::{
    ClosedApplePhrase, DeltaSink, EngineEvent, EventSink, SidebandEvidence, SidebandEvidenceKind,
    SpeechIntegrity, SpeechIntegrityPhase, TranscriptDelta, UnadmittedAppleWord,
    UnadmittedAppleWordSource,
};
use codescribe_core::pipeline::word_confidence::{
    UncertainSpan, WordConfidence, word_confidence_thresholds,
};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::transcript_bus::{TranscriptBus, TranscriptBusEvidenceEvent};

/// Live whole-document view of accepted formatter fragments. A child module of
/// the emitter so it reads the reducer's private state instead of copying it;
/// the file sits beside the emitter, hence the explicit path.
#[path = "formatter_live.rs"]
mod formatter_live;
#[cfg(test)]
#[path = "formatter_live_acceptance.rs"]
mod formatter_live_acceptance;

/// Read-only capture paint observer: full overlay text and acoustic warning state.
pub type CursorObserver = Arc<dyn Fn(&CompactProjection) + Send + Sync>;
/// Read-only, ephemeral Apple recognition paint; never a document mutation.
pub type ApplePreviewObserver = Arc<dyn Fn(&str) + Send + Sync>;
/// Read-only measured speech edges from this emitter's opened capture.
pub type SpeechActivityObserver = Arc<dyn Fn(&SidebandEvidence) + Send + Sync>;

/// UTF-16 range within ephemeral canvas text for an uncommitted (pending) fragment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingRange {
    pub utf16_start: u32,
    pub utf16_end: u32,
}

/// Passive paint from one opened capture. Sequence 1 binds the display before
/// any transcription event; later values replace paint, never document truth.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactProjection {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sequence: u64,
    /// Complete ephemeral canvas snapshot. Cursor surfaces bound this text at
    /// their own paint boundary; no consumer may use it as committed delivery.
    pub text: String,
    pub degraded: bool,
    /// Explicit in-progress text spans in UTF-16 coordinates within `text`.
    #[serde(default)]
    pub pending_ranges: Vec<PendingRange>,
    /// What the cursor dot says about the voice right now.
    #[serde(default)]
    pub voice: CursorVoice,
    /// Every unanchored text of this capture, in PCM order. It is painted
    /// beside the canvas, never inside the canvas string or the Bus. The
    /// Refused alternatives remain outside the stop canvas and delivery.
    pub evidence: Vec<UnanchoredEvidence>,
}

/// The voice under the cursor, from the session's one Silero observer.
/// Paint only; no state grants transcript or delivery authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorVoice {
    /// No speech observer this capture: the dot keeps its mode color.
    #[default]
    Unobserved,
    /// Silero hears speech and the words keep up.
    Speaking,
    /// Silero hears no speech and nothing is owed.
    Quiet,
    /// Speech outran its words past the stall bound, or the ledger owes text.
    Lagging,
}

impl CursorVoice {
    /// One reading of the latest integrity evidence. Owed text outranks speech.
    pub fn from_integrity(integrity: Option<&SpeechIntegrity>) -> Self {
        let Some(integrity) = integrity else {
            return Self::Unobserved;
        };
        match integrity.phase {
            SpeechIntegrityPhase::Unavailable => Self::Unobserved,
            SpeechIntegrityPhase::Stalled
            | SpeechIntegrityPhase::Recovering
            | SpeechIntegrityPhase::Unresolved => Self::Lagging,
            SpeechIntegrityPhase::Listening | SpeechIntegrityPhase::Tracking => {
                if integrity.speech_live {
                    Self::Speaking
                } else {
                    Self::Quiet
                }
            }
        }
    }
}

/// Read-only text the ledger kept visible without mutation authority,
/// anchored to its PCM range on the capture clock. `reason` is the ledger's
/// no-authority or slot-alternative reason. Its text is never compared with
/// the canvas: a differing alternative wholly inside a committed token is shown, not
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
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
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
    /// A6 per-word acoustic confidence evidence, in composition-token order.
    /// Structured so the projection locates uncertain words without reparsing
    /// receipt strings. Carries only tokens whose producer supplied a metric.
    pub word_confidence: Vec<WordConfidenceEvidence>,
}

/// A6: one composition token's acoustic confidence evidence. `token` is a
/// label used only to locate the word inside its entry's label, in order —
/// never an identity key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordConfidenceEvidence {
    pub token: String,
    pub token_sample_start: Option<u64>,
    pub token_sample_end: Option<u64>,
    pub producer: ObservationProducer,
    pub confidence: WordConfidence,
    pub surface_rewritten: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentContextMarker {
    position: usize,
    label: String,
    order: usize,
}

/// Per-take transcription track switches, frozen at capture open from the
/// immutable runtime snapshot. Both lanes default on; the direct publication
/// lane exists only while `seal_ledger_enabled` is false, and the Apple live
/// lane reaches paint and admission only while `apple_live_enabled` is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptionControls {
    pub seal_ledger_enabled: bool,
    pub apple_live_enabled: bool,
}

impl Default for TranscriptionControls {
    fn default() -> Self {
        Self {
            seal_ledger_enabled: true,
            apple_live_enabled: true,
        }
    }
}

/// One decoder observation accepted into the ledger-free document. Identity is
/// the PCM range plus request/generation of the hypothesis; `final_chunk`
/// pins the range against later equal-or-lower-priority rewrites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectObservation {
    pub observation: ObservationIdentity,
    pub text: String,
    pub final_chunk: bool,
}

/// Reducer-minted authority for ledger-free ("direct") publication.
///
/// Only [`TranscriptReducer`] can mint one: the constructor and the digest
/// stay private to this module, so no engine, Bus, bridge, or Swift caller
/// can fabricate document authority while the seal ledger is opted out. The
/// Bus verifies the digest and the exact observation list before persisting
/// a single row; a capability is not a ledger receipt and claims none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectPublicationCapability {
    pub(crate) session_id: String,
    pub(crate) capture_epoch: u64,
    /// Reducer revision this capability authorizes; monotonic per take.
    pub(crate) revision: u64,
    /// The complete accepted document at `revision`, in PCM occurrence order.
    pub(crate) observations: Vec<DirectObservation>,
    pub(crate) rendered_text: String,
    /// Decoder completion or an explicit operation continuing that completed document.
    pub(crate) terminal: bool,
    /// Explicit operation on the completed direct document: source CAS and provenance.
    /// Decoder observations never carry this authority.
    pub(crate) document_revision: Option<(u64, DocumentRevisionProvenance)>,
    digest: [u8; 32],
}

impl DirectPublicationCapability {
    fn document_revision_receipt_id(
        provenance: DocumentRevisionProvenance,
        session_id: &str,
        revision: u64,
    ) -> String {
        format!("{}-direct-{session_id}-{revision}", provenance.as_str())
    }

    /// Document provenance backed by this reducer capability, never an
    /// acoustic claim. The Bus calls this only after authenticating the full
    /// publication; decoder observations have no document-operation receipt.
    pub(crate) fn document_revision_receipt(&self) -> Option<ManualDocumentRevisionReceipt> {
        let (source_revision, provenance) = self.document_revision?;
        Some(ManualDocumentRevisionReceipt {
            receipt_id: Self::document_revision_receipt_id(
                provenance,
                &self.session_id,
                self.revision,
            ),
            provenance: provenance.as_str().to_string(),
            session_id: self.session_id.clone(),
            source_revision,
            revision: self.revision,
            source_occurrences: Vec::new(),
            source_seal_receipts: Vec::new(),
            rendered_text: self.rendered_text.clone(),
            capture_epoch: Some(self.capture_epoch),
            capture_receipt_id: Some(format!("direct-publication-{}", hex::encode(self.digest))),
        })
    }

    fn mint(
        session_id: &str,
        capture_epoch: u64,
        revision: u64,
        observations: Vec<DirectObservation>,
        rendered_text: String,
        terminal: bool,
    ) -> Self {
        let mut capability = Self {
            session_id: session_id.to_string(),
            capture_epoch,
            revision,
            observations,
            rendered_text,
            terminal,
            document_revision: None,
            digest: [0; 32],
        };
        capability.digest = capability.compute_digest();
        capability
    }

    fn compute_digest(&self) -> [u8; 32] {
        Sha256::digest(
            format!(
                "{:?}",
                (
                    "codescribe.direct-publication.v1",
                    &self.session_id,
                    self.capture_epoch,
                    self.revision,
                    &self.observations,
                    &self.rendered_text,
                    self.terminal,
                    &self.document_revision,
                )
            )
            .as_bytes(),
        )
        .into()
    }

    /// Verify this capability authorizes exactly `revision` for `session`.
    pub(crate) fn verifies(&self, session: &str, revision: &TranscriptRevision) -> bool {
        self.digest == self.compute_digest()
            && self.session_id == session
            && self.revision == revision.revision
            && self.rendered_text == revision.rendered_text
            && revision.entries.len() == self.observations.len()
            && revision
                .entries
                .iter()
                .zip(&self.observations)
                .all(|(entry, observation)| {
                    entry.occurrence == observation.observation.occurrence
                        && entry.label == observation.text
                        && entry.occurrence.capture_epoch == self.capture_epoch
                })
    }
}

/// A reducer-authorized document transition. Ledger receipts authorize the
/// ledger lane; `ApplyDirectTranscript` is authorized by the reducer-minted
/// [`DirectPublicationCapability`] instead. Every variant names a physical
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
        word_finality:
            Vec<codescribe_core::pipeline::acoustic_ledger::word_adjudication::WordFinality>,
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
    /// Ledger-free document transition authorized by the reducer's own direct
    /// capability, minted only while the take opted out of seal-ledger
    /// admission. The capability carries the PCM-range/request/generation
    /// identity of every accepted observation; no ledger receipt is claimed.
    ApplyDirectTranscript {
        capability: DirectPublicationCapability,
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
    /// A6 uncertain-word spans over `rendered_text`, computed by the reducer
    /// from ledger-pinned per-word confidence (never from text matching).
    /// Empty for occurrences whose presentation is shaped, consulted, or
    /// manually revised (d6), and for words with no metric (never "low").
    pub uncertain_spans: Vec<UncertainSpan>,
    // In-process reducer capability, never deserialized or exported as acoustic
    // evidence. Detects edits to any public snapshot field before publication.
    publication_digest: [u8; 32],
}

impl TranscriptRevision {
    fn record_trail(&self) {
        if let Some(first) = self.entries.first() {
            let (action, receipts) = match &self.action {
                ReducerAction::ApplyLedgerDecision { entry } => (
                    "ledger",
                    entry
                        .layer_decision_receipts
                        .last()
                        .cloned()
                        .into_iter()
                        .collect(),
                ),
                ReducerAction::ApplyUserRevision { .. } => ("document_revision", Vec::new()),
                ReducerAction::ApplyConsultationPresentation { .. } => ("consultation", Vec::new()),
                ReducerAction::ApplyManualEdit { .. } => ("manual_edit", Vec::new()),
                ReducerAction::ApplyIncrementalShaping { .. } => ("light_plus", Vec::new()),
                ReducerAction::RecordLedgerSeal { .. } => ("seal", Vec::new()),
                ReducerAction::RecordSealCoverage { .. } => ("coverage", Vec::new()),
                ReducerAction::RecordContextMarker { .. } => ("context", Vec::new()),
                ReducerAction::ApplyDirectTranscript { .. } => ("direct", Vec::new()),
            };
            codescribe_core::pipeline::trail::record_projection(
                &first.occurrence,
                self.revision.saturating_sub(1),
                self.revision,
                action,
                receipts,
            );
        }
    }

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
                    &self.uncertain_spans,
                )
            )
            .as_bytes(),
        )
        .into()
    }

    /// Validate the complete reducer-minted snapshot before any observer or
    /// delivery side effect. The Bus calls this; it never reconstructs text.
    pub(crate) fn authenticates_publication(&self, ledger: &AcousticLedger, session: &str) -> bool {
        if self.publication_digest != self.digest() {
            return false;
        }
        if self.entries.is_empty() {
            return match &self.action {
                ReducerAction::ApplyUserRevision { receipt } => {
                    ledger.authenticates_manual_document_revision(receipt)
                        && receipt.source_occurrences.is_empty()
                        && receipt.session_id == session
                        && receipt.revision == self.revision
                        && receipt.source_revision.checked_add(1) == Some(self.revision)
                        && receipt.rendered_text == self.rendered_text
                        && self.consultation_presentations.is_empty()
                        && self.uncertain_spans.is_empty()
                        && self.seal_coverage.as_ref() == ledger.latest_seal_coverage()
                }
                _ => false,
            };
        }
        if self
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
                        != hex::encode(Sha256::digest(left_context.as_bytes()))
                    || !ledger.authenticates_incremental_shaping(receipt)
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
                word_finality,
            } => ledger.seal_receipt(seal_receipt).is_some_and(|seal| {
                seal.word_finality == *word_finality
                    && seal.is_occurrence_seal() != *terminal
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
                ledger.authenticates_manual_document_revision(receipt)
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

    /// Empty coverage updates carry source CAS and diagnostics, never a document.
    pub(crate) fn authenticates_capture_revision(
        &self,
        ledger: &AcousticLedger,
        session: &str,
    ) -> bool {
        let ReducerAction::RecordSealCoverage { receipt, .. } = &self.action else {
            return false;
        };
        self.publication_digest == self.digest()
            && self.entries.is_empty()
            && self.rendered_text.is_empty()
            && receipt.session_id == session
            && ledger.has_manual_document_capture(session, receipt.capture_epoch)
            && ledger.latest_seal_coverage() == Some(receipt)
            && self.seal_coverage.as_ref() == Some(receipt)
    }

    /// Validate a ledger-free revision against the capability the reducer minted
    /// inside its action. Returns the verified capability for the publisher.
    /// No ledger is consulted: authority here is the digest-bound capability,
    /// the exact observation list, and the monotonic revision — nothing else.
    pub(crate) fn authenticates_direct_publication(
        &self,
        session: &str,
    ) -> Option<&DirectPublicationCapability> {
        if self.publication_digest != self.digest() {
            return None;
        }
        let ReducerAction::ApplyDirectTranscript { capability } = &self.action else {
            return None;
        };
        if (self.entries.is_empty() && (!capability.terminal || !self.rendered_text.is_empty()))
            || self
                .entries
                .windows(2)
                .any(|pair| pair[0].occurrence >= pair[1].occurrence)
        {
            return None;
        }
        // A direct take has no ledger coverage verdict or consultation group;
        // carrying one would be a second, foreign authority over this document.
        if self.seal_coverage.is_some()
            || self.comparison.is_some()
            || !self.consultation_presentations.is_empty()
        {
            return None;
        }
        if self.entries.iter().any(|entry| {
            entry.seal_receipt.is_some()
                || entry.manual_edit_receipt.is_some()
                || entry.presentation_receipt.is_some()
        }) {
            return None;
        }
        capability.verifies(session, self).then_some(capability)
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

/// A reducer-owned version derived from Raw. Its counter is independent from
/// the Raw document revision, and it never grants acoustic mutation authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DerivedTranscriptProjection {
    pub schema: String,
    pub emitted_at: String,
    pub session_id: String,
    pub revision: u64,
    pub source_raw_revision: u64,
    pub source_raw_text: String,
    pub source_state: String,
    pub requested_mode: String,
    pub delivered_mode: String,
    pub rendered_text: String,
    pub status: String,
    pub receipt_id: String,
    pub source_pcm: Option<(u64, u64, u64)>,
    pub source_observation_id: Option<(String, u64, u64)>,
    #[serde(skip)]
    source_observation: Option<ObservationIdentity>,
    #[serde(skip)]
    publication_digest: [u8; 32],
}

impl DerivedTranscriptProjection {
    fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(
            serde_json::to_vec(self).expect("derived receipt contains only serializable values"),
        );
        digest.update(format!("{:?}", self.source_observation).as_bytes());
        digest.finalize().into()
    }

    pub(crate) fn authenticates_publication(&self) -> bool {
        self.publication_digest == self.digest()
    }
}

/// One accepted operation on a terminal take's document, as Undo/Redo and the
/// versions picker see it. The step keeps the exact bytes it accepted, so
/// navigation re-selects them and never reruns a transcription or formatter.
/// Two attempts with equal bytes stay two steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentVersionStep {
    /// `raw`, `light-plus`, `user-edit`, `retranscribe` or `formatter`.
    pub provenance: String,
    /// Formatter mode of a formatted step.
    pub detail: Option<String>,
    /// Receipt of the operation that produced this step.
    pub receipt_id: String,
    pub emitted_at: String,
    /// Bytes the canvas shows, Copy/Insert deliver and Format reads while this
    /// step is selected.
    pub rendered_text: String,
    /// Committed document the step stands on.
    raw_text: String,
    raw_human_revision: bool,
    /// Accepted formatter result presented over `raw_text`, if any.
    presentation: Option<DerivedTranscriptProjection>,
}

/// Linear operation history of one take and the user's position in it. The
/// reducer revision counts receipts (navigation mints one too); the cursor is
/// where the user stands. A new operation after Undo drops the redo branch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentTimeline {
    steps: Vec<DocumentVersionStep>,
    cursor: usize,
}

impl DocumentTimeline {
    pub fn steps(&self) -> &[DocumentVersionStep] {
        &self.steps
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn current(&self) -> Option<&DocumentVersionStep> {
        self.steps.get(self.cursor)
    }
}

/// What one whole-document request asks the reducer corridor to do.
enum DocumentRequest {
    Revise(UserRevisionIntent),
    Navigate {
        session_id: String,
        source_revision: u64,
        target: usize,
    },
    /// An applied formatter result for the version selected at
    /// `source_revision`. Accepting it moves the cursor to a new step and
    /// mints the receipt that move needs, so a second result naming the same
    /// revision is stale.
    Format {
        session_id: String,
        source_revision: u64,
        policy: FormattingPolicy,
        result: AiFormatResult,
    },
}

impl DocumentRequest {
    fn session_id(&self) -> &str {
        match self {
            Self::Revise(intent) => &intent.session_id,
            Self::Navigate { session_id, .. } | Self::Format { session_id, .. } => session_id,
        }
    }

    fn source_revision(&self) -> u64 {
        match self {
            Self::Revise(intent) => intent.source_revision,
            Self::Navigate {
                source_revision, ..
            }
            | Self::Format {
                source_revision, ..
            } => *source_revision,
        }
    }

    fn provenance(&self) -> DocumentRevisionProvenance {
        match self {
            Self::Revise(intent) => intent.provenance,
            // A format leaves the committed bytes alone; its receipt records
            // the cursor moving onto the new formatted step.
            Self::Navigate { .. } | Self::Format { .. } => DocumentRevisionProvenance::Navigation,
        }
    }
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

/// One remembered Light+ evaluation.
///
/// The key is the occurrence itself plus every shaping input stored beside it:
/// source label, exact left-context digest, sentence break, and pause bits.
/// `shaped_text` is the computational result. Nothing here is authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShapingEvaluation {
    occurrence: OccurrenceIdentity,
    source_label: String,
    left_context_sha256: [u8; 32],
    sentence_break_before: bool,
    sentence_pause_bits: u32,
    shaped_text: String,
}

fn shaping_context_sha256(left_context: &str) -> [u8; 32] {
    let digest = Sha256::digest(left_context.as_bytes());
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

/// Shared document fields of one reducer revision's occurrence fanout.
/// Per-occurrence identity stays out of this comparison so every receipt can
/// be carried on one snapshot. A lifecycle, delivery, or derived difference
/// refuses the snapshot and keeps each event.
fn same_overlay_document_fanout(
    anchor: &TranscriptBusEvidenceEvent,
    event: &TranscriptBusEvidenceEvent,
) -> bool {
    !event.lifecycle_terminal
        && event.schema == anchor.schema
        && event.session_id == anchor.session_id
        && event.mode == anchor.mode
        && event.reducer_revision == anchor.reducer_revision
        && event.reducer_action == anchor.reducer_action
        && event.rendered_text == anchor.rendered_text
        && event.delivery_text == anchor.delivery_text
        && event.phase == anchor.phase
        && event.can_paste == anchor.can_paste
        && event.can_insert == anchor.can_insert
        && event.can_copy == anchor.can_copy
        && event.can_retranscribe == anchor.can_retranscribe
        && event.can_format == anchor.can_format
        && event.can_send_to_agent == anchor.can_send_to_agent
        && event.terminal == anchor.terminal
        && event.lifecycle_terminal == anchor.lifecycle_terminal
        && event.delivery == anchor.delivery
        && event.audience == anchor.audience
        && event.document_revision_receipt == anchor.document_revision_receipt
        && event.seal_coverage == anchor.seal_coverage
        && event.comparison == anchor.comparison
        && event.consultation_presentations == anchor.consultation_presentations
        && event.uncertain_spans == anchor.uncertain_spans
}

/// One overlay snapshot for a same-revision occurrence fanout.
///
/// The last row keeps the capture epoch and sequence the overlay already
/// accepts. Every row's acoustic receipt is concatenated in document order,
/// so no occurrence's capture, session, or receipt chain is dropped.
/// Rows whose sequence does not strictly advance, or whose capture epoch
/// moves backward, stay individual events.
fn coalesced_overlay_document(
    events: &[TranscriptBusEvidenceEvent],
) -> Option<TranscriptBusEvidenceEvent> {
    let first = events.first()?;
    if events.len() < 2 || first.lifecycle_terminal {
        return None;
    }
    let mut previous = first;
    for event in events.iter().skip(1) {
        if !same_overlay_document_fanout(first, event)
            || event.sequence <= previous.sequence
            || event.capture_epoch < previous.capture_epoch
        {
            return None;
        }
        previous = event;
    }
    let mut snapshot = events.last()?.clone();
    snapshot.acoustic_receipts = events
        .iter()
        .flat_map(|event| event.acoustic_receipts.iter().cloned())
        .collect();
    Some(snapshot)
}

/// Complete visible paint: main canvas and separately painted preview evidence.
#[derive(Debug, Default)]
struct PaintedCanvas {
    text: String,
    preview_only_words: usize,
    pending_ranges: Vec<PendingRange>,
    visible_words: Vec<VisibleWord>,
    committed_sources: BTreeMap<OccurrenceIdentity, CommittedPaintSource>,
    untimed_final_words: usize,
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
    Unadmitted(UnadmittedAppleWord),
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

fn normalize_visible_word(word: &str) -> String {
    word.trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
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

/// Ledger-owned evidence copied into the existing presentation surface.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnanchoredProjection {
    text: String,
    reason: &'static str,
    observation: ObservationIdentity,
    /// Exact ledger candidate and source provenance; never document authority.
    alternative: Option<SlotAlternative>,
}

/// The one committed Rust document plus explicitly non-authoritative UI paint.
/// Only `document_by_occurrence` can produce a committed revision. The Apple
/// mirror contributes to paint and stop delivery without entering the Bus.
#[derive(Debug, Clone, Default)]
pub struct TranscriptReducer {
    preparing_light_plus: bool,
    derived_projections: Vec<DerivedTranscriptProjection>,
    delivery_selection: Option<DerivedTranscriptProjection>,
    raw_revision_by_observation: std::collections::HashMap<ObservationIdentity, u64>,
    raw_human_revision: bool,
    /// Canonical document ordered by the PCM-backed occurrence key. W2 alone
    /// connects authenticated ledger actions and emits revisions from it.
    document_by_occurrence: BTreeMap<OccurrenceIdentity, TranscriptDocumentEntry>,
    /// Slot geometry copied only when an authenticated mutation reaches the
    /// document. Observations without committed slots cannot cover evidence.
    committed_slot_ranges: BTreeMap<OccurrenceIdentity, Vec<(u64, u64)>>,
    /// Deterministic presentation for occurrences already closed by a seal.
    /// Keyed by the same physical occurrence, so a later insert appends beside
    /// these shapes instead of replacing the document the way a single global
    /// override would.
    shaped_by_occurrence: BTreeMap<OccurrenceIdentity, ShapedPresentation>,
    /// Remembered Light+ evaluations, including unchanged results. An entry is
    /// computational memory keyed by the occurrence and its shaping inputs.
    /// It does not mint a revision, a seal, or a ledger receipt.
    shaping_evaluations: BTreeMap<OccurrenceIdentity, ShapingEvaluation>,
    /// First owner whose memo or committed shape may no longer match the
    /// document. Owners strictly before it stay valid. `None` means every
    /// current owner is settled. The cursor grants no mutation or seal authority.
    shaping_affected_from: Option<OccurrenceIdentity>,
    /// Pause bits used for the settled memos. A different tick pause starts at
    /// the earliest memo instead of trusting [`Self::shaping_affected_from`].
    shaping_pause_bits: Option<u32>,
    consultation_presentations: Vec<ConsultationPresentationReceipt>,
    /// Accepted operations of the terminal document and the user's position.
    document_timeline: DocumentTimeline,
    revision: u64,
    /// The worker publishes complete replacements. Revision orders snapshots,
    /// never the identity of the words they contain.
    apple_mirror_revision: u64,
    unadmitted_apple_words: Vec<UnadmittedAppleWord>,
    closed_apple_phrases: BTreeMap<u64, ClosedApplePhrase>,
    preview_disposition_receipts: u64,
    latest_seal_coverage: Option<SealCoverageReceipt>,
    latest_comparison: Option<TranscriptComparisonReceipt>,
    context_markers: Vec<DocumentContextMarker>,
    manual_rendered_text: Option<String>,
    manual_document_revision_receipt: Option<String>,
    /// Capability retained from this emitter's actual capture-open callback.
    manual_document_capture: Option<ManualDocumentCaptureReceipt>,
    /// Per-take transcription track switches, frozen at capture open.
    transcription_controls: TranscriptionControls,
    /// Ledger-free committed document, keyed by the same PCM occurrence key as
    /// the ledger lane. Filled only while the take opted out of seal-ledger
    /// admission; it never mixes with `document_by_occurrence` in one take.
    direct_document: BTreeMap<OccurrenceIdentity, TranscriptDocumentEntry>,
    /// The decoder observation and finality behind each direct entry, in the
    /// same occurrence order. Source of every minted direct capability.
    direct_provenance: BTreeMap<OccurrenceIdentity, DirectObservation>,
    /// Seen request generations survive unchanged text and replaced ranges.
    direct_generations: BTreeMap<(ObservationProducer, u64), u64>,
    /// The terminal capability minted by `DirectTranscriptEnd`; after it, the
    /// direct document accepts no further observations.
    direct_terminal: Option<DirectPublicationCapability>,
    /// Lifecycle ended; independent of whether the ledger issued a terminal seal.
    terminal: bool,
    terminal_sealed: bool,
    observed_seals: std::collections::BTreeSet<String>,
    applied_observations: Vec<ObservationIdentity>,
    /// Receipt identity keeps independent observations distinct on the same PCM.
    unanchored_evidence: BTreeMap<(OccurrenceIdentity, String), UnanchoredProjection>,
    /// Only new alternatives are examined at mutation publication, never paint.
    next_slot_alternative: usize,
}

/// Sample a preview word is judged by. A zero-width phrase pin is the pin
/// itself; a ranged word is its midpoint.
fn preview_sample_point(start: u64, end: u64) -> u64 {
    if start == end {
        start
    } else {
        start + end.saturating_sub(start) / 2
    }
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

/// The terminal presentation boundary never revisits an existing prefix.
fn append_terminal_sentence_boundary(text: &mut String) {
    if !matches!(
        text.trim_end().chars().last(),
        Some('.' | '!' | '?' | '…' | ':')
    ) {
        text.push('.');
    }
}

impl TranscriptReducer {
    fn mint_derived_projection(
        &mut self,
        session_id: String,
        source_raw_revision: u64,
        source_raw_text: String,
        policy: FormattingPolicy,
        result: AiFormatResult,
        source_observation: Option<ObservationIdentity>,
    ) -> DerivedTranscriptProjection {
        let status = if result.status == AiFormatStatus::Applied {
            "applied"
        } else {
            "unavailable"
        };
        let mut delivered = policy;
        let mut text = result.text;
        if result.status != AiFormatStatus::Applied {
            text = source_raw_text.clone();
            delivered = FormattingPolicy::Correction;
        } else {
            // Every applied formatter version leaves through here: a model
            // that rewrites the words may also rewrite the brand.
            let branded = match codescribe_core::pipeline::light_plus::brand_spelling(&text) {
                std::borrow::Cow::Owned(branded) => Some(branded),
                std::borrow::Cow::Borrowed(_) => None,
            };
            if let Some(branded) = branded {
                text = branded;
            }
        }
        // The high bit identifies the derived history namespace; Raw retains
        // its own monotonic counter and every CAS continues to use it.
        let revision = (1_u64 << 63) | (self.derived_projections.len() as u64 + 1);
        let mut projection = DerivedTranscriptProjection {
            schema: "codescribe.derived-transcript.v1".into(),
            emitted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            session_id: session_id.clone(),
            revision,
            source_raw_revision,
            source_raw_text,
            source_state: if source_observation.is_some() {
                "admitted_occurrence"
            } else {
                "raw_document"
            }
            .into(),
            requested_mode: policy.as_str().into(),
            delivered_mode: delivered.as_str().into(),
            rendered_text: text,
            status: status.into(),
            receipt_id: format!("formatter-derived-{session_id}-{source_raw_revision}-{revision}"),
            source_pcm: source_observation.as_ref().map(|source| {
                (
                    source.occurrence.capture_epoch,
                    source.occurrence.sample_start,
                    source.occurrence.sample_end,
                )
            }),
            source_observation_id: source_observation.as_ref().map(|source| {
                (
                    source.producer.as_str().into(),
                    source.request,
                    source.generation,
                )
            }),
            source_observation,
            publication_digest: [0; 32],
        };
        projection.publication_digest = projection.digest();
        self.derived_projections.push(projection.clone());
        projection
    }

    pub fn derived_projections(&self) -> &[DerivedTranscriptProjection] {
        &self.derived_projections
    }
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
        // One occurrence-ordered document: the ledger lane and the ledger-free
        // direct lane never coexist in one take, and the sort keeps that
        // invariant visible even to a reader who does not trust the gate.
        let mut entries = self
            .document_by_occurrence
            .values()
            .chain(self.direct_document.values())
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.occurrence.cmp(&right.occurrence));
        for entry in &mut entries {
            entry.presentation_receipt = self.shaped_by_occurrence.get(&entry.occurrence).cloned();
        }
        if let Some(receipt) = &self.manual_document_revision_receipt {
            for entry in &mut entries {
                entry.manual_edit_receipt = Some(receipt.clone());
            }
        }
        let rendered_text = self.committed_rendered_text();
        let uncertain_spans = self.uncertain_spans();
        let mut snapshot = TranscriptRevision {
            schema: "codescribe.transcript-revision.v1".to_string(),
            revision: self.revision,
            action,
            entries,
            rendered_text,
            seal_coverage: self.latest_seal_coverage.clone(),
            comparison: self.latest_comparison.clone(),
            consultation_presentations: Vec::new(),
            uncertain_spans,
            publication_digest: [0; 32],
        };
        snapshot.publication_digest = snapshot.digest();
        if !self.preparing_light_plus {
            snapshot.record_trail();
        }
        snapshot
    }

    /// Apply only the mutation authority granted by the shared ledger. An
    /// unsigned or unqualified occurrence fails closed and creates no document
    /// entry, even when an engine supplied visible text.
    /// PCM-ordered canvas: committed tokens, plus live unanchored evidence.
    /// Withdrawn Apple words remain in the evidence book, never in delivery.
    pub fn visible_projection(&self) -> String {
        let mut rendered = String::new();
        for (_, text, _, _) in self.visible_paint_fragments() {
            append_exact_fragment(&mut rendered, &text);
        }
        rendered
    }

    /// The same ordered fragments feed the main paint and its STOP receipt.
    fn visible_paint_fragments(&self) -> Vec<(u64, String, VisibleWordSource, bool)> {
        if self.document_by_occurrence.is_empty()
            && self.direct_document.is_empty()
            && let Some(text) = &self.manual_rendered_text
        {
            // An accepted human document supersedes preview paint, without
            // deleting refused evidence or claiming acoustic coverage for it.
            return vec![(
                0,
                text.clone(),
                VisibleWordSource::DocumentRevision {
                    receipt: self.manual_document_revision_receipt.clone(),
                    members: Vec::new(),
                    markers: 0,
                },
                false,
            )];
        }
        let mut fragments = self
            .unanchored_evidence
            .iter()
            .filter(|((range, _), evidence)| {
                evidence.alternative.is_none()
                    && evidence.reason != NoAuthorityReason::LateAppleWordNotCurrent.as_str()
                    && self.evidence_covering(range, evidence.reason).is_none()
            })
            .map(|((range, _), evidence)| {
                (
                    range.sample_start,
                    evidence.text.clone(),
                    VisibleWordSource::Unanchored(evidence.observation.clone()),
                    true,
                )
            })
            .collect::<Vec<_>>();
        let source_for = |range: &OccurrenceIdentity, entry: &TranscriptDocumentEntry| {
            let presentation = self
                .shaped_by_occurrence
                .get(range)
                .map(|receipt| receipt.receipt_id.clone());
            CommittedPaintSource {
                occurrence: range.clone(),
                observation_receipt: entry.observation_receipt.clone(),
                presentation_receipt: self
                    .manual_document_revision_receipt
                    .clone()
                    .or(presentation),
            }
        };
        if fragments.is_empty()
            && (self.manual_rendered_text.is_some() || !self.context_markers.is_empty())
        {
            let source = VisibleWordSource::DocumentRevision {
                receipt: self.manual_document_revision_receipt.clone(),
                members: self
                    .document_by_occurrence
                    .iter()
                    .map(|(range, entry)| source_for(range, entry))
                    .collect(),
                markers: if self.manual_rendered_text.is_some() {
                    0
                } else {
                    self.context_markers.len()
                },
            };
            fragments.push((0, self.committed_rendered_text(), source, false));
        } else {
            for (range, entry) in &self.document_by_occurrence {
                let source = VisibleWordSource::Committed(source_for(range, entry));
                fragments.push((
                    range.sample_start,
                    self.presentation_of(range, entry).to_string(),
                    source,
                    false,
                ));
            }
            // Ledger-free committed lane: identical paint source honesty, with
            // the direct observation receipt instead of a ledger receipt.
            for (range, entry) in &self.direct_document {
                let source = VisibleWordSource::Committed(source_for(range, entry));
                fragments.push((
                    range.sample_start,
                    self.presentation_of(range, entry).to_string(),
                    source,
                    false,
                ));
            }
        }
        fragments.sort_by_key(|(start, _, _, _)| *start);
        fragments
    }

    fn evidence_covering(
        &self,
        pin: &OccurrenceIdentity,
        reason: &str,
    ) -> Option<&OccurrenceIdentity> {
        self.document_by_occurrence.keys().find(|owner| {
            // A word pin may start before the Silero owner it belongs to. Its
            // midpoint inside a committed word slot is that same physical word,
            // whatever its text; a midpoint between slots is not covered.
            if reason != NoAuthorityReason::LateAppleWordNotCurrent.as_str()
                && range_within(pin, owner)
            {
                return true;
            }
            let midpoint = pin.sample_start + pin.sample_len() / 2;
            pin.is_anchored()
                && pin.same_capture(owner)
                && owner.sample_start <= midpoint
                && midpoint < owner.sample_end
                && self.committed_slot_ranges.get(*owner).is_some_and(|slots| {
                    slots
                        .iter()
                        .any(|&(start, end)| start <= midpoint && midpoint < end)
                })
        })
    }

    /// One read serves overlay paint, STOP and delivery. Midpoint coverage makes
    /// ledger-before-mirror publication safe: transient overlap never doubles a word.
    fn committed_covering(&self, start: u64, end: u64) -> Option<&OccurrenceIdentity> {
        let midpoint = preview_sample_point(start, end);
        self.document_by_occurrence
            .keys()
            .chain(self.direct_document.keys())
            .find(|owner| owner.sample_start <= midpoint && midpoint < owner.sample_end)
    }

    /// A formatted final owns every sample up to the last committed end.
    /// Pending and unmatched preview words whose point is still inside that
    /// extent restate speech the document already holds: formatting rewrites
    /// the letters, and a Silero cut can leave the raw pin in a gap or before
    /// the first occurrence. A point at or past the last committed end is a
    /// new tail and stays.
    ///
    /// An open partial is exempt. Its production pin is a stuck zero-width
    /// diagnostic, not a PCM position, so the extent cannot tell new speech
    /// from a restatement. That phrase leaves through `closed_by_final`
    /// before the paste snapshot. Refused-untimed phrases stay until timed.
    fn preview_restates_committed_document(&self, word: &UnadmittedAppleWord) -> bool {
        if matches!(
            word.source,
            UnadmittedAppleWordSource::OpenPartial { .. }
                | UnadmittedAppleWordSource::RefusedUntimed { .. }
        ) {
            return false;
        }
        let Some(extent_end) = self
            .document_by_occurrence
            .keys()
            .chain(self.direct_document.keys())
            .map(|owner| owner.sample_end)
            .max()
        else {
            return false;
        };
        preview_sample_point(word.sample_start, word.sample_end) < extent_end
    }

    /// The canvas fragments in paint order: committed tokens, live unanchored
    /// evidence and the provisional Apple tail. `read_paint` and the live
    /// formatted view both read this one list.
    fn read_paint_fragments(&self) -> Vec<(u64, String, VisibleWordSource, bool)> {
        let mut fragments = self.visible_paint_fragments();
        let ledger_word_keys = self
            .unanchored_evidence
            .iter()
            .filter(|(_, evidence)| evidence.alternative.is_none())
            .flat_map(|((range, _), evidence)| {
                evidence.text.split_whitespace().map(move |word| {
                    (
                        range.sample_start,
                        range.sample_end,
                        normalize_visible_word(word),
                    )
                })
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut seen = BTreeMap::new();
        for word in self.unadmitted_apple_words.iter().filter(|_| {
            self.manual_rendered_text.is_none() || !self.document_by_occurrence.is_empty()
        }) {
            // Formatted finals rewrite the label. Retire pending and unmatched
            // raw preview by the committed capture extent, not by string
            // equality. Open partials stay: a stuck pin does not locate them.
            if self.preview_restates_committed_document(word) {
                continue;
            }
            let untimed = matches!(
                word.source,
                UnadmittedAppleWordSource::OpenPartial { .. }
                    | UnadmittedAppleWordSource::RefusedUntimed { .. }
            );
            if !untimed {
                let key = (
                    word.sample_start,
                    word.sample_end,
                    normalize_visible_word(&word.text),
                );
                if ledger_word_keys.contains(&key) {
                    continue;
                }
                let already_seen = seen.contains_key(&key);
                let first_source = seen.entry(key).or_insert(word.source);
                if already_seen
                    && !(word.sample_start == word.sample_end && *first_source == word.source)
                {
                    continue;
                }
            }
            fragments.push((
                word.sample_start,
                word.text.clone(),
                VisibleWordSource::Unadmitted(word.clone()),
                true,
            ));
        }
        // Only ranged sources have PCM position. Stable sorting preserves word
        // order inside each untimed phrase and inside the open partial.
        fragments.sort_by_key(|(start, _, source, _)| match source {
            VisibleWordSource::Unadmitted(UnadmittedAppleWord {
                source: UnadmittedAppleWordSource::OpenPartial { .. },
                ..
            }) => (2, 0),
            VisibleWordSource::Unadmitted(UnadmittedAppleWord {
                source: UnadmittedAppleWordSource::RefusedUntimed { phrase_id },
                ..
            }) => (
                1,
                self.closed_apple_phrases
                    .get(phrase_id)
                    .map_or(*phrase_id, |phrase| phrase.arrival_index as u64),
            ),
            _ => (0, *start),
        });
        fragments
    }

    fn read_paint(&self) -> PaintedCanvas {
        let fragments = self.read_paint_fragments();
        let mut paint = PaintedCanvas::default();
        let mut utf16_len = 0usize;
        for (_, text, source, is_evidence) in fragments {
            for member in source.committed_members() {
                paint
                    .committed_sources
                    .insert(member.occurrence.clone(), member.clone());
            }
            // Offset metadata follows the one byte-preserving assembler.
            let previous_bytes = paint.text.len();
            append_exact_fragment(&mut paint.text, &text);
            let fragment_units = text.encode_utf16().count();
            let appended_units = paint.text[previous_bytes..].encode_utf16().count();
            let separator_units = appended_units - fragment_units;
            utf16_len += separator_units;
            let start = utf16_len;
            utf16_len += fragment_units;
            if is_evidence
                && fragment_units > 0
                && let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(utf16_len))
            {
                if let Some(last) = paint.pending_ranges.last_mut()
                    && usize::try_from(last.utf16_end).ok()
                        == Some(start as usize - separator_units)
                {
                    last.utf16_end = end;
                } else {
                    paint.pending_ranges.push(PendingRange {
                        utf16_start: start,
                        utf16_end: end,
                    });
                }
            }
            if is_evidence {
                paint.preview_only_words += text.split_whitespace().count();
            }
            if matches!(&source, VisibleWordSource::Unadmitted(word)
                if matches!(word.source, UnadmittedAppleWordSource::RefusedUntimed { .. }))
            {
                paint.untimed_final_words += text.split_whitespace().count();
            }
            let preview_rev = match &source {
                VisibleWordSource::Unadmitted(word) => match word.source {
                    UnadmittedAppleWordSource::OpenPartial { rev, .. } => Some(rev),
                    _ => None,
                },
                _ => None,
            };
            paint
                .visible_words
                .extend(
                    text.split_whitespace()
                        .enumerate()
                        .map(|(offset, word)| VisibleWord {
                            word: word.to_string(),
                            preview_rev,
                            source: source.clone(),
                            offset,
                            covered_by: None,
                        }),
                );
        }
        // Evidence remains visible in its sidebar, even when committed speech
        // represents it in the paste. Keep that coverage receipt in both reads.
        for ((range, _), evidence) in &self.unanchored_evidence {
            if evidence.alternative.is_some()
                || evidence.reason == NoAuthorityReason::LateAppleWordNotCurrent.as_str()
                || evidence.reason == NoAuthorityReason::OverlapWithoutWordPins.as_str()
            {
                // Ambiguous overlap is sidebar evidence. Geometric containment
                // supplies neither a delivery floor nor an accounting receipt.
                continue;
            }
            if let Some(owner) = self.evidence_covering(range, evidence.reason) {
                paint
                    .visible_words
                    .extend(
                        evidence
                            .text
                            .split_whitespace()
                            .enumerate()
                            .map(|(offset, word)| VisibleWord {
                                word: word.to_string(),
                                preview_rev: None,
                                source: VisibleWordSource::Unanchored(evidence.observation.clone()),
                                offset,
                                covered_by: Some(owner.clone()),
                            }),
                    );
            }
        }
        paint
    }

    /// Every unanchored text of one capture in PCM order, including evidence
    /// inside a committed token: no string decides what is shown.
    pub fn unanchored_evidence(
        &self,
        session_id: &str,
        capture_epoch: u64,
    ) -> Vec<UnanchoredEvidence> {
        self.unanchored_evidence
            .iter()
            .filter(|((occurrence, _), _)| {
                occurrence.session == session_id && occurrence.capture_epoch == capture_epoch
            })
            .map(|((occurrence, _), evidence)| UnanchoredEvidence {
                sample_start: occurrence.sample_start,
                sample_end: occurrence.sample_end,
                text: evidence.text.clone(),
                reason: evidence.reason.to_string(),
            })
            .collect()
    }

    fn project_unanchored(
        &mut self,
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        receipt: &MutationReceipt,
    ) {
        let Some(decision) =
            ledger.layer_trail().iter().rev().find(|decision| {
                decision.observation == *observation && decision.decision == *receipt
            })
        else {
            return;
        };
        // Silence is an authoritative disposition of this exact PCM. A seal
        // is finality of committed speech and says nothing about its alternatives.
        self.unanchored_evidence.retain(|(range, _), evidence| {
            let offered_owner = evidence
                .alternative
                .as_ref()
                .map_or(range, |alternative| &alternative.observation.occurrence);
            let human_disposition = observation.producer == ObservationProducer::ManualHuman
                && decision.is_evidence_backed()
                && receipt.grants_mutation()
                && offered_owner == &observation.occurrence;
            let silence_disposition = ledger.word_deletions().iter().any(|deletion| {
                deletion
                    .operation
                    .observation
                    .occurrence
                    .same_capture(range)
                    && deletion.verdict.target() == range
            });
            // A different observation on overlapping audio is not an answer to
            // this candidate, even if its label happens to be identical.
            !human_disposition && !silence_disposition
        });
        if let MutationReceipt::KeepVisibleUnanchored {
            occurrence,
            label,
            reason,
        } = receipt
        {
            let label = label.trim();
            if !label.is_empty() {
                self.unanchored_evidence.insert(
                    (occurrence.clone(), decision.receipt_id.clone()),
                    UnanchoredProjection {
                        text: label.to_string(),
                        reason: reason.as_str(),
                        observation: observation.clone(),
                        alternative: None,
                    },
                );
            }
        }
        if !decision.is_evidence_backed()
            || matches!(
                receipt,
                MutationReceipt::Refuse {
                    reason: RefuseReason::BatchDuplicate,
                    ..
                }
            )
        {
            return;
        }
        for (index, alternative) in ledger
            .slot_alternatives()
            .iter()
            .enumerate()
            .skip(self.next_slot_alternative)
        {
            let unresolved = matches!(
                alternative.reason,
                "ambiguous_pcm_target"
                    | "word_partition_requires_coverage"
                    | "group_alignment_ambiguous"
                    | "resegmentation_unaccounted_speech"
                    | "overlapping_new_pins"
                    | "word_speech_unproven"
            );
            let belongs = alternative.observation == *observation;
            // Leave another queued observation's alternatives for its event.
            // Irrelevant historical proposals cannot hold the cursor back.
            if index == self.next_slot_alternative && (!unresolved || belongs) {
                self.next_slot_alternative += 1;
            }
            if !unresolved || !belongs || alternative.candidate.trim().is_empty() {
                continue;
            }
            // The alternative explicitly names this observation's group. It
            // does not name a child refusal: matching labels cannot mint that
            // missing relationship or a finer timestamp.
            let candidate = &alternative.observation;
            {
                // A whole offered group owned by a stronger/newer source is
                // already accounted. Use exact source ranges, including gaps.
                let mut protected = alternative
                    .sources
                    .iter()
                    .filter(|source| {
                        source.producer == ObservationProducer::ManualHuman
                            || source.producer.authority_rank()
                                > observation.producer.authority_rank()
                            || (source.producer == observation.producer
                                && source.observation.generation >= observation.generation)
                    })
                    .flat_map(|source| ledger.slot_source_ranges(source))
                    .filter(|range| range.same_capture(&candidate.occurrence))
                    .collect::<Vec<_>>();
                protected.sort_by_key(|range| (range.sample_start, range.sample_end));
                let mut cursor = candidate.occurrence.sample_start;
                for range in protected {
                    if range.sample_start <= cursor && range.sample_end > cursor {
                        cursor = range.sample_end;
                    }
                }
                if cursor >= candidate.occurrence.sample_end {
                    continue;
                }
            }
            self.unanchored_evidence.insert(
                (
                    candidate.occurrence.clone(),
                    format!("{}:slot-alternative-{index}", decision.receipt_id),
                ),
                UnanchoredProjection {
                    text: alternative.candidate.clone(),
                    reason: alternative.reason,
                    observation: candidate.clone(),
                    alternative: Some(alternative.clone()),
                },
            );
        }
    }

    pub fn apply_ledger_mutation(
        &mut self,
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        receipt: &MutationReceipt,
    ) -> Option<TranscriptRevision> {
        let capture_owner = self.document_by_occurrence.keys().next().or_else(|| {
            self.unanchored_evidence
                .keys()
                .next()
                .map(|(range, _)| range)
        });
        if capture_owner.is_some_and(|owner| !owner.same_capture(&observation.occurrence)) {
            return None;
        }
        self.project_unanchored(ledger, observation, receipt);
        if self.document_by_occurrence.is_empty() && self.manual_rendered_text.is_some() {
            // Late recognizer evidence cannot replace an explicitly recovered
            // human document or move its source CAS. Evidence stays inspectable.
            return None;
        }
        if matches!(receipt, MutationReceipt::KeepVisibleUnanchored { .. }) {
            return None;
        }
        if !(receipt.grants_mutation() || matches!(receipt, MutationReceipt::Preserve { .. }))
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
        // An equal label can acquire real word pins without a new document
        // revision. Refresh geometry only from that authenticated receipt.
        if matches!(receipt, MutationReceipt::Preserve { .. }) {
            if self
                .document_by_occurrence
                .contains_key(&observation.occurrence)
            {
                self.committed_slot_ranges.insert(
                    observation.occurrence.clone(),
                    ledger.committed_word_pin_ranges(&observation.occurrence),
                );
            }
            return None;
        }
        let serial = ledger.serial_of(&observation.occurrence)?;
        let composition = ledger.compose(&observation.occurrence).ok()?;
        let trail = ledger
            .layer_trail_for(&observation.occurrence)
            .filter(|decision| decision.is_evidence_backed())
            .map(|decision| decision.receipt_id.clone())
            .collect::<Vec<_>>();
        let authenticated_empty_deletion = composition.tokens.is_empty()
            && ledger
                .word_deletions()
                .iter()
                .any(|deletion| deletion.operation.observation == *observation);
        if trail.is_empty() || (composition.tokens.is_empty() && !authenticated_empty_deletion) {
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
            word_confidence: composition
                .tokens
                .iter()
                .filter_map(|token| {
                    token.confidence.map(|confidence| WordConfidenceEvidence {
                        token: token.token.clone(),
                        token_sample_start: token.token_sample_start,
                        token_sample_end: token.token_sample_end,
                        producer: token.producer,
                        confidence,
                        surface_rewritten: token.surface_rewritten,
                    })
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
        // A changed label drops this owner and every later owner. Their left
        // context includes this presentation. A same-label edit keeps the
        // settled prefix and the suffix unless the next sentence break moved.
        let previous_label = self
            .document_by_occurrence
            .get(&observation.occurrence)
            .map(|existing| existing.label.clone());
        if self
            .shaped_by_occurrence
            .get(&observation.occurrence)
            .is_some_and(|shaped| shaped.source_label != entry.label)
        {
            self.shaped_by_occurrence.remove(&observation.occurrence);
        }
        let label_changed = previous_label.as_deref() != Some(entry.label.as_str());
        self.document_by_occurrence
            .insert(observation.occurrence.clone(), entry.clone());
        self.committed_slot_ranges.insert(
            observation.occurrence.clone(),
            ledger.committed_word_pin_ranges(&observation.occurrence),
        );
        self.invalidate_for_document_edit(&observation.occurrence, label_changed, ledger);
        let action = if observation.producer == ObservationProducer::ManualHuman {
            ReducerAction::ApplyManualEdit { entry }
        } else {
            ReducerAction::ApplyLedgerDecision { entry }
        };
        self.applied_observations.push(observation.clone());
        let revision = self.revision_for_action(action);
        self.raw_revision_by_observation
            .insert(observation.clone(), revision.revision);
        Some(revision)
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
        // Finality belongs to the committed source. Unresolved evidence keeps
        // its own receipt until an authoritative disposition accounts for it.
        let occurrence = receipt.sealed_occurrences.first()?.clone();
        self.observed_seals.insert(receipt.receipt_id.clone());
        let terminal = !receipt.is_occurrence_seal();
        if terminal {
            self.terminal_sealed = true;
        }
        Some(self.revision_for_action(ReducerAction::RecordLedgerSeal {
            word_finality: receipt.word_finality.clone(),
            occurrence,
            seal_receipt: receipt.receipt_id.clone(),
            terminal: !receipt.is_occurrence_seal(),
        }))
    }

    /// Mint the capability covering exactly the current direct document. The
    /// revision number is the one `revision_for_action` is about to assign;
    /// the two stay coupled by construction, never by a later lookup.
    fn mint_direct_capability(
        &self,
        session_id: &str,
        capture_epoch: u64,
        terminal: bool,
    ) -> DirectPublicationCapability {
        let observations = self
            .direct_document
            .iter()
            .map(|(range, entry)| {
                let provenance = self
                    .direct_provenance
                    .get(range)
                    .expect("every direct entry carries its observation");
                DirectObservation {
                    observation: provenance.observation.clone(),
                    text: entry.label.clone(),
                    final_chunk: provenance.final_chunk,
                }
            })
            .collect();
        DirectPublicationCapability::mint(
            session_id,
            capture_epoch,
            self.revision.saturating_add(1),
            observations,
            self.committed_rendered_text(),
            terminal,
        )
    }

    /// Accept one decoder observation into the ledger-free document. Open only
    /// while the take opted out of seal-ledger admission; when the ledger owns
    /// this take, direct text has no authority here at all.
    ///
    /// Fences: the range must name real audio; a newer observation of one range
    /// wins only on (producer authority rank, generation); a final chunk pins
    /// its range against equal-or-lower-priority rewrites. Disjoint ranges
    /// coexist in PCM order; nothing is clipped by string comparison.
    pub fn apply_direct_transcript(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        final_chunk: bool,
    ) -> Option<TranscriptRevision> {
        if self.transcription_controls.seal_ledger_enabled || self.direct_terminal.is_some() {
            return None;
        }
        let occurrence = observation.occurrence.clone();
        if !occurrence.is_anchored()
            || text.trim().is_empty()
            || !matches!(
                observation.producer,
                ObservationProducer::Apple
                    | ObservationProducer::Whisper
                    | ObservationProducer::CloudLive
            )
            || (observation.producer == ObservationProducer::Apple
                && !self.transcription_controls.apple_live_enabled)
        {
            return None;
        }
        let request = (observation.producer, observation.request);
        if self
            .direct_generations
            .get(&request)
            .is_some_and(|generation| *generation >= observation.generation)
        {
            return None;
        }
        // Receipt freshness is independent of whether the visible bytes change.
        self.direct_generations
            .insert(request, observation.generation);
        let mut replaced = Vec::new();
        for (range, existing) in &self.direct_provenance {
            if range.session != occurrence.session
                || range.capture_epoch != occurrence.capture_epoch
            {
                return None;
            }
            let same_request = existing.observation.producer == observation.producer
                && existing.observation.request == observation.request;
            let overlaps = range.sample_start < occurrence.sample_end
                && occurrence.sample_start < range.sample_end;
            if !same_request && !overlaps {
                continue;
            }
            let incumbent = existing.observation.producer.authority_rank();
            let challenger = observation.producer.authority_rank();
            if challenger < incumbent
                || (challenger == incumbent
                    && existing.final_chunk
                    && (!same_request || !final_chunk))
                || (challenger == incumbent
                    && !same_request
                    && observation.request <= existing.observation.request)
            {
                return None;
            }
            replaced.push(range.clone());
        }
        for range in replaced {
            self.direct_document.remove(&range);
            self.direct_provenance.remove(&range);
        }
        let entry = TranscriptDocumentEntry {
            occurrence: occurrence.clone(),
            label: text.to_string(),
            // Provenance string in the ledger lane's own prefix convention,
            // with the lane name instead of a receipt id: it attests a decoder
            // observation directly and never claims ledger membership.
            observation_receipt: format!(
                "{}:{}:{}:direct",
                observation.producer.as_str(),
                observation.request,
                observation.generation,
            ),
            word_evidence_receipts: Vec::new(),
            layer_decision_receipts: Vec::new(),
            seal_receipt: None,
            manual_edit_receipt: None,
            presentation_receipt: None,
            word_confidence: Vec::new(),
        };
        self.direct_document.insert(occurrence.clone(), entry);
        self.direct_provenance.insert(
            occurrence,
            DirectObservation {
                observation: observation.clone(),
                text: text.to_string(),
                final_chunk,
            },
        );
        let capability = self.mint_direct_capability(
            &observation.occurrence.session,
            observation.occurrence.capture_epoch,
            false,
        );
        Some(self.revision_for_action(ReducerAction::ApplyDirectTranscript { capability }))
    }

    /// Close the ledger-free document for this capture. The terminal capability
    /// is the honest completion marker the Bus records in place of an acoustic
    /// seal; after it, no observation may rewrite this take's direct document.
    pub fn apply_direct_transcript_end(
        &mut self,
        session_id: &str,
        capture_epoch: u64,
    ) -> Option<TranscriptRevision> {
        if self.transcription_controls.seal_ledger_enabled || self.direct_terminal.is_some() {
            return None;
        }
        if self
            .direct_document
            .first_key_value()
            .is_some_and(|(owner, _)| {
                owner.session != session_id || owner.capture_epoch != capture_epoch
            })
        {
            return None;
        }
        let capability = self.mint_direct_capability(session_id, capture_epoch, true);
        self.direct_terminal = Some(capability.clone());
        Some(self.revision_for_action(ReducerAction::ApplyDirectTranscript { capability }))
    }

    /// Authenticate the completed direct document, independently of acoustic
    /// seals. Only the reducer can mint this capability; every operation must
    /// name its current revision and the same opened capture.
    fn authenticated_direct_document(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<&DirectPublicationCapability, UserRevisionRefusal> {
        let owner = self
            .direct_terminal
            .as_ref()
            .ok_or(UserRevisionRefusal::NotTerminal)?;
        if self.transcription_controls.seal_ledger_enabled
            || !owner.terminal
            || owner.digest != owner.compute_digest()
            || owner.rendered_text != self.committed_rendered_text()
            || owner.revision != self.revision
        {
            return Err(UserRevisionRefusal::AuthorityUnavailable);
        }
        if owner.session_id != session_id {
            return Err(UserRevisionRefusal::SessionMismatch);
        }
        if source_revision != self.revision {
            return Err(UserRevisionRefusal::StaleRevision {
                expected: self.revision,
                actual: source_revision,
            });
        }
        if self.direct_document.is_empty() {
            return Err(UserRevisionRefusal::NoCommittedDocument);
        }
        Ok(owner)
    }

    /// A committed ledger-free document owns this take's canvas.
    fn direct_document_is_committed(&self) -> bool {
        !self.direct_document.is_empty()
    }

    /// Commit a whole-document user edit without fabricating per-word acoustic
    /// ownership. The ledger authenticates the exact committed source occurrence
    /// set, then this reducer mints the only new document revision.
    pub fn apply_user_revision(
        &mut self,
        ledger: &mut AcousticLedger,
        intent: &UserRevisionIntent,
    ) -> Result<TranscriptRevision, UserRevisionRefusal> {
        if intent.provenance == DocumentRevisionProvenance::Navigation {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "navigation_requires_accepted_step",
            ));
        }
        self.apply_document_text(ledger, intent, None)
    }

    /// Re-select one accepted step: Undo, Redo or a version pick. Mints one
    /// navigation receipt revision carrying the step's committed bytes; every
    /// step stays and only the cursor moves. A formatted step also returns its
    /// accepted presentation rebased onto that receipt, published after it.
    pub fn navigate_document(
        &mut self,
        ledger: &mut AcousticLedger,
        session_id: &str,
        source_revision: u64,
        target: usize,
    ) -> Result<(TranscriptRevision, Option<DerivedTranscriptProjection>), UserRevisionRefusal>
    {
        let step = self.document_timeline.steps.get(target).cloned().ok_or(
            UserRevisionRefusal::LedgerRefusal("navigation_step_missing"),
        )?;
        if target == self.document_timeline.cursor {
            return Err(UserRevisionRefusal::Unchanged);
        }
        let intent = UserRevisionIntent {
            session_id: session_id.to_string(),
            source_revision,
            rendered_text: step.raw_text.clone(),
            provenance: DocumentRevisionProvenance::Navigation,
        };
        let revision = self.apply_document_text(ledger, &intent, Some(target))?;
        let presentation = step
            .presentation
            .as_ref()
            .map(|accepted| self.rebase_presentation(accepted));
        Ok((revision, presentation))
    }

    /// Accept one applied formatter result as a new step after the cursor.
    /// The source is the version selected at `source_revision` (CAS); an
    /// equal-bytes result is still an accepted attempt. A result the word
    /// receipt refuses keeps its evidence row and adds nothing. Acceptance
    /// mints one navigation receipt revision (committed bytes unchanged), so
    /// every later request, including a second formatter result that named
    /// the same revision, must name the new one.
    fn accept_formatter_result(
        &mut self,
        ledger: &mut AcousticLedger,
        session_id: &str,
        source_revision: u64,
        policy: FormattingPolicy,
        result: AiFormatResult,
    ) -> Result<(TranscriptRevision, DerivedTranscriptProjection), UserRevisionRefusal> {
        let source = self.terminal_revision_source(session_id, source_revision)?;
        let committed = self.committed_rendered_text();
        self.anchor_document_timeline(session_id);
        if self.document_timeline.steps.is_empty() {
            return Err(UserRevisionRefusal::NoCommittedDocument);
        }
        let mut projection = self.mint_derived_projection(
            session_id.to_string(),
            source_revision,
            source.clone(),
            policy,
            result,
            None,
        );
        let index = self.derived_projections.len() - 1;
        if projection.status != "applied" {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "formatter_result_not_applied",
            ));
        }
        let cursor = self.document_timeline.cursor;
        let revision = self.apply_document_text(
            ledger,
            &UserRevisionIntent {
                session_id: session_id.to_string(),
                source_revision,
                rendered_text: committed.clone(),
                provenance: DocumentRevisionProvenance::Navigation,
            },
            Some(cursor),
        )?;
        // Bind the presentation to the receipt just minted: the Bus paints a
        // derived projection only over the revision it last published.
        projection.source_raw_revision = self.revision;
        if source != committed {
            projection.source_raw_text = committed;
            projection.source_state = "selected_version".into();
        }
        projection.receipt_id = format!(
            "formatter-derived-{session_id}-{}-{}",
            projection.source_raw_revision, projection.revision
        );
        projection.publication_digest = projection.digest();
        self.derived_projections[index] = projection.clone();
        let step = self.snapshot_step(
            "formatter",
            Some(policy.as_str().to_string()),
            projection.receipt_id.clone(),
            Some(projection.clone()),
        );
        self.push_document_step(step);
        Ok((revision, projection))
    }

    /// The linear operation history and the selected step.
    pub fn document_timeline(&self) -> &DocumentTimeline {
        &self.document_timeline
    }

    /// Bytes the canvas shows for the selected step: its accepted formatter
    /// result when it is a formatted step, the committed document otherwise.
    fn selected_text(&self) -> String {
        self.document_timeline
            .current()
            .and_then(|step| step.presentation.as_ref())
            .map_or_else(
                || self.committed_rendered_text(),
                |presentation| presentation.rendered_text.clone(),
            )
    }

    fn snapshot_step(
        &self,
        provenance: &str,
        detail: Option<String>,
        receipt_id: String,
        presentation: Option<DerivedTranscriptProjection>,
    ) -> DocumentVersionStep {
        let raw_text = self.committed_rendered_text();
        DocumentVersionStep {
            provenance: provenance.to_string(),
            detail,
            receipt_id,
            emitted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            rendered_text: presentation
                .as_ref()
                .map_or_else(|| raw_text.clone(), |shown| shown.rendered_text.clone()),
            raw_text,
            raw_human_revision: self.raw_human_revision,
            presentation,
        }
    }

    /// Record the document as accepted before its first explicit operation.
    /// An empty document is no version: the first operation becomes step 0.
    fn anchor_document_timeline(&mut self, session_id: &str) {
        if !self.document_timeline.steps.is_empty()
            || self.committed_rendered_text().trim().is_empty()
        {
            return;
        }
        let receipt = self.manual_document_revision_receipt.clone();
        let provenance = match receipt.as_deref() {
            Some(id) if id.starts_with("light-plus-") => "light-plus",
            Some(id) if id.starts_with("retranscribe-") => "retranscribe",
            Some(id) if id.starts_with("user-edit-") => "user-edit",
            _ => "raw",
        };
        let receipt_id = receipt.unwrap_or_else(|| format!("raw-{session_id}-{}", self.revision));
        let base = self.snapshot_step(provenance, None, receipt_id, None);
        self.document_timeline = DocumentTimeline {
            steps: vec![base],
            cursor: 0,
        };
    }

    /// Accept one operation after the cursor; an abandoned redo branch ends.
    fn push_document_step(&mut self, step: DocumentVersionStep) {
        let timeline = &mut self.document_timeline;
        timeline.steps.truncate(timeline.cursor.saturating_add(1));
        timeline.steps.push(step);
        timeline.cursor = timeline.steps.len() - 1;
    }

    /// Light+ reshapes the committed document in place; it is not an
    /// operation the user undoes, so the selected step follows its bytes.
    fn refresh_selected_step_raw(&mut self) {
        let raw_text = self.committed_rendered_text();
        let raw_human_revision = self.raw_human_revision;
        let cursor = self.document_timeline.cursor;
        if let Some(step) = self.document_timeline.steps.get_mut(cursor) {
            if step.presentation.is_none() {
                step.rendered_text.clone_from(&raw_text);
            }
            step.raw_text = raw_text;
            step.raw_human_revision = raw_human_revision;
        }
    }

    /// Present an accepted formatter result again over the receipt navigation
    /// just minted. Same bytes and mode, no provider call: only a new derived
    /// identity, so the Bus binds the paint to the current committed revision.
    fn rebase_presentation(
        &mut self,
        accepted: &DerivedTranscriptProjection,
    ) -> DerivedTranscriptProjection {
        let mut projection = accepted.clone();
        projection.revision = (1_u64 << 63) | (self.derived_projections.len() as u64 + 1);
        projection.source_raw_revision = self.revision;
        projection.source_raw_text = self.committed_rendered_text();
        projection.emitted_at =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        projection.receipt_id = format!(
            "formatter-navigation-{}-{}-{}",
            projection.session_id, projection.source_raw_revision, projection.revision
        );
        projection.publication_digest = projection.digest();
        self.derived_projections.push(projection.clone());
        projection
    }

    /// The one text corridor for edits, retranscriptions, Light+ and
    /// navigation. `navigation` names the step being re-selected.
    fn apply_document_text(
        &mut self,
        ledger: &mut AcousticLedger,
        intent: &UserRevisionIntent,
        navigation: Option<usize>,
    ) -> Result<TranscriptRevision, UserRevisionRefusal> {
        if intent.provenance == DocumentRevisionProvenance::Formatter {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "formatter_requires_derived_projection",
            ));
        }
        if intent.rendered_text.trim().is_empty() {
            return Err(UserRevisionRefusal::EmptyText);
        }
        let direct = self.direct_document_is_committed();
        let source_occurrences = if direct {
            self.authenticated_direct_document(&intent.session_id, intent.source_revision)?;
            Vec::new()
        } else if self.document_by_occurrence.is_empty() {
            if !matches!(
                intent.provenance,
                DocumentRevisionProvenance::UserEdit
                    | DocumentRevisionProvenance::Retranscribe
                    | DocumentRevisionProvenance::Navigation
            ) {
                return Err(UserRevisionRefusal::NoCommittedDocument);
            }
            let owner = self
                .manual_document_capture
                .as_ref()
                .filter(|owner| {
                    owner.completed() && ledger.authenticates_manual_document_capture(owner)
                })
                .ok_or(UserRevisionRefusal::NoCommittedDocument)?;
            if owner.session_id() != intent.session_id {
                return Err(UserRevisionRefusal::SessionMismatch);
            }
            if intent.source_revision != self.revision {
                return Err(UserRevisionRefusal::StaleRevision {
                    expected: self.revision,
                    actual: intent.source_revision,
                });
            }
            Vec::new()
        } else if intent.provenance == DocumentRevisionProvenance::LightPlus {
            self.authenticated_presentation_occurrences(&intent.session_id, intent.source_revision)?
        } else {
            self.authenticated_revision_occurrences(&intent.session_id, intent.source_revision)?
        };
        // An accepted retranscription is an attempt even when its bytes repeat,
        // and navigation re-selects bytes it already accepted. An edit is
        // compared with what the canvas shows, Light+ with the document.
        let unchanged = match intent.provenance {
            DocumentRevisionProvenance::UserEdit => self.selected_text() == intent.rendered_text,
            DocumentRevisionProvenance::LightPlus => {
                self.committed_rendered_text() == intent.rendered_text
            }
            _ => false,
        };
        if unchanged {
            return Err(UserRevisionRefusal::Unchanged);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(UserRevisionRefusal::RevisionExhausted)?;
        let accepted_operation = matches!(
            intent.provenance,
            DocumentRevisionProvenance::UserEdit | DocumentRevisionProvenance::Retranscribe
        );
        if accepted_operation {
            self.anchor_document_timeline(&intent.session_id);
        }
        let receipt = if direct {
            None
        } else {
            Some(
                ledger
                    .record_manual_document_revision(
                        &intent.session_id,
                        intent.source_revision,
                        revision,
                        &intent.rendered_text,
                        &source_occurrences,
                        intent.provenance,
                    )
                    .map_err(UserRevisionRefusal::LedgerRefusal)?,
            )
        };
        let receipt_id = receipt.as_ref().map_or_else(
            || {
                DirectPublicationCapability::document_revision_receipt_id(
                    intent.provenance,
                    &intent.session_id,
                    revision,
                )
            },
            |receipt| receipt.receipt_id.clone(),
        );
        let navigated_human_revision = navigation
            .and_then(|target| self.document_timeline.steps.get(target))
            .map(|step| step.raw_human_revision);
        match navigated_human_revision {
            Some(human) => self.raw_human_revision = human,
            None => {
                self.raw_human_revision |=
                    intent.provenance != DocumentRevisionProvenance::LightPlus;
            }
        }
        self.manual_rendered_text = Some(intent.rendered_text.clone());
        if intent.provenance != DocumentRevisionProvenance::LightPlus {
            self.consultation_presentations.clear();
            self.shaped_by_occurrence.clear();
            self.shaping_evaluations.clear();
            self.shaping_pause_bits = None;
        }
        // Light+ keeps the live shapes beneath its bytes: when a late ledger
        // edit retires this document, the settled prefix renders exactly as
        // before and only the edited owner onward is shaped again.
        // The manual document owns presentation until a later ledger edit
        // clears it. The cursor then starts at the first physical owner.
        self.shaping_affected_from = self.document_by_occurrence.keys().next().cloned();
        self.manual_document_revision_receipt =
            receipt.as_ref().map(|receipt| receipt.receipt_id.clone());
        match navigation {
            Some(target) => self.document_timeline.cursor = target,
            None if accepted_operation => {
                let step =
                    self.snapshot_step(intent.provenance.as_str(), None, receipt_id.clone(), None);
                self.push_document_step(step);
            }
            None => self.refresh_selected_step_raw(),
        }
        let action = if let Some(receipt) = receipt {
            ReducerAction::ApplyUserRevision { receipt }
        } else {
            let epoch = self
                .direct_terminal
                .as_ref()
                .expect("authenticated direct owner")
                .capture_epoch;
            let mut capability = self.mint_direct_capability(&intent.session_id, epoch, true);
            capability.document_revision = Some((intent.source_revision, intent.provenance));
            capability.digest = capability.compute_digest();
            self.direct_terminal = Some(capability.clone());
            ReducerAction::ApplyDirectTranscript { capability }
        };
        Ok(self.revision_for_action(action))
    }

    /// Derive a Max version from the admitted sealed group. Later suffix
    /// revisions leave its unchanged members valid; human document edits do not.
    pub fn apply_consultation_presentation(
        &mut self,
        ledger: &mut AcousticLedger,
        input: ConsultationPresentationInput<'_>,
    ) -> Result<DerivedTranscriptProjection, UserRevisionRefusal> {
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
        let source_text = self.committed_rendered_text();
        let text = self.consulted_rendered_text();
        let mut projection = self.mint_derived_projection(
            first.occurrence.session.clone(),
            self.revision,
            source_text,
            FormattingPolicy::Max,
            AiFormatResult {
                text,
                reasoning_text: None,
                status: AiFormatStatus::Applied,
            },
            None,
        );
        projection.source_state = "sealed_group".into();
        projection.receipt_id = format!("formatter-max-{}", receipt.receipt_id);
        projection.publication_digest = projection.digest();
        *self
            .derived_projections
            .last_mut()
            .expect("minted Max projection") = projection.clone();
        Ok(projection)
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
            if self.manual_rendered_text.is_none() {
                return Err(UserRevisionRefusal::NoCommittedDocument);
            }
            let owner = self
                .manual_document_capture
                .as_ref()
                .filter(|owner| owner.completed())
                .ok_or(UserRevisionRefusal::NoCommittedDocument)?;
            if owner.session_id() != session_id {
                return Err(UserRevisionRefusal::SessionMismatch);
            }
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

    /// Sentence break for one occurrence. The previous same-epoch owner is the
    /// ordered predecessor, not a walk of the prefix. It does not build
    /// left-context text.
    fn sentence_break_before(
        &self,
        occurrence: &OccurrenceIdentity,
        ledger: &AcousticLedger,
        sentence_pause_sec: f32,
    ) -> bool {
        let Some((previous, _)) = self.document_by_occurrence.range(..occurrence).next_back()
        else {
            return false;
        };
        if previous.capture_epoch != occurrence.capture_epoch {
            return false;
        }
        let Some(serial) = ledger.serial_of(previous) else {
            return false;
        };
        let Some(samples) = previous.sample_end.checked_sub(previous.sample_start) else {
            return false;
        };
        let Some(gap) = occurrence.sample_start.checked_sub(previous.sample_end) else {
            return false;
        };
        samples > 0
            && gap as f64 * serial.duration_ms / samples as f64
                >= f64::from(sentence_pause_sec) * 1000.0
    }

    /// Exact computational hit. A hit is an unchanged evaluation only. A stored
    /// string that differs from the source label is not a receipt and is ignored.
    fn unchanged_evaluation_hit(
        &self,
        occurrence: &OccurrenceIdentity,
        source_label: &str,
        left_context_sha256: [u8; 32],
        sentence_break_before: bool,
        sentence_pause_bits: u32,
    ) -> bool {
        self.shaping_evaluations
            .get(occurrence)
            .is_some_and(|memo| {
                memo.occurrence == *occurrence
                    && memo.source_label == source_label
                    && memo.left_context_sha256 == left_context_sha256
                    && memo.sentence_break_before == sentence_break_before
                    && memo.sentence_pause_bits == sentence_pause_bits
                    && memo.shaped_text == source_label
            })
    }

    /// Whether this owner can be skipped without rebuilding its left context.
    ///
    /// A committed shape with the same source label is the existing
    /// already-shaped guard. A computational memo is stable only while its
    /// label, pause, and predecessor sentence break still match. Left context
    /// is trusted here because a document edit drops memos from the edited
    /// owner forward, so a settled prefix is not hashed again.
    fn owner_shaping_stable(
        &self,
        occurrence: &OccurrenceIdentity,
        sentence_pause_sec: f32,
        ledger: &AcousticLedger,
    ) -> bool {
        #[cfg(test)]
        tests::record_shaping_work(&occurrence.session, |counts| counts.stable_checks += 1);
        let Some(entry) = self.document_by_occurrence.get(occurrence) else {
            return false;
        };
        if self
            .shaped_by_occurrence
            .get(occurrence)
            .is_some_and(|shaped| shaped.source_label == entry.label)
        {
            return true;
        }
        let Some(memo) = self.shaping_evaluations.get(occurrence) else {
            return false;
        };
        memo.occurrence == *occurrence
            && memo.source_label == entry.label
            && memo.shaped_text == entry.label
            && memo.sentence_pause_bits == sentence_pause_sec.to_bits()
            && memo.sentence_break_before
                == self.sentence_break_before(occurrence, ledger, sentence_pause_sec)
    }

    /// First owner a non-terminal tick must evaluate.
    ///
    /// Empty `named` does not mean "shape nothing": a new admission still
    /// starts at the affected cursor. A named seal checks that bounded set
    /// and the cursor. A stable named owner with no invalidated suffix is
    /// not a start, so the tick does not walk the document.
    fn first_incremental_shaping_owner(
        &self,
        named: &[OccurrenceIdentity],
        sentence_pause_sec: f32,
        ledger: &AcousticLedger,
    ) -> Option<OccurrenceIdentity> {
        if self.manual_rendered_text.is_some() || self.document_by_occurrence.is_empty() {
            return None;
        }
        let pause_bits = sentence_pause_sec.to_bits();
        let pause_drift = self
            .shaping_pause_bits
            .is_some_and(|bits| bits != pause_bits);
        let earliest_unstable = if pause_drift {
            self.shaping_evaluations
                .keys()
                .next()
                .cloned()
                .or_else(|| self.affected_shaping_owner())
        } else {
            self.affected_shaping_owner()
        };
        let mut named_needs_work: Option<OccurrenceIdentity> = None;
        for occurrence in named
            .iter()
            .filter(|occurrence| self.document_by_occurrence.contains_key(*occurrence))
        {
            let needs_work = !self.owner_shaping_stable(occurrence, sentence_pause_sec, ledger);
            if !needs_work {
                continue;
            }
            named_needs_work = Some(match named_needs_work {
                Some(existing) if &existing < occurrence => existing,
                _ => occurrence.clone(),
            });
        }
        match (named_needs_work, earliest_unstable) {
            (Some(named), Some(unstable)) => Some(if named < unstable { named } else { unstable }),
            (Some(named), None) => Some(named),
            (None, Some(unstable)) => Some(unstable),
            (None, None) => None,
        }
    }

    /// First document owner at or after the affected cursor.
    fn affected_shaping_owner(&self) -> Option<OccurrenceIdentity> {
        let from = self.shaping_affected_from.as_ref()?;
        self.document_by_occurrence
            .range(from.clone()..)
            .next()
            .map(|(key, _)| key.clone())
    }

    fn next_owner_after(&self, occurrence: &OccurrenceIdentity) -> Option<OccurrenceIdentity> {
        self.document_by_occurrence
            .range((
                std::ops::Bound::Excluded(occurrence),
                std::ops::Bound::Unbounded,
            ))
            .next()
            .map(|(key, _)| key.clone())
    }

    /// Drop computational memory and committed shapes from `from` forward.
    /// `BTreeMap::split_off` is the ordered boundary. It does not hash prefixes.
    fn drop_shaping_from(&mut self, from: &OccurrenceIdentity) {
        let _ = self.shaping_evaluations.split_off(from);
        let _ = self.shaped_by_occurrence.split_off(from);
    }

    fn mark_shaping_affected(&mut self, from: &OccurrenceIdentity) {
        self.shaping_affected_from = Some(match self.shaping_affected_from.take() {
            Some(existing) if &existing < from => existing,
            _ => from.clone(),
        });
    }

    /// A same-label edit can still change the successor's sentence break when
    /// the predecessor serial duration moves. One successor, one predecessor.
    fn successor_break_drifted(
        &self,
        occurrence: &OccurrenceIdentity,
        ledger: &AcousticLedger,
    ) -> bool {
        let Some(successor) = self.next_owner_after(occurrence) else {
            return false;
        };
        let (pause, stored_break) = {
            let Some(memo) = self.shaping_evaluations.get(&successor) else {
                return false;
            };
            (
                f32::from_bits(memo.sentence_pause_bits),
                memo.sentence_break_before,
            )
        };
        self.sentence_break_before(&successor, ledger, pause) != stored_break
    }

    fn remember_unchanged_shaping(
        &mut self,
        occurrence: &OccurrenceIdentity,
        source_label: String,
        left_context_sha256: [u8; 32],
        sentence_break_before: bool,
        sentence_pause_bits: u32,
        shaped_text: String,
    ) {
        self.shaping_pause_bits = Some(sentence_pause_bits);
        self.shaping_evaluations.insert(
            occurrence.clone(),
            ShapingEvaluation {
                occurrence: occurrence.clone(),
                source_label,
                left_context_sha256,
                sentence_break_before,
                sentence_pause_bits,
                shaped_text,
            },
        );
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
        #[cfg(test)]
        tests::record_shaping_work(&occurrence.session, |counts| counts.owner_visits += 1);
        // A whole-document revision (user edit or terminal Light+)
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
        let sentence_break_before =
            self.sentence_break_before(occurrence, ledger, sentence_pause_sec);
        let left_context = self.rendered_occurrence_span(Some(occurrence));
        let left_context_sha256 = shaping_context_sha256(&left_context);
        let sentence_pause_bits = sentence_pause_sec.to_bits();
        if self.unchanged_evaluation_hit(
            occurrence,
            &source_label,
            left_context_sha256,
            sentence_break_before,
            sentence_pause_bits,
        ) {
            return Err(IncrementalShapingRefusal::Unchanged);
        }
        #[cfg(test)]
        {
            tests::record_shaping_work(&occurrence.session, |counts| {
                counts.shape_calls += 1;
                counts.context_bytes += left_context.len();
            });
        }
        let shaped = codescribe_core::pipeline::light_plus::apply_live_span(
            &left_context,
            &source_label,
            sentence_break_before,
        );
        // An empty shaping must
        // never be committed: an empty presentation would delete spoken audio
        // from the document on the strength of a formatting pass.
        if shaped.trim().is_empty() {
            return Err(IncrementalShapingRefusal::EmptyShape);
        }
        if shaped == source_label {
            self.remember_unchanged_shaping(
                occurrence,
                source_label,
                left_context_sha256,
                sentence_break_before,
                sentence_pause_bits,
                shaped,
            );
            return Err(IncrementalShapingRefusal::Unchanged);
        }
        self.shaping_evaluations.remove(occurrence);
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
        self.invalidate_shaped_suffix(occurrence);
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

    /// Return the version the user has selected after authenticating the
    /// session/revision compare-and-swap boundary: a formatted step's accepted
    /// bytes, otherwise the committed document. This is read-only formatter
    /// input and cannot mint a ledger or Bus event.
    pub fn terminal_revision_source(
        &self,
        session_id: &str,
        source_revision: u64,
    ) -> Result<String, UserRevisionRefusal> {
        if self.direct_document_is_committed() {
            self.authenticated_direct_document(session_id, source_revision)?;
        } else {
            self.authenticated_revision_occurrences(session_id, source_revision)?;
        }
        Ok(self.selected_text())
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
        let session_id = if let Some(owner) = self.direct_document.keys().next() {
            self.authenticated_direct_document(&owner.session, self.revision)
                .ok()?;
            owner.session.clone()
        } else {
            if !(self.terminal || self.terminal_sealed) {
                return None;
            }
            self.document_by_occurrence.keys().next()?.session.clone()
        };
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
    ///
    /// Stop may append sentence-end punctuation only. Existing bytes, including
    /// human edits and unresolved early formatting, never reopen at closure.
    pub fn light_plus_intent(
        &self,
        _ledger: &AcousticLedger,
        _sentence_pause_sec: f32,
    ) -> Option<UserRevisionIntent> {
        let session_id = self.document_by_occurrence.keys().next()?.session.clone();
        let source = self.committed_rendered_text();
        if source.trim().is_empty() {
            return None;
        }
        // Stop supplies only the sentence-end boundary. All casing, comma and
        // whitespace shaping happens progressively on source-bound live spans.
        // Even a long last owner or an invalidated cache cannot reopen a prefix.
        let mut shaped = source.clone();
        append_terminal_sentence_boundary(&mut shaped);
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
        // Lifecycle closure is not a lexical or acoustic disposition.
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

    /// Retain a source-bound derived version of one sealed occurrence's
    /// committed label. Formatting is text processing, not an acoustic
    /// observer: this corridor never admits a Formatter word observation,
    /// changes Raw revision, relabels a ledger slot, opens or closes a frontier,
    /// or issues a seal. The compare-and-swap is the source itself — the last
    /// mutation that produced the label, the ledger label and the document
    /// entry must all still match; otherwise the result is kept as
    /// `stale_source` and never delivered. Returns `true` when a derived
    /// version was recorded; it never produces a Raw revision.
    pub fn apply_occurrence_label_proposal(
        &mut self,
        ledger: &AcousticLedger,
        proposal: &OccurrenceLabelProposal,
    ) -> bool {
        if !proposal.binds_real_samples() {
            return false;
        }
        let occurrence = OccurrenceIdentity::new(
            proposal.session.clone(),
            proposal.capture_epoch,
            proposal.sample_start,
            proposal.sample_end,
        );
        // Only committed text is formatted; an unsealed label is still owned
        // by its acoustic producers.
        if !ledger.is_qualified(&occurrence)
            || !ledger.is_sealed(&occurrence)
            || ledger.text_of(&occurrence).is_none()
        {
            return false;
        }
        let Some(source) = proposal.source_observation.as_ref() else {
            return false;
        };
        // One derived version per source and mode: a repeated delivery of the
        // same result mints no second projection or Bus row.
        if self.derived_projections.iter().any(|projection| {
            projection.source_observation.as_ref() == Some(source)
                && projection.requested_mode == proposal.policy.as_str()
                && projection.source_raw_text == proposal.source_text
        }) {
            return false;
        }
        let candidate_label = proposal.proposed_label.trim();
        // A whole-document human edit leaves acoustic occurrence labels intact;
        // those labels alone therefore cannot authenticate a late formatter.
        let source_current = !self.raw_human_revision
            && proposal.source_observation.as_ref().is_some_and(|source| {
                source.occurrence == occurrence
                    && ledger
                        .layer_trail_for(&occurrence)
                        .filter(|decision| decision.decision.grants_mutation())
                        .last()
                        .is_some_and(|decision| decision.observation == *source)
                    && ledger.text_of(&occurrence) == Some(proposal.source_text.as_str())
                    && self
                        .document_by_occurrence
                        .get(&occurrence)
                        .is_some_and(|entry| entry.label == proposal.source_text)
            });
        {
            let source_revision = self
                .raw_revision_by_observation
                .get(source)
                .copied()
                .unwrap_or(0);
            let (formatted_text, status) = match proposal.disposition {
                LabelProposalDisposition::Propose if !candidate_label.is_empty() => {
                    (candidate_label.to_string(), AiFormatStatus::Applied)
                }
                LabelProposalDisposition::PreserveExisting => {
                    let preserved = self
                        .shaped_by_occurrence
                        .get(&occurrence)
                        .filter(|shaped| shaped.source_label == proposal.source_text)
                        .map_or_else(
                            || proposal.source_text.clone(),
                            |shaped| shaped.shaped_text.clone(),
                        );
                    (preserved, AiFormatStatus::Applied)
                }
                _ => (candidate_label.to_string(), AiFormatStatus::Failed),
            };
            let projection = self.mint_derived_projection(
                proposal.session.clone(),
                source_revision,
                proposal.source_text.clone(),
                proposal.policy,
                AiFormatResult {
                    text: formatted_text,
                    reasoning_text: None,
                    status,
                },
                proposal.source_observation.clone(),
            );
            if !source_current {
                let mut rejected = projection;
                rejected.status = "stale_source".into();
                rejected.publication_digest = rejected.digest();
                *self
                    .derived_projections
                    .last_mut()
                    .expect("minted derived projection") = rejected;
            }
        }
        true
    }

    fn explicit_raw_step_selected(&self) -> bool {
        self.document_timeline.steps.len() > 1
            && self
                .document_timeline
                .current()
                .is_some_and(|step| step.presentation.is_none())
    }

    fn needs_refused_occurrence_projection_wait(
        &self,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
    ) -> bool {
        if self.delivery_selection.is_some()
            || !matches!(
                policy,
                FormattingPolicy::Correction | FormattingPolicy::Smart
            )
            || self.document_by_occurrence.is_empty()
            || !self.direct_document.is_empty()
            || self.committed_rendered_text() != raw_at_handoff
            || self.raw_human_revision
            || self.manual_rendered_text.is_some()
            || !self.context_markers.is_empty()
            || self.explicit_raw_step_selected()
        {
            return false;
        }
        self.document_by_occurrence.values().any(|entry| {
            entry.seal_receipt.is_some()
                && !entry.label.trim().is_empty()
                && entry.manual_edit_receipt.is_none()
                && self.latest_source_projection(entry, policy).is_none()
        })
    }

    fn needs_occurrence_projection_wait(
        &self,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
    ) -> bool {
        if self.delivery_selection.is_some()
            || !matches!(
                policy,
                FormattingPolicy::Correction | FormattingPolicy::Smart
            )
            || self.document_by_occurrence.is_empty()
            || self.committed_rendered_text() != raw_at_handoff
            || self.raw_human_revision
            || !self.context_markers.is_empty()
            || !self
                .document_by_occurrence
                .values()
                .chain(self.direct_document.values())
                .all(|entry| entry.seal_receipt.is_some())
        {
            return false;
        }
        if self.derived_projections.iter().rev().any(|projection| {
            projection.source_observation.is_none()
                && projection.source_state == "raw_document"
                && projection.source_raw_revision == self.revision
                && projection.requested_mode == policy.as_str()
                && projection.source_raw_text == raw_at_handoff
        }) {
            return false;
        }
        let mut missing_any = false;
        for entry in self.document_by_occurrence.values() {
            if entry.label.trim().is_empty() || entry.manual_edit_receipt.is_some() {
                return false;
            }
            match self.latest_source_projection(entry, policy) {
                Some(projection) if projection.status == "applied" => {}
                Some(_) => return false,
                None => missing_any = true,
            }
        }
        missing_any
    }

    /// Record one controller-authenticated context reference. The captured
    /// position is applied to every later document render, so an early marker
    /// remains anchored as preceding occurrences arrive.
    pub fn record_context_marker(
        &mut self,
        position: usize,
        label: &str,
    ) -> Option<TranscriptRevision> {
        if self.document_by_occurrence.is_empty() && self.manual_document_capture.is_some() {
            // A marker cannot create or automatically revise a capture-only
            // human document. Explicit whole-document intents own that CAS.
            return None;
        }
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
        // Both lanes iterate in the same PCM occurrence order; a take never
        // holds the two at once, and the merged walk keeps the span honest
        // even for a reader who does not trust the gate.
        let mut entries: Vec<(&OccurrenceIdentity, &TranscriptDocumentEntry)> = self
            .document_by_occurrence
            .iter()
            .chain(self.direct_document.iter())
            .collect();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        for (occurrence, entry) in entries {
            if until.is_some_and(|limit| occurrence == limit) {
                break;
            }
            append_exact_fragment(&mut rendered, self.presentation_of(occurrence, entry));
        }
        rendered
    }

    /// Record the ordered invalidation boundary for one document edit.
    ///
    /// A new or relabelled owner drops computational memory and committed
    /// shapes from that owner forward. A same-label edit drops the successor
    /// only when its stored sentence break no longer matches the predecessor.
    /// Settled owners before the boundary are not hashed. Historical ledger
    /// receipts stay immutable.
    fn invalidate_for_document_edit(
        &mut self,
        occurrence: &OccurrenceIdentity,
        label_changed: bool,
        ledger: &AcousticLedger,
    ) {
        self.consultation_presentations
            .retain(|receipt| group_matches_entries(receipt, self.document_by_occurrence.values()));
        if label_changed {
            self.drop_shaping_from(occurrence);
            self.mark_shaping_affected(occurrence);
            return;
        }
        if self.successor_break_drifted(occurrence, ledger)
            && let Some(successor) = self.next_owner_after(occurrence)
        {
            self.drop_shaping_from(&successor);
            self.mark_shaping_affected(&successor);
        }
    }

    /// A committed shape changes the left context of every later owner.
    fn invalidate_shaped_suffix(&mut self, occurrence: &OccurrenceIdentity) {
        let Some(successor) = self.next_owner_after(occurrence) else {
            return;
        };
        self.drop_shaping_from(&successor);
        self.mark_shaping_affected(&successor);
    }

    /// The presentation bytes for one entry: its committed shape while that
    /// shape still matches the ledger label, otherwise the label itself.
    fn consulted_rendered_text(&self) -> String {
        let mut text = String::new();
        for (occurrence, entry) in &self.document_by_occurrence {
            let group = self
                .consultation_presentations
                .iter()
                .rev()
                .find(|receipt| {
                    group_matches_entries(receipt, self.document_by_occurrence.values())
                        && receipt
                            .members
                            .iter()
                            .any(|member| &member.occurrence == occurrence)
                });
            if let Some(group) = group {
                if &group.members[0].occurrence == occurrence {
                    append_exact_fragment(&mut text, &group.rendered_text);
                }
            } else {
                append_exact_fragment(&mut text, self.presentation_of(occurrence, entry));
            }
        }
        text
    }

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

    /// A6 uncertain-word spans over the committed document.
    ///
    /// Offsets are built by construction: this walk mirrors
    /// `rendered_occurrence_span` fragment-for-fragment and then applies the
    /// shared context-marker rule, so a span is never located by searching
    /// text. An entry contributes only while its presentation bytes are the
    /// raw ledger label — shaped (Light+), consultation, and manually revised
    /// occurrences honestly contribute nothing (d6). Live spans come only
    /// from Whisper / CloudLive slots (d7); Apple confidence travels into the
    /// ledger but does not paint live. Each word is its own span (d3).
    fn uncertain_spans(&self) -> Vec<UncertainSpan> {
        if self.manual_rendered_text.is_some() {
            return Vec::new();
        }
        let thresholds = word_confidence_thresholds();
        // (char_start, char_end) into the pre-marker document, plus evidence.
        let mut char_spans: Vec<(usize, usize, OccurrenceIdentity, WordConfidenceEvidence)> =
            Vec::new();
        let mut rendered = String::new();
        for (occurrence, entry) in &self.document_by_occurrence {
            let presentation = self.presentation_of(occurrence, entry);
            let eligible = presentation == entry.label && !entry.word_confidence.is_empty();
            let fragment_char_start = {
                append_exact_fragment(&mut rendered, presentation);
                rendered.chars().count() - presentation.chars().count()
            };
            if !eligible {
                continue;
            }
            let label = entry.label.as_str();
            let mut cursor = 0usize;
            for evidence in &entry.word_confidence {
                // Locate every token in composition order, uncertain or not:
                // the cursor is what keeps five identical words distinct.
                let Some(relative) = label
                    .get(cursor..)
                    .and_then(|tail| tail.find(&evidence.token))
                else {
                    continue;
                };
                let byte_start = cursor + relative;
                let byte_end = byte_start + evidence.token.len();
                cursor = byte_end;
                // d7: Apple slots carry confidence into the ledger but never
                // paint live; Lexicon/Formatter/ManualHuman carry none.
                if !matches!(
                    evidence.producer,
                    ObservationProducer::Whisper | ObservationProducer::CloudLive
                ) || !evidence.confidence.is_uncertain(&thresholds)
                {
                    continue;
                }
                let char_start = fragment_char_start + label[..byte_start].chars().count();
                let char_end = fragment_char_start + label[..byte_end].chars().count();
                char_spans.push((char_start, char_end, occurrence.clone(), evidence.clone()));
            }
        }
        if char_spans.is_empty() {
            return Vec::new();
        }
        // Apply the shared context-marker rule, shifting spans with each
        // insertion exactly as `render_context_markers` applies them.
        let mut ordered = self.context_markers.to_vec();
        ordered.sort_by(|left, right| {
            right
                .position
                .cmp(&left.position)
                .then_with(|| right.order.cmp(&left.order))
        });
        let mut chars: Vec<char> = rendered.chars().collect();
        for marker in ordered {
            let (byte_offset, insertion) =
                context_marker_insertion(&chars, marker.position, &marker.label);
            let offset = chars.iter().take(marker.position.min(chars.len())).count();
            debug_assert_eq!(
                chars
                    .iter()
                    .take(offset)
                    .map(|ch| ch.len_utf8())
                    .sum::<usize>(),
                byte_offset
            );
            let insertion_chars: Vec<char> = insertion.chars().collect();
            for (start, end, _, _) in &mut char_spans {
                if *start >= offset {
                    *start += insertion_chars.len();
                    *end += insertion_chars.len();
                } else if *end > offset {
                    *end += insertion_chars.len();
                }
            }
            chars.splice(offset..offset, insertion_chars);
        }
        // char offsets → UTF-16 code units on the final document.
        let mut utf16_offsets = Vec::with_capacity(chars.len() + 1);
        utf16_offsets.push(0u32);
        for ch in &chars {
            utf16_offsets.push(utf16_offsets.last().copied().unwrap_or(0) + ch.len_utf16() as u32);
        }
        char_spans
            .into_iter()
            .map(|(start, end, occurrence, evidence)| UncertainSpan {
                occurrence,
                slot_sample_start: evidence.token_sample_start.unwrap_or(0),
                slot_sample_end: evidence.token_sample_end.unwrap_or(0),
                utf16_start: utf16_offsets.get(start).copied().unwrap_or(0),
                utf16_end: utf16_offsets.get(end).copied().unwrap_or(0),
                producer: evidence.producer,
                source: evidence.confidence.source,
                milli_value: evidence.confidence.milli_value,
                surface_rewritten: evidence.surface_rewritten,
            })
            .collect()
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
        let (byte_offset, insertion) =
            context_marker_insertion(&chars, marker.position, &marker.label);
        rendered.insert_str(byte_offset, &insertion);
    }
    rendered
}

/// The single context-marker insertion rule: where the marker lands and what
/// bytes it carries (with edge spaces). Shared by the document renderer and
/// the A6 uncertain-span shifter so offsets can never drift apart.
fn context_marker_insertion(chars: &[char], position: usize, label: &str) -> (usize, String) {
    let offset = position.min(chars.len());
    let previous = offset.checked_sub(1).and_then(|index| chars.get(index));
    let next = chars.get(offset);
    let splits_word = previous.is_some_and(|ch| ch.is_alphanumeric())
        && next.is_some_and(|ch| ch.is_alphanumeric());
    let leading_space = !splits_word && previous.is_some_and(|ch| !ch.is_whitespace());
    let trailing_space = !splits_word && next.is_some_and(|ch| !ch.is_whitespace());
    let insertion = format!(
        "{}{}{}",
        if leading_space { " " } else { "" },
        label,
        if trailing_space { " " } else { "" }
    );
    let byte_offset = chars.iter().take(offset).map(|ch| ch.len_utf8()).sum();
    (byte_offset, insertion)
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
    pub untimed_final_words: usize,
    /// (phrase id, zero-based arrival position), including later timed re-seals.
    pub untimed_final_phrases: Vec<(u64, usize)>,
    closed_phrases: BTreeMap<u64, ClosedApplePhrase>,
    /// Acoustic provenance from this Raw paint.
    committed_sources: BTreeMap<OccurrenceIdentity, CommittedPaintSource>,
}

impl VisibleCanvasSnapshot {
    /// Late Apple words stay in the evidence book and never reach the canvas.
    /// Both pasted and slot-covered canvas counts are therefore always zero.
    pub fn late_apple_word_counts(&self) -> (usize, usize) {
        (0, 0)
    }

    /// Preview words inside the committed capture are the formatted document's
    /// raw restatement, including a pin that missed the Silero cut. A point at
    /// or past the last committed end is a tail, not an admission.
    fn admitted_into_committed_extent(&self, start: u64, end: u64) -> Option<String> {
        if self.committed_sources.is_empty() {
            return None;
        }
        let point = preview_sample_point(start, end);
        let extent_end = self
            .committed_sources
            .keys()
            .map(|owner| owner.sample_end)
            .max()?;
        if point >= extent_end {
            return None;
        }
        let owner = self
            .committed_sources
            .keys()
            .find(|owner| owner.sample_start <= point && point < owner.sample_end)
            .or_else(|| {
                self.committed_sources
                    .keys()
                    .find(|owner| owner.sample_end == extent_end)
            })?;
        Some(format!("admitted_into occurrence={owner:?}"))
    }

    /// Compare two reads by occurrence/offset or counted PCM word identity.
    /// A word in another range cannot hide a missing visible word.
    pub fn missing_words_from(&self, pasted: &Self) -> Vec<MissingVisibleWord> {
        let same_capture =
            self.session_id == pasted.session_id && self.capture_epoch == pasted.capture_epoch;
        let mut available = BTreeMap::<(u64, u64, String), usize>::new();
        let mut phrase_words = BTreeMap::<(u64, String), usize>::new();
        let mut open_revisions = BTreeMap::new();
        for present in &pasted.visible_words {
            if let VisibleWordSource::Unadmitted(current) = &present.source {
                match current.source {
                    UnadmittedAppleWordSource::OpenPartial { rev, phrase_id } => {
                        open_revisions.insert(phrase_id, rev);
                        *phrase_words
                            .entry((phrase_id, normalize_visible_word(&present.word)))
                            .or_default() += 1;
                        continue;
                    }
                    UnadmittedAppleWordSource::RefusedUntimed { phrase_id } => {
                        *phrase_words
                            .entry((phrase_id, normalize_visible_word(&present.word)))
                            .or_default() += 1;
                        continue;
                    }
                    _ => {}
                }
            }
            let range = match &present.source {
                VisibleWordSource::Unadmitted(current) => {
                    Some((current.sample_start, current.sample_end))
                }
                VisibleWordSource::Unanchored(observation) => Some((
                    observation.occurrence.sample_start,
                    observation.occurrence.sample_end,
                )),
                _ => None,
            };
            if let Some((start, end)) = range
                && present.covered_by.is_none()
            {
                *available
                    .entry((start, end, normalize_visible_word(&present.word)))
                    .or_default() += 1;
            }
        }
        self.visible_words.iter().filter_map(|word| {
            let reason = if !same_capture {
                Some("unaccounted".to_string())
            } else if let Some(occurrence) = &word.covered_by {
                Some(format!("covered_by_committed occurrence={occurrence:?}"))
            } else if let VisibleWordSource::Unadmitted(original) = &word.source {
                let phrase = match original.source {
                    UnadmittedAppleWordSource::OpenPartial { phrase_id, .. } => Some((phrase_id, true)),
                    UnadmittedAppleWordSource::RefusedUntimed { phrase_id } => Some((phrase_id, false)),
                    _ => None,
                };
                if let Some((phrase_id, was_open)) = phrase {
                    if was_open && let Some(rev) = open_revisions.get(&phrase_id) {
                        let key = (phrase_id, normalize_visible_word(&word.word));
                        if phrase_words.get_mut(&key).is_some_and(|count| {
                            if *count == 0 { false } else { *count -= 1; true }
                        }) {
                            None
                        } else {
                            Some(format!("superseded_by_partial rev={rev}"))
                        }
                    } else if !was_open && phrase_words.get_mut(&(phrase_id, normalize_visible_word(&word.word)))
                        .is_some_and(|count| {
                            if *count == 0 { false } else { *count -= 1; true }
                        }) {
                        None
                    } else if let Some(closed) = pasted.closed_phrases.get(&phrase_id) {
                        if !was_open && closed.outcomes.contains_key(
                            &codescribe_core::pipeline::contracts::ApplePhraseOutcome::Untimed) {
                            Some("unaccounted".into())
                        } else {
                            Some(format!("closed_by_final phrase={phrase_id} outcomes={}",
                                closed.describe_outcomes()))
                        }
                    } else if let Some(reason) = pasted.admitted_into_committed_extent(
                        original.sample_start,
                        original.sample_end,
                    ) {
                        Some(reason)
                    } else {
                        Some("unaccounted".into())
                    }
                } else {
                    let identity = (original.sample_start, original.sample_end, normalize_visible_word(&word.word));
                    if available.get_mut(&identity).is_some_and(|count| {
                        if *count == 0 { false } else { *count -= 1; true }
                    }) {
                        None
                    } else if let Some(reason) = pasted.admitted_into_committed_extent(
                        original.sample_start,
                        original.sample_end,
                    ) {
                        Some(reason)
                    } else {
                        let midpoint = preview_sample_point(original.sample_start, original.sample_end);
                        if let Some(observation) = pasted.visible_words.iter().find_map(|present| {
                            let VisibleWordSource::Unanchored(observation) = &present.source else { return None; };
                            let range = &observation.occurrence;
                            (range.sample_start <= midpoint && midpoint < range.sample_end).then_some(observation)
                        }) {
                            Some(format!("retained_as_evidence occurrence={:?}", observation.occurrence))
                        } else {
                            Some("unaccounted".into())
                        }
                    }
                }
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
            } else if let VisibleWordSource::Unanchored(observation) = &word.source {
                pasted.committed_sources.keys()
                    .find(|owner| range_within(&observation.occurrence, owner))
                    .map(|owner| format!("covered_by_committed occurrence={owner:?}"))
                    .or_else(|| Some("unaccounted".to_string()))
            } else {
                Some("unaccounted".to_string())
            };
            reason.map(|reason| MissingVisibleWord { word: word.word.clone(), reason })
        }).collect()
    }
}

#[derive(Debug, Default, Clone)]
struct StoredCursorPaint {
    text: String,
    pending_ranges: Vec<PendingRange>,
}

/// All target mutations are serialized through one mpsc worker, guaranteeing
/// that overlay deltas and the shared transcript snapshot see identical order.
pub struct PresentationEmitter {
    refinement_warnings: Option<Arc<std::sync::Mutex<Vec<String>>>>,
    cursor_observer: Option<CursorObserver>,
    /// Apple partial paint only; independent of reducer publication and delivery.
    apple_preview_observer: Option<ApplePreviewObserver>,
    speech_activity_observer: Option<SpeechActivityObserver>,
    cursor_capture: std::sync::OnceLock<(String, u64)>,
    cursor_sequence: std::sync::Mutex<u64>,
    cursor_integrity: std::sync::Mutex<Option<SpeechIntegrity>>,
    /// Last full paint, not a document or independently reconstructed delta.
    cursor_paint: std::sync::Mutex<StoredCursorPaint>,
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
    /// Per-take track switches from the frozen runtime snapshot, set once by
    /// [`Self::with_transcription_controls`] before capture opens.
    transcription_controls: TranscriptionControls,
    /// A terminal diagnostic cannot erase visible words before stop snapshots them.
    stop_snapshot_pending: std::sync::atomic::AtomicBool,
    /// Refused overlap pins in this take by ledger reason; see
    /// [`Self::overlap_refusal_repeats`].
    overlap_refusals: std::sync::Mutex<BTreeMap<String, u64>>,
    formatter_pending: std::sync::atomic::AtomicBool,
    derived_projection_notify: tokio::sync::Notify,
    /// Last whole formatted view published while the take is still capturing.
    live_view: formatter_live::LiveViewMemo,
    sentence_pause_sec: f32,
    tick_clock: Arc<dyn Fn() -> std::time::Duration + Send + Sync>,
    #[cfg(test)]
    paint_commands: std::sync::Mutex<Vec<String>>,
}

/// How stop-time formatter selection assembles occurrence projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionAssembly {
    TerminalDocument,
    RefusedPerOccurrence,
}

impl PresentationEmitter {
    /// One take can refuse an overlap pin thousands of times (24 646 lines
    /// in four days, 85% of engine warnings, on 2026-10-26). The first stays
    /// a warning, later ones go to debug, and finalisation reports the count
    /// for each ledger reason.
    fn overlap_refusal_repeats(&self, code: &str, message: &str) -> bool {
        if code != codescribe_core::pipeline::contracts::OVERLAP_PIN_REFUSED {
            return false;
        }
        let reason = codescribe_core::pipeline::contracts::overlap_pin_refusal_reason(message);
        let mut refusals = self
            .overlap_refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let earlier = !refusals.is_empty();
        *refusals.entry(reason.to_owned()).or_default() += 1;
        earlier
    }

    /// Report this take's refused overlap pins once, at finalisation, with
    /// the count for each ledger reason.
    fn report_overlap_refusals(&self) {
        let refusals = {
            let mut refusals = self
                .overlap_refusals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *refusals)
        };
        let refused: u64 = refusals.values().sum();
        if refused > 1 {
            tracing::info!(
                refused,
                reasons = ?refusals,
                "overlap_pin_refused repeated in this take; repeats are at debug level"
            );
        }
    }

    /// Fence only presentation callbacks; the retired take still drains into
    /// its own ledger, reducer, transcript buffer, and archive.
    pub fn retire_presentation(&self) {
        *self
            .active_presentation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
        self.derived_projection_notify.notify_waiters();
    }

    pub fn with_active_presentation(&self, publish: impl FnOnce()) {
        let active = self
            .active_presentation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
        let paint = reducer.read_paint();
        Some(VisibleCanvasSnapshot {
            session_id: session_id.clone(),
            capture_epoch: *capture_epoch,
            revision: reducer.revision,
            text: paint.text,
            preview_only_words: paint.preview_only_words,
            visible_words: paint.visible_words,
            committed_sources: paint.committed_sources,
            untimed_final_words: paint.untimed_final_words,
            untimed_final_phrases: reducer
                .closed_apple_phrases
                .iter()
                .filter(|(_, phrase)| phrase.was_untimed)
                .map(|(&id, phrase)| (id, phrase.arrival_index))
                .collect(),
            closed_phrases: reducer.closed_apple_phrases.clone(),
            has_committed_document: !reducer.document_by_occurrence.is_empty()
                || reducer.direct_document_is_committed(),
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
        // A ledger-free direct document freezes exactly as committed: actual
        // document presence was already required above, and no Light+ receipt
        // can be minted without the ledger — the bytes cross the stop boundary
        // unchanged rather than dressed in a foreign receipt.
        if self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .direct_document_is_committed()
        {
            return Ok(frozen);
        }
        let budget = self.schedule_light_plus();
        let authority = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = authority.lock().unwrap_or_else(|error| error.into_inner());
        {
            let state = self
                .session_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.authenticated_presentation_occurrences(&frozen.session_id, frozen.revision)?;
            if state.committed_rendered_text() != frozen.text {
                return Err(UserRevisionRefusal::StaleRevision {
                    expected: state.revision,
                    actual: frozen.revision,
                });
            }
        }
        let Some(commit) = self.run_light_plus_tick(&mut ledger, &budget, true) else {
            return Ok(frozen);
        };
        drop(ledger);
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
        let mut visible_words = commit
            .rendered_text
            .split_whitespace()
            .enumerate()
            .map(|(offset, word)| VisibleWord {
                word: word.to_string(),
                preview_rev: None,
                source: source.clone(),
                offset,
                covered_by: None,
            })
            .collect::<Vec<_>>();
        visible_words.extend(
            frozen
                .visible_words
                .iter()
                .filter(|word| word.covered_by.is_some())
                .cloned(),
        );
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
                        let active = worker_presentation
                            .lock()
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
            overlap_refusals: std::sync::Mutex::default(),
            transcription_controls: TranscriptionControls::default(),
            stop_snapshot_pending: std::sync::atomic::AtomicBool::new(false),
            formatter_pending: std::sync::atomic::AtomicBool::new(false),
            derived_projection_notify: tokio::sync::Notify::new(),
            live_view: formatter_live::LiveViewMemo::default(),
            sentence_pause_sec: 0.7,
            tick_clock: {
                let origin = std::time::Instant::now();
                Arc::new(move || origin.elapsed())
            },
            refinement_warnings: None,
            cursor_observer: None,
            apple_preview_observer: None,
            speech_activity_observer: None,
            cursor_capture: std::sync::OnceLock::new(),
            cursor_sequence: std::sync::Mutex::new(0),
            cursor_integrity: std::sync::Mutex::new(None),
            cursor_paint: std::sync::Mutex::new(StoredCursorPaint::default()),
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

    /// Counts-only processing receipts retained by the channel close owner.
    pub fn with_refinement_warnings(
        mut self,
        warnings: Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        self.refinement_warnings = Some(warnings);
        self
    }

    /// Set from the immutable settings generation held by this capture.
    pub fn with_sentence_pause_sec(mut self, seconds: f32) -> Self {
        self.sentence_pause_sec = seconds.clamp(0.3, 2.0);
        self
    }

    /// Freeze the per-take transcription track switches before capture opens.
    /// Called exactly once by the controller from the take's immutable runtime
    /// snapshot; the defaults keep both existing lanes for every old caller.
    /// With the seal ledger opted out, the reducer accepts the direct
    /// publication lane instead; with Apple live opted out, that lane's
    /// events never touch paint or document.
    pub fn with_transcription_controls(
        mut self,
        seal_ledger_enabled: bool,
        apple_live_enabled: bool,
    ) -> Self {
        let controls = TranscriptionControls {
            seal_ledger_enabled,
            apple_live_enabled,
        };
        self.transcription_controls = controls;
        self.session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .transcription_controls = controls;
        self
    }

    /// Observe ephemeral paint without granting document or delivery authority.
    pub fn with_cursor_observer(mut self, observer: CursorObserver) -> Self {
        self.cursor_observer = Some(observer);
        self
    }

    /// Observe the existing raw Apple preview without admitting or rewriting words.
    pub fn with_apple_preview_observer(mut self, observer: ApplePreviewObserver) -> Self {
        self.apple_preview_observer = Some(observer);
        self
    }

    /// Observe owned VAD edges independently of text, ledger and Apple controls.
    pub fn with_speech_activity_observer(mut self, observer: SpeechActivityObserver) -> Self {
        self.speech_activity_observer = Some(observer);
        self
    }

    fn paint_apple_preview(&self, text: &str) {
        if let Some(observer) = &self.apple_preview_observer {
            self.with_active_presentation(|| observer(text));
        }
    }

    #[cfg(test)]
    fn paint_cursor(&self, rendered: &str) {
        self.paint_cursor_with_ranges(rendered, Vec::new());
    }

    fn paint_cursor_with_ranges(&self, rendered: &str, pending_ranges: Vec<PendingRange>) {
        if self.cursor_observer.is_none() {
            return;
        }
        *self.cursor_paint.lock().unwrap_or_else(|e| e.into_inner()) = StoredCursorPaint {
            text: rendered.to_owned(),
            pending_ranges,
        };
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
        let voice = CursorVoice::from_integrity(integrity.as_ref());
        let stored_paint = self.cursor_paint.lock().unwrap_or_else(|e| e.into_inner());
        let paint_text = stored_paint.text.clone();
        let pending_ranges = stored_paint.pending_ranges.clone();
        // Snapshot under the sequence lock so a later sequence can never carry
        // older evidence. Callers never hold the reducer lock while painting.
        let state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut evidence = state.unanchored_evidence(session_id, *capture_epoch);
        evidence.extend(
            state
                .unadmitted_apple_words
                .iter()
                .filter(|word| {
                    matches!(
                        word.source,
                        UnadmittedAppleWordSource::OpenPartial { .. }
                            | UnadmittedAppleWordSource::RefusedUntimed { .. }
                    ) || state
                        .committed_covering(word.sample_start, word.sample_end)
                        .is_none()
                })
                .map(|word| UnanchoredEvidence {
                    sample_start: word.sample_start,
                    sample_end: word.sample_end,
                    text: word.text.clone(),
                    reason: match &word.source {
                        UnadmittedAppleWordSource::OpenPartial { rev, phrase_id } => {
                            format!("OpenPartial {{ rev: {rev}, phrase_id: {phrase_id} }}")
                        }
                        UnadmittedAppleWordSource::Pending { utterance_id } => {
                            format!("Pending {{ utterance_id: {utterance_id} }}")
                        }
                        UnadmittedAppleWordSource::Unmatched => "Unmatched".to_string(),
                        UnadmittedAppleWordSource::RefusedUntimed { phrase_id } => {
                            format!("RefusedUntimed {{ phrase_id: {phrase_id} }}")
                        }
                    },
                }),
        );
        drop(state);
        evidence.sort_by_key(|item| (item.sample_start, item.sample_end));
        *sequence = next;
        self.with_active_presentation(|| {
            observer(&CompactProjection {
                session_id: session_id.clone(),
                capture_epoch: *capture_epoch,
                sequence: next,
                // Earlier recovery debt must not hide words arriving now. Amber
                // stays authoritative until that debt is actually resolved.
                text: if degraded && paint_text.is_empty() {
                    "…".into()
                } else {
                    paint_text
                },
                degraded,
                pending_ranges: if degraded && stored_paint.text.is_empty() {
                    Vec::new()
                } else {
                    pending_ranges
                },
                voice,
                evidence,
            })
        });
    }

    /// Whether this take promised literal words (see [`Self::set_literal_delivery`]).
    pub fn literal_delivery(&self) -> bool {
        self.literal_delivery
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Signal the emitter to finish after every queued reducer revision.
    pub async fn finish(&mut self) {
        self.paint_apple_preview("");
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
                let canvas = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .read_paint();
                *paint = canvas.text;
                #[cfg(test)]
                self.paint_commands.lock().unwrap().push(paint.clone());
                self.paint_cursor_with_ranges(paint, canvas.pending_ranges);
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
        let session = self
            .transcript_bus
            .as_ref()
            .map(|bus| bus.capture_session_id())
            .or_else(|| {
                self.cursor_capture
                    .get()
                    .map(|(session, _)| session.as_str())
            })
            .or_else(|| {
                revision
                    .entries
                    .first()
                    .map(|entry| entry.occurrence.session.as_str())
            });
        let Some(session) = session else {
            return false;
        };
        revision.authenticates_publication(ledger, session)
    }

    /// Forward reducer observations to the overlay callback.
    ///
    /// A same-revision document fanout becomes one snapshot. Every other
    /// event, including lifecycle, delivery, and single-row updates, is
    /// forwarded unchanged. The Bus journal is not involved here.
    fn emit_overlay_events(&self, events: &[TranscriptBusEvidenceEvent]) {
        let Some(callback) = &self.projection_callback else {
            return;
        };
        if let Some(snapshot) = coalesced_overlay_document(events) {
            self.with_active_presentation(|| callback(&snapshot));
            return;
        }
        for event in events {
            self.with_active_presentation(|| callback(event));
        }
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
                self.emit_overlay_events(&events);
            }
        } else {
            return;
        }
        self.send_committed_paint(revision.rendered_text);
    }

    /// Publish a ledger-free revision. The Bus authenticates the embedded
    /// reducer capability; a refusal publishes nothing and paints nothing, so
    /// an unauthenticated direct snapshot never reaches delivery. The ordered
    /// paint/delivery FIFO is the same one the ledger lane uses.
    fn publish_direct_revision(&self, revision: TranscriptRevision) {
        if let Some(bus) = &self.transcript_bus {
            let events = bus.publish_direct_revision(&revision);
            if events.is_empty() {
                return;
            }
            self.emit_overlay_events(&events);
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
        mut intent: UserRevisionIntent,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let document_id = intent.session_id.clone();
        intent.session_id = self.capture_session_for_document(&document_id)?;
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        self.commit_document_revision(&mut ledger, DocumentRequest::Revise(intent))
            .map(|mut commit| {
                commit.session_id = document_id;
                commit
            })
    }

    /// Undo, Redo or a version pick: re-select accepted step `target` of the
    /// linear history. Saved bytes only; nothing is transcribed or formatted
    /// again. The projection callback repaints, as for every revision.
    pub fn navigate_document(
        &self,
        session_id: String,
        source_revision: u64,
        target: usize,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let document_id = session_id;
        let session_id = self.capture_session_for_document(&document_id)?;
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        self.commit_document_revision(
            &mut ledger,
            DocumentRequest::Navigate {
                session_id,
                source_revision,
                target,
            },
        )
        .map(|mut commit| {
            commit.session_id = document_id;
            commit
        })
    }

    pub fn owns_document(&self, document_id: &str) -> bool {
        self.transcript_bus
            .as_ref()
            .map(|bus| bus.session_id())
            .or_else(|| {
                self.cursor_capture
                    .get()
                    .map(|(session, _)| session.as_str())
            })
            .is_some_and(|session| session == document_id)
    }

    /// UI commands name the document; ledger operations name its physical
    /// capture. Neither a display label nor another document can select it.
    pub fn capture_session_for_document(
        &self,
        document_id: &str,
    ) -> Result<String, UserRevisionRefusal> {
        if !self.owns_document(document_id) {
            return Err(UserRevisionRefusal::SessionMismatch);
        }
        self.transcript_bus
            .as_ref()
            .map(|bus| bus.capture_session_id().to_string())
            .or_else(|| {
                self.cursor_capture
                    .get()
                    .map(|(session, _)| session.clone())
            })
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)
    }

    /// Steps and cursor of the take the reducer holds, with the revision a
    /// navigation must name. Another session reads as no history.
    pub fn document_timeline(&self, session_id: &str) -> (u64, DocumentTimeline) {
        let state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let owns_session = self
            .transcript_bus
            .as_ref()
            .map(|bus| bus.session_id())
            .or_else(|| {
                self.cursor_capture
                    .get()
                    .map(|(session, _)| session.as_str())
            })
            .is_some_and(|session| session == session_id);
        if !owns_session {
            return (state.revision, DocumentTimeline::default());
        }
        (state.revision, state.document_timeline().clone())
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
        drop(ledger);
        self.publish_derived_paint(&revision);
        Ok(UserRevisionCommit {
            session_id: revision.session_id.clone(),
            source_revision: revision.source_raw_revision,
            revision: revision.revision,
            rendered_text: revision.rendered_text.clone(),
            provenance_receipt: revision.receipt_id.clone(),
        })
    }

    fn publish_derived_paint(&self, projection: &DerivedTranscriptProjection) {
        if let Some(bus) = &self.transcript_bus {
            bus.record_derived_projection(projection);
            if let (Some(callback), Some(event)) = (
                &self.projection_callback,
                bus.derived_presentation_event(projection),
            ) {
                self.with_active_presentation(|| callback(&event));
            }
        }
        self.send_cmd(EmitterCmd::PaintEphemeralPreview(
            projection.rendered_text.clone(),
        ));
    }

    /// The one whole-document revision corridor: reducer mints the revision
    /// against the ledger, the Bus observes it, the delivery buffer follows.
    /// User edits and the terminal Light+ pass enter here. Formatter and Max
    /// results enter the independent derived projection corridor.
    fn commit_document_revision(
        &self,
        ledger: &mut AcousticLedger,
        request: DocumentRequest,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        let mut state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.document_by_occurrence.is_empty() && !state.direct_document_is_committed() {
            if !matches!(
                request.provenance(),
                DocumentRevisionProvenance::UserEdit
                    | DocumentRevisionProvenance::Retranscribe
                    | DocumentRevisionProvenance::Navigation
            ) {
                return Err(UserRevisionRefusal::NoCommittedDocument);
            }
            let (session, epoch) = self
                .cursor_capture
                .get()
                .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
            if session != request.session_id() {
                return Err(UserRevisionRefusal::SessionMismatch);
            }
            if state.revision != request.source_revision() {
                return Err(UserRevisionRefusal::StaleRevision {
                    expected: state.revision,
                    actual: request.source_revision(),
                });
            }
            state
                .revision
                .checked_add(1)
                .ok_or(UserRevisionRefusal::RevisionExhausted)?;
            if let DocumentRequest::Revise(intent) = &request
                && intent.rendered_text.trim().is_empty()
            {
                return Err(UserRevisionRefusal::EmptyText);
            }
            if self
                .cmd_tx
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .is_none_or(|sender| sender.is_closed())
            {
                return Err(UserRevisionRefusal::AuthorityUnavailable);
            }
            if !self.transcript_bus.as_ref().is_some_and(|bus| {
                bus.completed_capture_revision(session, *epoch, request.source_revision())
            }) {
                return Err(UserRevisionRefusal::NotTerminal);
            }
            let owner = state
                .manual_document_capture
                .as_ref()
                .filter(|owner| {
                    owner.matches(session, *epoch)
                        && ledger.authenticates_manual_document_capture(owner)
                })
                .cloned()
                .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
            if !owner.completed() {
                state.manual_document_capture = Some(
                    ledger
                        .complete_manual_document_capture(&owner, state.revision)
                        .map_err(UserRevisionRefusal::LedgerRefusal)?,
                );
            }
            state.mark_terminal_lifecycle();
        }
        let (revision, presentation) = match &request {
            DocumentRequest::Revise(intent) => (state.apply_user_revision(ledger, intent)?, None),
            DocumentRequest::Navigate {
                session_id,
                source_revision,
                target,
            } => state.navigate_document(ledger, session_id, *source_revision, *target)?,
            DocumentRequest::Format {
                session_id,
                source_revision,
                policy,
                result,
            } => {
                let (revision, projection) = state.accept_formatter_result(
                    ledger,
                    session_id,
                    *source_revision,
                    *policy,
                    result.clone(),
                )?;
                (revision, Some(projection))
            }
        };
        drop(state);
        let direct = matches!(revision.action, ReducerAction::ApplyDirectTranscript { .. });
        let authenticated = if direct {
            revision
                .authenticates_direct_publication(request.session_id())
                .is_some()
        } else {
            self.authenticates_revision(&revision, ledger)
        };
        if !authenticated {
            return Err(UserRevisionRefusal::LedgerRefusal(
                "publication_authentication_failed",
            ));
        }
        if let Some(bus) = &self.transcript_bus {
            let events = if direct {
                bus.publish_direct_revision(&revision)
            } else {
                bus.publish_revision(&revision, ledger)
            };
            if events.is_empty() {
                return Err(UserRevisionRefusal::LedgerRefusal(
                    "bus_publication_refused",
                ));
            }
            self.emit_overlay_events(&events);
        }
        self.send_committed_paint(revision.rendered_text.clone());
        // A formatted step is presented again over the receipt just published.
        if let Some(presentation) = &presentation {
            self.publish_derived_paint(presentation);
        }
        if let (DocumentRequest::Format { .. }, Some(projection)) = (&request, &presentation) {
            // The acknowledgement names the formatted version; Swift checks
            // it against the revision it asked to format.
            return Ok(UserRevisionCommit {
                session_id: projection.session_id.clone(),
                source_revision: request.source_revision(),
                revision: projection.revision,
                rendered_text: projection.rendered_text.clone(),
                provenance_receipt: projection.receipt_id.clone(),
            });
        }
        let provenance_receipt = match &revision.action {
            ReducerAction::ApplyUserRevision { receipt } => receipt.receipt_id.clone(),
            ReducerAction::ApplyDirectTranscript { capability } => {
                capability
                    .document_revision_receipt()
                    .expect("explicit direct operation")
                    .receipt_id
            }
            _ => unreachable!("document operation must mint a document revision"),
        };
        Ok(UserRevisionCommit {
            session_id: request.session_id().to_string(),
            source_revision: request.source_revision(),
            revision: revision.revision,
            rendered_text: revision.rendered_text,
            provenance_receipt,
        })
    }

    /// Light+ document shape at terminal or after a frozen Stop revision gains
    /// late words — capital at sentence starts, a closing period,
    /// hesitations preserved, punctuation seams collapsed — minted as one
    /// ledger-stamped document revision with provenance `light-plus`, so the
    /// Bus, the delivery buffer, and the formatter CAS all see the same bytes.
    /// It runs for every lane except the literal contract and never touches
    /// an occurrence label: the ledger stays the sole author of the words.
    fn schedule_light_plus(&self) -> codescribe_core::pipeline::light_plus::TickBudget {
        codescribe_core::pipeline::light_plus::TickBudget::schedule(self.tick_clock.clone())
    }

    fn light_plus_deadline_receipt(
        &self,
        budget: &codescribe_core::pipeline::light_plus::TickBudget,
    ) {
        if let Some(bus) = &self.transcript_bus
            && let Some(event) = bus.light_plus_deadline_event(budget.elapsed().as_millis())
            && let Some(callback) = &self.projection_callback
        {
            self.with_active_presentation(|| callback(&event));
        }
        self.on_event(&EngineEvent::Warning {
            code: "light_plus_tick_deadline".into(),
            message: format!(
                "Light+ skipped after {} ms from scheduling; delivered unchanged Raw.",
                budget.elapsed().as_millis()
            ),
        });
    }

    /// Prepare all work, including ledger receipts and authenticated rendering,
    /// against private candidates. Only this thread can admit them. Queue wait,
    /// locks, computation and final admission share the scheduled deadline.
    fn run_light_plus_tick(
        &self,
        ledger: &mut AcousticLedger,
        budget: &codescribe_core::pipeline::light_plus::TickBudget,
        terminal: bool,
    ) -> Option<UserRevisionCommit> {
        self.run_light_plus_tick_for(ledger, budget, terminal, &[])
    }

    fn run_light_plus_tick_for(
        &self,
        ledger: &mut AcousticLedger,
        budget: &codescribe_core::pipeline::light_plus::TickBudget,
        terminal: bool,
        named_occurrences: &[OccurrenceIdentity],
    ) -> Option<UserRevisionCommit> {
        if self.literal_delivery() {
            return None;
        }
        if budget.expired() {
            self.light_plus_deadline_receipt(budget);
            return None;
        }
        let state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Delivery has already selected its bytes: no tick may replace them.
        if state.delivery_selection.is_some() {
            return None;
        }
        let pause = self.sentence_pause_sec;
        // A bounded seal with no invalidated suffix has nothing to compute.
        // Proving that before the clone skips the reducer, the ledger, and
        // the preparation thread. A terminal pass still shapes the document.
        if !terminal
            && state
                .first_incremental_shaping_owner(named_occurrences, pause, ledger)
                .is_none()
        {
            return None;
        }
        let source_revision = state.revision;
        let source_derived_count = state.derived_projections.len();
        let mut candidate = state.clone();
        candidate.preparing_light_plus = true;
        drop(state);
        let mut candidate_ledger = ledger.clone();
        let named_occurrences = named_occurrences.to_vec();
        let prepared = budget.prepare(move |worker_budget| {
            let mut revisions = Vec::new();
            if terminal {
                if let Some(intent) = candidate.light_plus_intent(&candidate_ledger, pause) {
                    if worker_budget.expired() {
                        return None;
                    }
                    match candidate.apply_user_revision(&mut candidate_ledger, &intent) {
                        Ok(revision) => revisions.push(revision),
                        Err(_) => return None,
                    }
                }
            } else if let Some(start) = candidate.first_incremental_shaping_owner(
                &named_occurrences,
                pause,
                &candidate_ledger,
            ) {
                let mut presentation_changed = false;
                let mut unresolved = None;
                let owners = candidate
                    .document_by_occurrence
                    .range(start.clone()..)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                for occurrence in owners {
                    if worker_budget.expired() {
                        return None;
                    }
                    if !presentation_changed
                        && occurrence != start
                        && candidate.owner_shaping_stable(&occurrence, pause, &candidate_ledger)
                    {
                        continue;
                    }
                    match candidate.apply_incremental_shaping_with_pause(
                        &mut candidate_ledger,
                        &occurrence,
                        pause,
                    ) {
                        Ok(revision) => {
                            revisions.push(revision);
                            presentation_changed = true;
                        }
                        Err(
                            IncrementalShapingRefusal::Unchanged
                            | IncrementalShapingRefusal::AlreadyShaped,
                        ) => {}
                        Err(_) => {
                            if unresolved.is_none() {
                                unresolved = Some(occurrence);
                            }
                        }
                    }
                }
                if unresolved.is_none() {
                    candidate.shaping_affected_from = None;
                    candidate.shaping_pause_bits = Some(pause.to_bits());
                } else {
                    candidate.shaping_affected_from = unresolved;
                }
            }
            if worker_budget.expired() {
                return None;
            }
            if revisions.iter().any(|revision| {
                revision.entries.first().is_none_or(|entry| {
                    !revision
                        .authenticates_publication(&candidate_ledger, &entry.occurrence.session)
                })
            }) {
                return None;
            }
            Some((candidate, candidate_ledger, revisions))
        });
        let Some(Some((mut candidate, candidate_ledger, revisions))) = prepared else {
            if budget.expired() {
                self.light_plus_deadline_receipt(budget);
            }
            return None;
        };
        if revisions.is_empty() {
            if budget.expired() {
                self.light_plus_deadline_receipt(budget);
                return None;
            }
            let mut state = self
                .session_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.revision != source_revision
                || state.derived_projections.len() != source_derived_count
                || state.delivery_selection.is_some()
            {
                return None;
            }
            state
                .shaping_evaluations
                .clone_from(&candidate.shaping_evaluations);
            state
                .shaping_affected_from
                .clone_from(&candidate.shaping_affected_from);
            state.shaping_pause_bits = candidate.shaping_pause_bits;
            return None;
        }
        let mut state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.revision != source_revision
            || state.derived_projections.len() != source_derived_count
            || state.delivery_selection.is_some()
        {
            return None;
        }
        if budget.expired() {
            drop(state);
            self.light_plus_deadline_receipt(budget);
            return None;
        }
        candidate.preparing_light_plus = false;
        let previous_state = std::mem::replace(&mut *state, candidate);
        let previous_ledger = std::mem::replace(ledger, candidate_ledger);
        // The deadline includes the actual admission, not just calculation.
        // No observer has seen a candidate yet, so an expired commit rolls back.
        if budget.expired() {
            *state = previous_state;
            *ledger = previous_ledger;
            drop(state);
            self.light_plus_deadline_receipt(budget);
            return None;
        }
        drop(state);
        let commit = revisions.last().and_then(|revision| {
            if let ReducerAction::ApplyUserRevision { receipt } = &revision.action {
                Some(UserRevisionCommit {
                    session_id: receipt.session_id.clone(),
                    source_revision: receipt.source_revision,
                    revision: revision.revision,
                    rendered_text: revision.rendered_text.clone(),
                    provenance_receipt: receipt.receipt_id.clone(),
                })
            } else {
                None
            }
        });
        for revision in revisions {
            revision.record_trail();
            if let Some(bus) = &self.transcript_bus {
                let events = bus.publish_revision(&revision, ledger);
                self.emit_overlay_events(&events);
            }
            self.send_committed_paint(revision.rendered_text);
        }
        commit
    }

    fn mint_light_plus_revision(
        &self,
        ledger: &mut AcousticLedger,
        budget: &codescribe_core::pipeline::light_plus::TickBudget,
    ) {
        self.run_light_plus_tick(ledger, budget, true);
    }

    fn mint_incremental_light_plus(
        &self,
        ledger: &mut AcousticLedger,
        occurrences: &[OccurrenceIdentity],
        budget: &codescribe_core::pipeline::light_plus::TickBudget,
    ) {
        self.run_light_plus_tick_for(ledger, budget, false, occurrences);
    }

    /// Finish only this settled participant's lifecycle. Other participants
    /// still own the live ledger; detaching must never seal their speech.
    pub(crate) fn note_channel_detached(&self) {
        let mut state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.mark_terminal_lifecycle();
        // The settled interval owns a terminal direct document, even while
        // other recipients keep the common recognizer running. This capability
        // authenticates only its existing observations, never an acoustic seal.
        let revision = self
            .cursor_capture
            .get()
            .and_then(|(session, epoch)| state.apply_direct_transcript_end(session, *epoch));
        drop(state);
        if let Some(revision) = revision {
            self.publish_direct_revision(revision);
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
        let session_id = self.capture_session_for_document(session_id)?;
        self.session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .terminal_revision_source(&session_id, source_revision)
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
        let mut request = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .terminal_formatter_request()?;
        if let Some(bus) = &self.transcript_bus {
            if request.session_id != bus.capture_session_id() {
                return None;
            }
            request.session_id = bus.session_id().to_string();
        }
        Some(request)
    }

    /// Inspect admitted Raw labels before presentation shaping in falsifiers.
    #[cfg(test)]
    pub(crate) fn raw_entries_for_test(&self) -> Vec<TranscriptDocumentEntry> {
        let state = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.document_by_occurrence.values().cloned().collect()
    }

    /// Admit only a successful formatter result as a source-bound derived
    /// version. Failure, skipped policy, and healthy no-op are visible
    /// refusals and therefore mint no ledger/history/Copy-last evidence.
    pub fn apply_formatter_revision(
        &self,
        session_id: String,
        source_revision: u64,
        policy: FormattingPolicy,
        result: AiFormatResult,
    ) -> Result<UserRevisionCommit, UserRevisionRefusal> {
        match result.status {
            AiFormatStatus::Applied => {}
            AiFormatStatus::Failed => return Err(UserRevisionRefusal::FormatterFailed),
            AiFormatStatus::Skipped => return Err(UserRevisionRefusal::FormatterUnavailable),
            AiFormatStatus::AiNoop => return Err(UserRevisionRefusal::FormatterNoop),
        }
        let document_id = session_id;
        let session_id = self.capture_session_for_document(&document_id)?;
        let ledger = self
            .acoustic_ledger
            .as_ref()
            .ok_or(UserRevisionRefusal::AuthorityUnavailable)?;
        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        self.commit_document_revision(
            &mut ledger,
            DocumentRequest::Format {
                session_id,
                source_revision,
                policy,
                result,
            },
        )
        .map(|mut commit| {
            commit.session_id = document_id;
            commit
        })
    }

    /// Wait while background occurrence formatter jobs are still in flight for
    /// this active presentation and the current sealed document lacks complete
    /// per-occurrence projections for `policy`.
    pub async fn wait_for_occurrence_projections(
        &self,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
        timeout: std::time::Duration,
    ) {
        if timeout.is_zero() {
            return;
        }
        let _ = tokio::time::timeout(timeout, async {
            loop {
                let notified = self.derived_projection_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let should_wait = {
                    let active = *self
                        .active_presentation
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if !active
                        || !self
                            .formatter_pending
                            .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        false
                    } else {
                        let reducer = self
                            .session_state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        reducer.needs_occurrence_projection_wait(policy, raw_at_handoff)
                    }
                };
                if !should_wait {
                    break;
                }
                notified.await;
            }
        })
        .await;
    }

    /// Wait while background occurrence formatter jobs are still in flight for
    /// this active presentation and a refused take still has sealed occurrences
    /// without a formatter projection for `policy`. The retained job owner
    /// enforces `retained_formatter_deadline` and clears `formatter_pending`
    /// on completion, timeout, panic, or cancellation.
    pub async fn wait_for_refused_occurrence_projections(
        &self,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
    ) {
        loop {
            let notified = self.derived_projection_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let should_wait = {
                let active = *self
                    .active_presentation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if !active
                    || !self
                        .formatter_pending
                        .load(std::sync::atomic::Ordering::SeqCst)
                {
                    false
                } else {
                    let reducer = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    reducer.needs_refused_occurrence_projection_wait(policy, raw_at_handoff)
                }
            };
            if !should_wait {
                break;
            }
            notified.await;
        }
    }

    /// Freeze one terminal version. Current accepted occurrence formats keep
    /// their place; missing or failed fragments retain their committed text.
    /// Late formatter answers never trigger another delivery.
    pub fn delivery_projection(
        &self,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
    ) -> Option<DerivedTranscriptProjection> {
        self.delivery_projection_with_assembly(
            None,
            policy,
            raw_at_handoff,
            ProjectionAssembly::TerminalDocument,
        )
    }

    pub fn delivery_projection_with_assembly(
        &self,
        take_id: Option<&str>,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
        assembly: ProjectionAssembly,
    ) -> Option<DerivedTranscriptProjection> {
        if let Some(document) = take_id.map(|id| id.strip_suffix(":stopping").unwrap_or(id))
            && !self.owns_document(document)
        {
            return None;
        }
        if matches!(
            policy,
            FormattingPolicy::Correction | FormattingPolicy::Smart
        ) && let Some(projection) =
            self.occurrence_delivery_projection(take_id, policy, raw_at_handoff)
        {
            return Some(projection);
        }
        if assembly == ProjectionAssembly::RefusedPerOccurrence {
            return None;
        }
        let mut reducer = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(selected) = &reducer.delivery_selection {
            return Some(selected.clone());
        }
        let session_id = reducer
            .document_by_occurrence
            .keys()
            .next()
            .or_else(|| reducer.direct_document.keys().next())?
            .session
            .clone();
        let exact_raw = reducer.committed_rendered_text() == raw_at_handoff;
        let request = TerminalFormatterRequest {
            session_id,
            source_revision: if exact_raw { reducer.revision } else { 0 },
            source_text: raw_at_handoff.to_string(),
        };
        let mut text = String::new();
        let mut complete = policy != FormattingPolicy::Off
            && reducer.committed_rendered_text() == raw_at_handoff
            && reducer
                .document_by_occurrence
                .values()
                .chain(reducer.direct_document.values())
                .all(|entry| entry.seal_receipt.is_some())
            && !reducer.raw_human_revision
            && reducer.context_markers.is_empty();
        if let Some(whole) = reducer
            .derived_projections
            .iter()
            .rev()
            .find(|projection| {
                projection.source_observation.is_none()
                    && projection.source_state == "raw_document"
                    && projection.source_raw_revision == request.source_revision
                    && projection.requested_mode == policy.as_str()
                    && projection.source_raw_text == raw_at_handoff
            })
            .cloned()
        {
            reducer.delivery_selection = Some(whole.clone());
            drop(reducer);
            self.publish_derived_paint(&whole);
            return Some(whole);
        }
        if policy == FormattingPolicy::Max
            && exact_raw
            && !reducer.raw_human_revision
            && !reducer.consultation_presentations.is_empty()
        {
            text = reducer.consulted_rendered_text();
            complete = true;
        } else {
            for entry in reducer.document_by_occurrence.values() {
                match reducer.latest_source_projection(entry, policy) {
                    Some(projection) if projection.status == "applied" => {
                        append_exact_fragment(&mut text, &projection.rendered_text)
                    }
                    _ => {
                        complete = false;
                        break;
                    }
                }
            }
        }
        let result = AiFormatResult {
            text: if policy == FormattingPolicy::Off {
                request.source_text.clone()
            } else if complete {
                text
            } else {
                request.source_text.clone()
            },
            reasoning_text: None,
            status: if complete || policy == FormattingPolicy::Off {
                AiFormatStatus::Applied
            } else {
                AiFormatStatus::Failed
            },
        };
        let mut projection = reducer.mint_derived_projection(
            request.session_id,
            request.source_revision,
            request.source_text,
            policy,
            result,
            None,
        );
        if !exact_raw {
            projection.source_state = "stop_canvas_unsettled".into();
            projection.publication_digest = projection.digest();
            *reducer
                .derived_projections
                .last_mut()
                .expect("minted selection") = projection.clone();
        }
        // A formatted delivery over the exact committed document is the take's
        // first accepted format: Undo returns to the Raw behind it.
        if exact_raw
            && policy != FormattingPolicy::Off
            && projection.status == "applied"
            && projection.rendered_text != raw_at_handoff
        {
            let session_id = projection.session_id.clone();
            reducer.anchor_document_timeline(&session_id);
            let step = reducer.snapshot_step(
                "formatter",
                Some(policy.as_str().to_string()),
                projection.receipt_id.clone(),
                Some(projection.clone()),
            );
            reducer.push_document_step(step);
        }
        reducer.delivery_selection = Some(projection.clone());
        drop(reducer);
        self.publish_derived_paint(&projection);
        Some(projection)
    }

    fn occurrence_delivery_projection(
        &self,
        take_id: Option<&str>,
        policy: FormattingPolicy,
        raw_at_handoff: &str,
    ) -> Option<DerivedTranscriptProjection> {
        if self.literal_delivery()
            || !matches!(
                policy,
                FormattingPolicy::Correction | FormattingPolicy::Smart
            )
        {
            return None;
        }
        let mut reducer = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(selected) = &reducer.delivery_selection {
            return (selected.status == "applied").then(|| selected.clone());
        }
        let session_id = reducer
            .document_by_occurrence
            .keys()
            .next()
            .map(|occurrence| occurrence.session.clone())?;
        if let Some(document) = take_id.map(|id| id.strip_suffix(":stopping").unwrap_or(id))
            && self.capture_session_for_document(document).ok().as_deref()
                != Some(session_id.as_str())
        {
            return None;
        }
        if reducer.document_by_occurrence.is_empty()
            || !reducer.direct_document.is_empty()
            || reducer.committed_rendered_text() != raw_at_handoff
            || reducer.raw_human_revision
            || !reducer.context_markers.is_empty()
            || reducer.explicit_raw_step_selected()
            || reducer
                .document_by_occurrence
                .keys()
                .any(|occurrence| occurrence.session != session_id)
        {
            return None;
        }
        // A whole accepted version remains ahead of occurrence assembly.
        if let Some(whole) = reducer
            .derived_projections
            .iter()
            .rev()
            .find(|projection| {
                projection.source_observation.is_none()
                    && projection.source_state == "raw_document"
                    && projection.source_raw_revision == reducer.revision
                    && projection.requested_mode == policy.as_str()
                    && projection.source_raw_text == raw_at_handoff
                    && projection.status == "applied"
            })
            .cloned()
        {
            reducer.delivery_selection = Some(whole.clone());
            drop(reducer);
            self.publish_derived_paint(&whole);
            return Some(whole);
        }
        // Non-human manual presentation is Light+'s owned Stop boundary. It
        // may append a period, but cannot replace the accepted live prefix.
        let closes_sentence = if let Some(manual) = &reducer.manual_rendered_text {
            let occurrences = reducer.rendered_occurrence_span(None);
            match manual.strip_prefix(&occurrences) {
                Some("") => false,
                Some(".") => true,
                _ => return None,
            }
        } else {
            false
        };
        let mut text = String::new();
        let mut any_formatted = false;
        for (occurrence, entry) in &reducer.document_by_occurrence {
            let formatted = if entry.manual_edit_receipt.is_none() {
                reducer
                    .latest_source_projection(entry, policy)
                    .filter(|projection| {
                        projection.status == "applied"
                            && !projection.rendered_text.trim().is_empty()
                    })
                    .map(|projection| projection.rendered_text.clone())
            } else {
                None
            };
            if let Some(formatted) = formatted {
                append_exact_fragment(&mut text, &formatted);
                any_formatted = true;
            } else {
                append_exact_fragment(&mut text, reducer.presentation_of(occurrence, entry));
            }
        }
        if !any_formatted || text.trim().is_empty() {
            return None;
        }
        if closes_sentence {
            append_terminal_sentence_boundary(&mut text);
        }
        let source_revision = reducer.revision;
        let mut projection = reducer.mint_derived_projection(
            session_id.clone(),
            source_revision,
            raw_at_handoff.to_string(),
            policy,
            AiFormatResult {
                text: text.clone(),
                reasoning_text: None,
                status: AiFormatStatus::Applied,
            },
            None,
        );
        if projection.rendered_text != text {
            projection.rendered_text = text.clone();
            projection.publication_digest = projection.digest();
            *reducer
                .derived_projections
                .last_mut()
                .expect("minted occurrence selection") = projection.clone();
        }
        if projection.rendered_text != raw_at_handoff {
            reducer.anchor_document_timeline(&session_id);
            let step = reducer.snapshot_step(
                "formatter",
                Some(policy.as_str().to_string()),
                projection.receipt_id.clone(),
                Some(projection.clone()),
            );
            reducer.push_document_step(step);
        }
        reducer.delivery_selection = Some(projection.clone());
        drop(reducer);
        self.publish_derived_paint(&projection);
        Some(projection)
    }

    pub(crate) fn record_projection_delivery(
        &self,
        payload: &str,
        disposition: super::transcript_bus::TranscriptDelivery,
    ) {
        let reducer = self
            .session_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let (Some(bus), Some(selection)) = (&self.transcript_bus, &reducer.delivery_selection) {
            bus.record_projection_delivery(selection, payload, disposition);
        }
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
    fn close_participation(&self, session_id: &str, capture_epoch: u64) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.cursor_capture.get().is_some_and(|(session, epoch)| {
                session == session_id && *epoch == capture_epoch && capture_epoch != 0
            }),
            "document participation completion capture identity mismatch"
        );
        self.note_channel_detached();
        Ok(())
    }

    fn wait_presentation_published<'a>(
        &'a self,
        session_id: &'a str,
        capture_epoch: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            anyhow::ensure!(
                self.cursor_capture.get().is_some_and(|(session, epoch)| {
                    session == session_id && *epoch == capture_epoch && capture_epoch != 0
                }),
                "presentation completion capture identity mismatch"
            );
            anyhow::ensure!(
                self.wait_paint_published().await,
                "presentation FIFO closed without a drain acknowledgement"
            );
            Ok(())
        })
    }

    fn consultation_destinations(&self) -> usize {
        1
    }

    fn on_formatter_pending(&self, pending: bool) {
        self.formatter_pending
            .store(pending, std::sync::atomic::Ordering::SeqCst);
        if !pending {
            self.derived_projection_notify.notify_waiters();
        }
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
                .is_some_and(|bus| bus.capture_session_id() != session_id)
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
            if let Some(bus) = &self.transcript_bus {
                bus.observe_capture_opened(session_id, capture_epoch);
            }
            if let Some(ledger) = &self.acoustic_ledger {
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                match ledger.bind_manual_document_capture(session_id, capture_epoch) {
                    Ok(owner) => {
                        self.session_state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .manual_document_capture = Some(owner);
                    }
                    Err(reason) => {
                        tracing::warn!(reason, "manual document capture ownership refused")
                    }
                }
            }
            self.repaint_cursor();
        }
    }

    /// Route an `EngineEvent` into reducer state and ordered delta delivery.
    fn on_event(&self, event: &EngineEvent) {
        // Declared first, so it drops last: every ledger and reducer guard of
        // the arms below is already released when the live view republishes.
        let _live_view = formatter_live::LiveViewRefresh::after(self, event);
        let tick_budget = self.schedule_light_plus();
        // Apple-live opt-out: that lane's events never touch paint, mirror, or
        // document. A late callback after the lane's own retirement dies here.
        if !self.transcription_controls.apple_live_enabled
            && matches!(
                event,
                EngineEvent::Preview { .. }
                    | EngineEvent::UnadmittedAppleWords { .. }
                    | EngineEvent::PreviewDisposition { .. }
            )
        {
            return;
        }
        // Seal-ledger opt-out: the ledger lane holds no document authority for
        // this take. The direct publication lane below is the only one open.
        if !self.transcription_controls.seal_ledger_enabled
            && matches!(
                event,
                EngineEvent::LedgerMutation { .. }
                    | EngineEvent::LedgerSeal { .. }
                    | EngineEvent::SealCoverage { .. }
                    | EngineEvent::OccurrenceLabelProposal { .. }
            )
        {
            return;
        }
        match event {
            EngineEvent::DecoderCovered { .. } => {}
            EngineEvent::DirectTranscript {
                observation,
                text,
                final_chunk,
            } => {
                // Direct events count only inside the capture they name.
                if self.cursor_capture.get().is_none_or(|(session, epoch)| {
                    session != &observation.occurrence.session
                        || *epoch != observation.occurrence.capture_epoch
                }) {
                    return;
                }
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .apply_direct_transcript(observation, text, *final_chunk);
                if let Some(revision) = revision {
                    self.publish_direct_revision(revision);
                }
            }
            EngineEvent::DirectTranscriptEnd {
                session_id,
                capture_epoch,
            } => {
                if self.cursor_capture.get().is_none_or(|(session, epoch)| {
                    session != session_id || *epoch != *capture_epoch
                }) {
                    return;
                }
                let revision = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .apply_direct_transcript_end(session_id, *capture_epoch);
                if let Some(revision) = revision {
                    self.publish_direct_revision(revision);
                }
            }
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
                if self.cursor_capture.get().is_some_and(|(session, epoch)| {
                    session != &observation.occurrence.session
                        || *epoch != observation.occurrence.capture_epoch
                }) {
                    return;
                }
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
                let before_paint = matches!(receipt, MutationReceipt::Preserve { .. })
                    .then(|| state.visible_projection());
                let before_evidence = state.unanchored_evidence.len();
                let revision = state.apply_ledger_mutation(&ledger, observation, receipt);
                let evidence_changed = state.unanchored_evidence.len() != before_evidence;
                let visible = state.visible_projection();
                let coverage_changed = before_paint.is_some_and(|before| before != visible);
                drop(state);
                if evidence_changed {
                    self.repaint_cursor();
                }
                if unanchored {
                    drop(ledger);
                    if !visible.trim().is_empty() {
                        self.send_cmd(EmitterCmd::PaintEphemeralPreview(visible));
                    }
                    return;
                }
                if coverage_changed {
                    self.send_cmd(EmitterCmd::PaintEphemeralPreview(visible.clone()));
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
                        self.emit_overlay_events(&events);
                    }
                    self.send_cmd(EmitterCmd::PublishCommittedRevision {
                        paint: visible,
                        delivery: revision.rendered_text,
                    });
                    if was_stop_revision {
                        self.mint_light_plus_revision(&mut ledger, &tick_budget);
                    } else {
                        self.mint_incremental_light_plus(&mut ledger, &[], &tick_budget);
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
                // The channel follower consumes the terminal seal immediately.
                // Shape its settled Raw before that delivery boundary is published.
                if !receipt.is_occurrence_seal() && self.refinement_warnings.is_some() {
                    self.session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .mark_terminal_lifecycle();
                    self.mint_light_plus_revision(&mut ledger, &tick_budget);
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
                    self.emit_overlay_events(&events);
                }
                // The terminal seal closes the ledger's word authority; the
                // Light+ document revision follows immediately, before the controller
                // publishes `session_ended`, so the terminal projection Swift
                // holds already carries the shaped bytes and revision number.
                if terminal {
                    self.mint_light_plus_revision(&mut ledger, &tick_budget);
                } else {
                    // An occurrence seal closes exactly those words and nothing
                    // else. Retry presentation for any committed span that was
                    // not shaped at admission; the lifecycle stays open.
                    self.mint_incremental_light_plus(
                        &mut ledger,
                        &receipt.sealed_occurrences,
                        &tick_budget,
                    );
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
                    self.emit_overlay_events(&events);
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
                let (evidence_closed, derived_projection) = {
                    let mut reducer = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let before = reducer.unanchored_evidence.len();
                    let derived_before = reducer.derived_projections.len();
                    // A formatter result never seals and never revises Raw: the
                    // occurrence was sealed by its acoustic producers before the
                    // job was scheduled, and the result is a derived version.
                    let _ = reducer.apply_occurrence_label_proposal(&ledger, proposal);
                    let evidence_closed = reducer.unanchored_evidence.len() != before;
                    let derived = (reducer.derived_projections.len() > derived_before)
                        .then(|| reducer.derived_projections.last().cloned())
                        .flatten();
                    (evidence_closed, derived)
                };
                if let (Some(bus), Some(projection)) = (&self.transcript_bus, derived_projection) {
                    bus.record_derived_projection(&projection);
                }
                self.derived_projection_notify.notify_waiters();
                if evidence_closed {
                    self.repaint_cursor();
                }
                // The accepted fragment reaches the overlay as part of the whole
                // live view: `LiveViewRefresh` (derived paint, like `publish_derived_paint`)
                // publishes it when this event ends, after the ledger guard below is released.
                // Raw shaping does not depend on formatting; this tick is an
                // idempotent re-check of the unchanged acoustic label.
                self.mint_incremental_light_plus(
                    &mut ledger,
                    std::slice::from_ref(&occurrence),
                    &tick_budget,
                );
            }
            EngineEvent::VadStart { .. } => {}
            EngineEvent::VadEnd { .. } => self.paint_apple_preview(""),
            EngineEvent::SidebandEvidence { evidence } => {
                if let Some(observer) = &self.speech_activity_observer
                    && self.cursor_capture.get().is_some_and(|(session, epoch)| {
                        session == &evidence.range.session && *epoch == evidence.range.capture_epoch
                    })
                    && matches!(
                        &evidence.evidence,
                        SidebandEvidenceKind::SpeechStart { .. }
                            | SidebandEvidenceKind::SpeechEnd { .. }
                    )
                {
                    observer(evidence);
                }
                debug!(
                    sequence = evidence.sequence,
                    sample_start = evidence.range.sample_start,
                    sample_end = evidence.range.sample_end,
                    "PresentationEmitter observed sideband evidence without mutating text"
                );
            }
            EngineEvent::UnadmittedAppleWords {
                revision,
                words,
                closed_phrases,
            } => {
                {
                    let mut state = self
                        .session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if *revision <= state.apple_mirror_revision {
                        return;
                    }
                    state.apple_mirror_revision = *revision;
                    state.unadmitted_apple_words.clone_from(words);
                    state.closed_apple_phrases.clone_from(closed_phrases);
                }
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(String::new()));
            }
            EngineEvent::PreviewDisposition {
                superseded_through_rev,
                final_disposition,
                refused_evidence,
            } => {
                let mut state = self
                    .session_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.preview_disposition_receipts += 1;
                debug!(
                    superseded_through_rev,
                    ?final_disposition,
                    refused_slices = refused_evidence.len(),
                    receipts = state.preview_disposition_receipts,
                    "Apple phrase disposition receipt"
                );
            }
            EngineEvent::Preview { rev, text, pin } => {
                self.paint_apple_preview(text);
                debug!(
                    rev,
                    text_chars = text.chars().count(),
                    sample_start = pin.range.sample_start,
                    sample_end = pin.range.sample_end,
                    "Recognizer preview receipt; Apple mirror owns paint"
                );
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
                self.paint_apple_preview("");
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
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
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
                let canonical_text = {
                    let state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.visible_projection()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                // Capture is over, but terminal presentation authority stays
                // alive for an explicit overlay revision. The controller
                // replaces or drops it at the next take; `finish()` is for an
                // owner that truly retires the presentation.
            }
            EngineEvent::Warning { code, message } => {
                if self.overlap_refusal_repeats(code, message) {
                    tracing::debug!("Engine warning [{}]: {}", code, message);
                } else {
                    tracing::warn!("Engine warning [{}]: {}", code, message);
                }
                if (code.starts_with("live_refinement_")
                    || code.starts_with("tail_patch_")
                    || code == "agent_raw_whisper_disabled"
                    || code == "local_tail_patch_degraded"
                    || code == "layer1_lane_degraded"
                    || code == "light_plus_tick_deadline")
                    && let Some(warnings) = &self.refinement_warnings
                {
                    let mut warnings = warnings.lock().unwrap_or_else(|error| error.into_inner());
                    if !warnings.contains(code) {
                        warnings.push(code.clone());
                    }
                }
            }
            EngineEvent::SessionFinalised { .. } => {
                self.report_overlap_refusals();
                if self
                    .stop_snapshot_pending
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.session_state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .mark_terminal_lifecycle();
                    if self.transcription_controls.seal_ledger_enabled
                        && let Some(ledger) = &self.acoustic_ledger
                    {
                        let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                        self.mint_light_plus_revision(&mut ledger, &tick_budget);
                    }
                    return;
                }
                let canonical_text = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.mark_terminal_lifecycle();
                    state.visible_projection()
                };
                self.send_cmd(EmitterCmd::PaintEphemeralPreview(canonical_text));
                // Lifecycle end closes edit admission independently of the
                // seal verdict. Light+ is idempotent: a document the terminal
                // seal already shaped yields no second intent. A ledger-free
                // take has no Light+ authority; its document stands as
                // committed by the direct lane.
                if self.transcription_controls.seal_ledger_enabled
                    && let Some(ledger) = &self.acoustic_ledger
                {
                    let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                    self.mint_light_plus_revision(&mut ledger, &tick_budget);
                }
            }
        }
    }
}

/// Authority-bound presentation tests. These are preregistered for C12; C11
/// does not compile or execute them under the W2 embargo.
#[cfg(test)]
#[path = "emitter_tests.rs"]
mod tests;
