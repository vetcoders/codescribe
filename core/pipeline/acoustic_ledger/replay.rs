//! Deterministic replay and diagnostics: seal-coverage assessment and the
//! per-layer decision trail.
//!
//! [`SealCoverageReceipt`] says why a take is or is not fully covered, and the
//! [`LayerDecisionReceipt`] trail plus manual, consultation and shaping
//! receipts are what a replay reads back to reproduce the document.

use super::*;

// ---------------------------------------------------------------------------
// Per-layer decision trail
// ---------------------------------------------------------------------------

/// One layer's complete answer about one occurrence.
///
/// * inputs: every call to [`AcousticLedger::admit`], without exception.
/// * outputs: the inspectable Apple -> Whisper -> retained-text decision chain
///   behind a visible label.
/// * invariants: exactly one receipt per observation, appended in arrival
///   order and never rewritten. A later layer changing the visible label adds
///   a link to the chain; it does not erase the earlier one. The decision it
///   records is the very [`MutationReceipt`] the ledger returned, so the trail
///   cannot drift away from what actually happened.
/// * intended W2 consumers: the Transcript Bus evidence projection and the
///   overlay's per-word history.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerDecisionReceipt {
    /// Position of this receipt in the ledger-wide trail. Stable once appended.
    pub ordinal: usize,
    /// Stable identifier for cross-referencing from a projection.
    pub receipt_id: String,
    /// The observation that was judged.
    pub observation: ObservationIdentity,
    /// Label the layer proposed.
    pub candidate_label: String,
    /// The layer's own tokenization of that label, recorded rather than
    /// authored: the ledger never re-tokenizes on a producer's behalf.
    pub candidate_tokens: Vec<String>,
    /// Serials the occurrence held when the decision was taken. Empty means the
    /// occurrence had not been qualified, which is itself the evidence.
    pub serials: Vec<AcousticSerial>,
    /// The receipt the ledger actually returned.
    pub decision: MutationReceipt,
    /// Ordinal of the previous decision on the same occurrence, if any.
    pub predecessor_ordinal: Option<usize>,
    /// The span's character rate over its declared range exceeded the clock-lie
    /// bar. The text in `decision` is kept. This receipt is the flag.
    pub clock_lie: bool,
    /// Flagged occurrence that refused this replacement, if any.
    pub clock_lie_blocker: Option<OccurrenceIdentity>,
}

impl LayerDecisionReceipt {
    /// Producer that made the proposal.
    pub fn producer(&self) -> ObservationProducer {
        self.observation.producer
    }

    /// Layer label used by the runtime trace.
    pub fn layer(&self) -> &'static str {
        self.observation.producer.layer_label()
    }

    /// Producer-local generation of the proposal.
    pub fn generation(&self) -> u64 {
        self.observation.generation
    }

    /// Stable reason label for the decision.
    pub fn reason(&self) -> &'static str {
        self.decision.as_str()
    }

    /// Whether the decision was taken with acoustic evidence in hand.
    ///
    /// A trail entry without a serial is the N11/N12 fail-closed edge: the
    /// projection must refuse such a token rather than emit an unsigned one.
    pub fn is_evidence_backed(&self) -> bool {
        !self.serials.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Manual edit provenance
// ---------------------------------------------------------------------------

/// The only supersession a sealed label accepts.
///
/// * inputs: an explicit human edit submitted as an [`ObservationProducer::ManualHuman`]
///   observation on a sealed occurrence.
/// * outputs: provenance for the label change; the sealed acoustic serial and
///   the prior layer history are untouched.
/// * invariants: the seal itself is never lifted. The physical claim, its
///   serial and its decision chain stay exactly as sealed; only the lexical
///   label is superseded, and only with a named human receipt.
/// * intended W2 consumers: the Swift overlay's explicit edit path and the
///   Transcript Bus evidence projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualEditReceipt {
    /// Stable identifier for cross-referencing from a projection.
    pub receipt_id: String,
    /// The sealed physical event whose label was superseded.
    pub occurrence: OccurrenceIdentity,
    /// Identifier of the seal this edit supersedes the label of.
    pub supersedes_seal: String,
    /// Label that was visible before the edit.
    pub superseded_label: String,
    /// Label the human put in its place.
    pub label: String,
    /// The human observation that carried the edit.
    pub observation: ObservationIdentity,
}

/// Authenticated origin of a whole-document revision.
///
/// Both routes enter the same ledger + reducer corridor. The origin remains
/// explicit so a formatter result can never masquerade as a human correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentRevisionProvenance {
    UserEdit,
    /// Explicit whole-file button pass after capture lifecycle closure.
    Retranscribe,
    Formatter,
    /// Deterministic Light+ sentence shaping minted by Rust at the terminal
    /// seal, before any formatter or user edit sees the document.
    LightPlus,
    /// Undo, redo or a version pick: the reducer re-selects bytes it already
    /// accepted. A receipt for the move, never a new accepted operation.
    Navigation,
}

impl DocumentRevisionProvenance {
    /// Stable spelling used by ledger receipts and Bus consumers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserEdit => "user-edit",
            Self::Retranscribe => "retranscribe",
            Self::Formatter => "formatter",
            Self::LightPlus => "light-plus",
            Self::Navigation => "navigation",
        }
    }
}

/// Provenance for an explicit rewrite of a complete committed transcript.
///
/// The receipt names source occurrences and issued seals when a ledger owns
/// them. A direct reducer operation instead names its authenticated publication
/// in `capture_receipt_id`, with both acoustic source lists empty. Neither
/// path fabricates per-word alignment for the replacement text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManualDocumentRevisionReceipt {
    /// Stable document identifier, also copied into acoustic receipts when present.
    pub receipt_id: String,
    /// Stable origin label for quality capture and external observers.
    pub provenance: String,
    /// Recording session whose committed document was revised.
    pub session_id: String,
    /// Reducer revision the user actually edited.
    pub source_revision: u64,
    /// New reducer revision minted by Rust.
    pub revision: u64,
    /// Exact occurrence identities that made up the source document.
    pub source_occurrences: Vec<OccurrenceIdentity>,
    /// Issued occurrence seals, if any; absence never claims acoustic finality.
    pub source_seal_receipts: Vec<String>,
    /// Complete user-authored replacement bytes.
    pub rendered_text: String,
    /// Capture epoch for a document with no ledger-owned acoustic source entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_epoch: Option<u64>,
    /// Ledger capture ownership or an authenticated direct publication id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_receipt_id: Option<String>,
}

/// Ledger-minted ownership of one physical capture, held only in process.
/// Deliberately has no deserializer or public fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualDocumentCaptureReceipt {
    receipt_id: String,
    session_id: String,
    capture_epoch: u64,
    source_revision: Option<u64>,
}

impl ManualDocumentCaptureReceipt {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn matches(&self, session_id: &str, capture_epoch: u64) -> bool {
        self.session_id == session_id && self.capture_epoch == capture_epoch
    }

    pub fn completed(&self) -> bool {
        self.source_revision.is_some()
    }
}

/// One immutable acoustic source member claimed by a grouped Agent answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsultationPresentationMember {
    pub occurrence: OccurrenceIdentity,
    pub source_label: String,
    pub seal_receipt: String,
}

/// Borrowed claim; only the ledger can record its authenticated receipt.
pub struct ConsultationPresentationInput<'a> {
    pub consultation_id: &'a str,
    pub turn_id: &'a str,
    pub source_revision: u64,
    pub revision: u64,
    pub members: &'a [ConsultationPresentationMember],
    pub rendered_text: &'a str,
}

/// Presentation of a sealed group, not a whole-document edit or PCM label.
/// Revision numbers describe reducer admission, not provider request timing.
/// An unrelated suffix may advance the reducer while the answer is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsultationPresentationReceipt {
    pub receipt_id: String,
    pub consultation_id: String,
    pub turn_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub members: Vec<ConsultationPresentationMember>,
    pub rendered_text: String,
}

/// Provenance for one deterministic presentation shaping of a committed
/// occurrence, minted while the session lifecycle is still open.
///
/// * inputs: one admitted occurrence, the exact committed label it holds, and the
///   committed left neighbourhood the casing decision saw.
/// * outputs: the presentation bytes the reducer renders for that occurrence.
/// * invariants: the acoustic label, its evidence, its geometry and its seal
///   stay untouched — this receipt sits *beside* them. It names exactly one
///   occurrence and never asserts a seal that has not been issued. It is not a
///   [`ManualDocumentRevisionReceipt`]: a partial shape
///   must never be projected as a complete human document edit.
/// * intended consumers: the transcript reducer's rendered document and the
///   Transcript Bus projection of that revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalShapingReceipt {
    /// Stable identifier copied into the reducer entry and Bus projection.
    pub receipt_id: String,
    /// Stable origin label. Always `light-plus` today.
    pub provenance: String,
    /// Recording session whose occurrence was shaped.
    pub session_id: String,
    /// Reducer revision the shaping was computed against.
    pub source_revision: u64,
    /// New reducer revision minted by Rust.
    pub revision: u64,
    /// The one physical occurrence this shaping presents.
    pub occurrence: OccurrenceIdentity,
    /// Acoustic seal when one existed at shaping time; absent for live text.
    pub source_seal_receipt: Option<String>,
    /// Whether the PCM gap before this occurrence started a sentence.
    pub sentence_break_before: bool,
    /// Exact committed label the shape was derived from. A later relabel makes
    /// the shape stale and detectable rather than silently wrong.
    pub source_label: String,
    /// Exact historical context; its digest must never silently change.
    pub left_context: String,
    /// Digest of the committed left neighbourhood the casing decision saw.
    pub left_context_sha256: String,
    /// Presentation bytes for this occurrence.
    pub shaped_text: String,
}

/// The seven facts one incremental shaping asserts, borrowed for exactly the
/// length of the call that authenticates them.
///
/// * inputs: the session and the consecutive revision pair the reducer moves
///   between, the one committed occurrence being shaped, the exact committed
///   label it holds, the committed left neighbourhood the casing decision saw,
///   and the presentation bytes proposed for it.
/// * outputs: none. This value carries no verdict; every refusal and the only
///   receipt are still minted inside
///   [`AcousticLedger::record_incremental_shaping`], in its existing order.
/// * invariants: borrowed, so it cannot own or outlive the ledger state it
///   describes, and it authenticates nothing on its own — holding one proves
///   that a caller assembled a claim, never that the ledger accepted it.
/// * intended consumers: the transcript reducer's per-occurrence Light+ pass,
///   and the ledger tests that vary this claim one fact at a time.
#[derive(Debug, Clone, Copy)]
pub struct IncrementalShapingInput<'a> {
    /// Recording session the shaping is claimed for.
    pub session_id: &'a str,
    /// Reducer revision the shaping was computed against.
    pub source_revision: u64,
    /// New reducer revision the shaping would mint. Must follow
    /// `source_revision` with no gap.
    pub revision: u64,
    /// The one closed occurrence this shaping presents.
    pub occurrence: &'a OccurrenceIdentity,
    /// Exact committed label the shape claims to be derived from.
    pub source_label: &'a str,
    /// Committed left neighbourhood the casing decision saw.
    pub left_context: &'a str,
    /// Presentation bytes proposed for this occurrence.
    pub shaped_text: &'a str,
    pub sentence_break_before: bool,
}

/// Why no authenticated acoustic measurement backs a coverage verdict.
///
/// Absence of evidence is its own outcome. None of these may certify a
/// successful terminal seal, and none of them is silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticEvidenceGap {
    /// No acoustic observer measured this take at all.
    NotObserved,
    /// A measurement exists but names another session or capture epoch.
    IdentityMismatch,
    /// PCM reached the observer and some of it was not finite, so nothing it
    /// measured for this take can be trusted — an unmeasurable region reads as
    /// silence and there is no way to tell the two apart after the fact.
    InvalidMeasurement,
    /// The observer measured only part of the capture: committed speech or a
    /// measured span reaches past its extent, or that extent stops short of the
    /// PCM the take actually produced.
    PartialObservation,
}

impl AcousticEvidenceGap {
    /// Stable token for logs, receipts and the Bus projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotObserved => "not_observed",
            Self::IdentityMismatch => "identity_mismatch",
            Self::InvalidMeasurement => "invalid_measurement",
            Self::PartialObservation => "partial_observation",
        }
    }
}

/// Whether the committed occurrence union covers the measured speech span
/// closely enough to become terminal transcript truth.
///
/// [`Self::Complete`] is the only outcome that may certify one. It requires an
/// authenticated measurement — measured silence qualifies, missing measurement
/// does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealCoverageStatus {
    Complete,
    Incomplete,
    /// No authenticated measurement; the take's speech extent is unknown.
    Unavailable(AcousticEvidenceGap),
}

impl SealCoverageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Unavailable(_) => "unavailable",
        }
    }

    /// Whether this verdict may certify terminal transcript truth. Every
    /// finality consumer branches on this, so a new non-complete outcome can
    /// never fall through a guard that only knew one refusal.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Why measurement was missing, if it was.
    pub fn unavailable_reason(self) -> Option<AcousticEvidenceGap> {
        match self {
            Self::Unavailable(gap) => Some(gap),
            Self::Complete | Self::Incomplete => None,
        }
    }
}

/// Ledger-owned comparison of committed occurrence coverage against the
/// capture energy clock. Text never participates in this calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealCoverageReceipt {
    pub session_id: String,
    pub capture_epoch: u64,
    /// Bound capture clock for this take; absent when no clock was bound.
    pub sample_rate_hz: Option<u32>,
    pub speech_samples: u64,
    pub covered_samples: u64,
    pub uncovered_speech_ranges: Vec<TailSampleRange>,
    pub max_uncovered_samples: u64,
    pub incomplete_threshold_samples: u64,
    pub status: SealCoverageStatus,
    /// Which acoustic observer supplied the measurement this receipt judged.
    pub speech_producer: String,
    /// Availability token of that observer, kept for the log/Bus reader.
    pub availability: String,
    /// Contiguous PCM extent the observer measured. `None` when unavailable.
    pub observed_samples: Option<u64>,
}

impl SealCoverageReceipt {
    /// Covered fraction of measured speech.
    ///
    /// `None` when no authenticated measurement exists: an unknown extent has
    /// no ratio, and rendering it as `1.0` is exactly how absence used to look
    /// like a perfect take. Measured silence keeps a real `1.0` — there was
    /// nothing to cover and the observer was there to say so.
    pub fn coverage_ratio(&self) -> Option<f64> {
        if self.status.unavailable_reason().is_some() {
            return None;
        }
        if self.speech_samples == 0 {
            return Some(1.0);
        }
        Some(self.covered_samples as f64 / self.speech_samples as f64)
    }
}

/// Self-reporting whole-session text comparison. Both rendered values remain
/// evidence only; only occurrence-authenticated mutations can change text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptComparisonReceipt {
    pub apple_sha256: String,
    pub apple_char_count: u64,
    pub apple_rendered_text: String,
    pub final_pass_sha256: String,
    pub final_pass_char_count: u64,
    pub final_pass_rendered_text: String,
}

impl TranscriptComparisonReceipt {
    pub fn new(apple_rendered_text: String, final_pass_rendered_text: String) -> Self {
        let digest = |text: &str| hex::encode(Sha256::digest(text.as_bytes()));
        Self {
            apple_sha256: digest(&apple_rendered_text),
            apple_char_count: apple_rendered_text.chars().count() as u64,
            apple_rendered_text,
            final_pass_sha256: digest(&final_pass_rendered_text),
            final_pass_char_count: final_pass_rendered_text.chars().count() as u64,
            final_pass_rendered_text,
        }
    }
}

impl AcousticLedger {
    /// Store the latest capture-clock coverage fact for terminal admission.
    /// A receipt for another session/epoch cannot poison this ledger.
    pub fn record_seal_coverage(&mut self, receipt: SealCoverageReceipt) -> bool {
        let binds_this_ledger = self.committed.keys().any(|occurrence| {
            occurrence.session == receipt.session_id
                && occurrence.capture_epoch == receipt.capture_epoch
        }) || self.committed.is_empty();
        if !binds_this_ledger
            || receipt.uncovered_speech_ranges.iter().any(|range| {
                range.session != receipt.session_id || range.capture_epoch != receipt.capture_epoch
            })
        {
            return false;
        }
        let capture = OccurrenceIdentity::new(&receipt.session_id, receipt.capture_epoch, 0, 0);
        if crate::pipeline::trail::is_enabled(&capture) {
            crate::pipeline::trail::record(
                &capture,
                crate::pipeline::trail::TrailEvent::CoverageDiagnostics {
                    diagnostics: self.coverage_diagnostics(&receipt),
                },
            );
        }
        self.latest_seal_coverage = Some(receipt);
        true
    }

    pub fn latest_seal_coverage(&self) -> Option<&SealCoverageReceipt> {
        self.latest_seal_coverage.as_ref()
    }

    /// Store the latest island accounting for this capture. Observational:
    /// nothing reads it to admit, refuse or seal. Another session or epoch
    /// cannot replace it, as with [`Self::record_seal_coverage`].
    pub fn record_speech_islands(
        &mut self,
        evidence: crate::pipeline::speech_islands::SpeechIslandEvidence,
    ) -> bool {
        let binds_this_ledger = self.committed.is_empty()
            || self.committed.keys().any(|occurrence| {
                occurrence.session == evidence.session_id
                    && occurrence.capture_epoch == evidence.capture_epoch
            });
        if !binds_this_ledger {
            return false;
        }
        self.latest_speech_islands = Some(evidence);
        true
    }

    pub fn latest_speech_islands(
        &self,
    ) -> Option<&crate::pipeline::speech_islands::SpeechIslandEvidence> {
        self.latest_speech_islands.as_ref()
    }

    /// Compare committed occurrence ranges with authenticated measured speech
    /// on the same capture clock. Ranges are unioned before subtraction, so
    /// overlap or repeated labels can neither inflate nor erase coverage.
    ///
    /// The speech input is an observer's own [`AcousticSpeechEvidence`], not a
    /// bare range vector: an empty set means silence only when the observer
    /// says it measured the extent. Missing, foreign, invalid or partial
    /// measurement leaves the take [`SealCoverageStatus::Unavailable`] instead
    /// of being filtered into a trivially complete receipt.
    pub fn assess_seal_coverage(
        &self,
        session: &str,
        capture_epoch: u64,
        speech: &AcousticSpeechEvidence,
        incomplete_threshold_samples: u64,
    ) -> SealCoverageReceipt {
        use crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER;
        use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

        fn merged_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
            ranges.retain(|(start, end)| end > start);
            ranges.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
            for (start, end) in ranges {
                if let Some((_, previous_end)) = merged.last_mut()
                    && start <= *previous_end
                {
                    *previous_end = (*previous_end).max(end);
                } else {
                    merged.push((start, end));
                }
            }
            merged
        }

        let refuse = |gap: AcousticEvidenceGap| SealCoverageReceipt {
            session_id: session.to_string(),
            capture_epoch,
            sample_rate_hz: self.capture_rate_hz,
            speech_samples: 0,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 0,
            incomplete_threshold_samples,
            status: SealCoverageStatus::Unavailable(gap),
            speech_producer: speech.producer().to_string(),
            availability: speech.availability().as_str().to_string(),
            observed_samples: None,
        };

        // The evidence must name this take. A caller cannot authenticate a
        // foreign observer by asking about its own session.
        if !speech.identity().matches(session, capture_epoch) {
            return refuse(AcousticEvidenceGap::IdentityMismatch);
        }
        let Some(observed_samples) = speech.availability().observed_samples() else {
            return refuse(match speech.availability() {
                AcousticAvailability::IdentityMismatch => AcousticEvidenceGap::IdentityMismatch,
                AcousticAvailability::InvalidMeasurement { .. } => {
                    AcousticEvidenceGap::InvalidMeasurement
                }
                AcousticAvailability::Discontinuous { .. } => {
                    AcousticEvidenceGap::PartialObservation
                }
                // `observed_samples()` already excluded the observed arm.
                AcousticAvailability::NotObserved | AcousticAvailability::Observed { .. } => {
                    AcousticEvidenceGap::NotObserved
                }
            });
        };
        // A foreign range inside otherwise-authenticated evidence is a mismatch
        // that must survive adjudication. Filtering it away is what turned an
        // all-foreign input into an empty successful receipt.
        if speech
            .ranges()
            .iter()
            .any(|range| range.session != session || range.capture_epoch != capture_epoch)
        {
            return refuse(AcousticEvidenceGap::IdentityMismatch);
        }

        // Only acoustic observers can certify speech or silence. Invalid
        // ranges invalidate the measurement before union can discard them.
        if !matches!(
            speech.producer(),
            CAPTURE_ENERGY_PRODUCER | SILERO_BOUNDARIES_PRODUCER
        ) || speech
            .ranges()
            .iter()
            .any(|range| range.sample_end <= range.sample_start)
        {
            return refuse(AcousticEvidenceGap::InvalidMeasurement);
        }

        let producer = speech.producer();
        let availability = speech.availability();
        let speech = merged_ranges(
            speech
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect(),
        );
        let committed = merged_ranges(
            self.committed
                .keys()
                .filter(|occurrence| {
                    occurrence.session == session && occurrence.capture_epoch == capture_epoch
                })
                .map(|occurrence| (occurrence.sample_start, occurrence.sample_end))
                .collect(),
        );

        // Committed speech, or a measured span, past the observed extent means
        // the remainder was never heard by this observer. An unobserved tail is
        // not silence and may not be certified as covered.
        if speech
            .iter()
            .chain(committed.iter())
            .any(|(_, end)| *end > observed_samples)
        {
            return refuse(AcousticEvidenceGap::PartialObservation);
        }

        // A debt label stays visible and cannot certify the PCM it names.
        // Subtract every pending recovery, committed or not. An uncommitted
        // debt range still punches out of a wider neighbour: another label
        // must not make unresolved speech disappear.
        let debt = self.pending_text_recoveries(session, capture_epoch);
        let committed: Vec<(u64, u64)> = committed
            .into_iter()
            .flat_map(|range| {
                let mut pieces = vec![range];
                for occurrence in &debt {
                    pieces = pieces
                        .into_iter()
                        .flat_map(|(start, end)| {
                            if occurrence.sample_end <= start || occurrence.sample_start >= end {
                                return vec![(start, end)];
                            }
                            let mut retained = Vec::with_capacity(2);
                            if start < occurrence.sample_start {
                                retained.push((start, occurrence.sample_start));
                            }
                            if end > occurrence.sample_end {
                                retained.push((occurrence.sample_end, end));
                            }
                            retained
                        })
                        .collect();
                }
                pieces
            })
            .collect();

        let speech_samples = speech
            .iter()
            .map(|(start, end)| end.saturating_sub(*start))
            .sum::<u64>();
        let mut uncovered = Vec::new();
        for (speech_start, speech_end) in speech {
            let mut cursor = speech_start;
            for (covered_start, covered_end) in &committed {
                if *covered_end <= cursor || *covered_start >= speech_end {
                    continue;
                }
                if *covered_start > cursor {
                    uncovered.push(TailSampleRange {
                        session: session.to_string(),
                        capture_epoch,
                        sample_start: cursor,
                        sample_end: (*covered_start).min(speech_end),
                    });
                }
                cursor = cursor.max(*covered_end).min(speech_end);
                if cursor >= speech_end {
                    break;
                }
            }
            if cursor < speech_end {
                uncovered.push(TailSampleRange {
                    session: session.to_string(),
                    capture_epoch,
                    sample_start: cursor,
                    sample_end: speech_end,
                });
            }
        }
        let uncovered_samples = uncovered
            .iter()
            .map(|range| range.sample_end.saturating_sub(range.sample_start))
            .sum::<u64>();
        let max_uncovered_samples = uncovered
            .iter()
            .map(|range| range.sample_end.saturating_sub(range.sample_start))
            .max()
            .unwrap_or(0);
        let status = if !debt.is_empty() || max_uncovered_samples > incomplete_threshold_samples {
            SealCoverageStatus::Incomplete
        } else {
            SealCoverageStatus::Complete
        };
        SealCoverageReceipt {
            session_id: session.to_string(),
            capture_epoch,
            sample_rate_hz: self.capture_rate_hz,
            speech_samples,
            covered_samples: speech_samples.saturating_sub(uncovered_samples),
            uncovered_speech_ranges: uncovered,
            max_uncovered_samples,
            incomplete_threshold_samples,
            status,
            speech_producer: producer.to_string(),
            availability: availability.as_str().to_string(),
            observed_samples: Some(observed_samples),
        }
    }

    // -- per-layer decision history ----------------------------------------

    /// The complete decision trail, in arrival order.
    pub fn layer_trail(&self) -> &[LayerDecisionReceipt] {
        &self.trail
    }

    /// The decision trail for one occurrence, in arrival order.
    pub fn layer_trail_for<'a>(
        &'a self,
        occurrence: &'a OccurrenceIdentity,
    ) -> impl Iterator<Item = &'a LayerDecisionReceipt> {
        self.owner_history
            .get(occurrence)
            .into_iter()
            .flat_map(|owner| owner.trail_ordinals.iter())
            .map(|ordinal| &self.trail[*ordinal])
    }

    /// Decisions taken on an occurrence after it was sealed.
    ///
    /// Every one of these is either an automatic attempt the seal refused or a
    /// human supersession with a [`ManualEditReceipt`] beside it.
    pub fn post_seal_decisions<'a>(
        &'a self,
        occurrence: &'a OccurrenceIdentity,
    ) -> Vec<&'a LayerDecisionReceipt> {
        let Some(seal) = self.seals.get(occurrence) else {
            return Vec::new();
        };
        self.layer_trail_for(occurrence)
            .filter(|entry| !seal.layer_trail_ordinals.contains(&entry.ordinal))
            .collect()
    }

    /// Every explicit human supersession, in arrival order.
    pub fn manual_edits(&self) -> &[ManualEditReceipt] {
        &self.manual_edits
    }

    /// Called by the retained presentation's physical capture-open callback.
    /// The private capability cannot be constructed from a UI session string.
    pub fn bind_manual_document_capture(
        &mut self,
        session_id: &str,
        capture_epoch: u64,
    ) -> Result<ManualDocumentCaptureReceipt, &'static str> {
        if session_id.is_empty() || capture_epoch == 0 {
            return Err("manual_capture_identity_missing");
        }
        if let Some(owner) = &self.manual_document_capture {
            return if owner.session_id == session_id && owner.capture_epoch == capture_epoch {
                Ok(owner.clone())
            } else {
                Err("manual_capture_owner_mismatch")
            };
        }
        if self
            .evidence
            .keys()
            .chain(self.committed.keys())
            .chain(self.frontiers.keys())
            .chain(self.pending_text_recovery.iter())
            .any(|occurrence| {
                occurrence.session != session_id || occurrence.capture_epoch != capture_epoch
            })
            || self.latest_seal_coverage.as_ref().is_some_and(|coverage| {
                coverage.session_id != session_id || coverage.capture_epoch != capture_epoch
            })
        {
            return Err("manual_capture_owner_mismatch");
        }
        let owner = ManualDocumentCaptureReceipt {
            receipt_id: format!("manual-capture-{}", uuid::Uuid::new_v4()),
            session_id: session_id.to_owned(),
            capture_epoch,
            source_revision: None,
        };
        self.manual_document_capture = Some(owner.clone());
        Ok(owner)
    }

    /// The emitter calls this only after its same-session Bus lifecycle ended.
    /// Neither a terminal reducer Boolean nor deserialized text is a capability.
    pub fn complete_manual_document_capture(
        &mut self,
        owner: &ManualDocumentCaptureReceipt,
        source_revision: u64,
    ) -> Result<ManualDocumentCaptureReceipt, &'static str> {
        let Some(bound) = self.manual_document_capture.as_mut() else {
            return Err("manual_capture_not_opened");
        };
        if bound != owner || bound.source_revision.is_some() {
            return Err("manual_capture_completion_replayed");
        }
        bound.source_revision = Some(source_revision);
        Ok(bound.clone())
    }

    pub fn authenticates_manual_document_capture(
        &self,
        owner: &ManualDocumentCaptureReceipt,
    ) -> bool {
        self.manual_document_capture.as_ref() == Some(owner)
    }

    pub fn has_manual_document_capture(&self, session_id: &str, capture_epoch: u64) -> bool {
        self.manual_document_capture
            .as_ref()
            .is_some_and(|owner| owner.matches(session_id, capture_epoch))
            && self
                .evidence
                .keys()
                .chain(self.frontiers.keys())
                .chain(self.pending_text_recovery.iter())
                .all(|occurrence| {
                    occurrence.session == session_id && occurrence.capture_epoch == capture_epoch
                })
            && self.latest_seal_coverage.as_ref().is_none_or(|coverage| {
                coverage.session_id == session_id && coverage.capture_epoch == capture_epoch
            })
    }

    /// Authenticate stored document provenance, including the capture-only form.
    pub fn authenticates_manual_document_revision(
        &self,
        receipt: &ManualDocumentRevisionReceipt,
    ) -> bool {
        if !self.manual_document_revisions.contains(receipt) {
            return false;
        }
        if !receipt.source_occurrences.is_empty() {
            return receipt.capture_receipt_id.is_none() && receipt.capture_epoch.is_none();
        }
        self.manual_document_capture.as_ref().is_some_and(|owner| {
            owner.source_revision.is_some()
                && owner.session_id == receipt.session_id
                && Some(owner.capture_epoch) == receipt.capture_epoch
                && Some(&owner.receipt_id) == receipt.capture_receipt_id.as_ref()
                && receipt.source_seal_receipts.is_empty()
                && matches!(
                    receipt.provenance.as_str(),
                    "user-edit" | "retranscribe" | "navigation"
                )
        })
    }

    /// Authenticate one provenance-bearing rewrite of the complete document.
    ///
    /// A whole-document edit cannot honestly be divided back into occurrence
    /// labels without a new word-to-PCM alignment pass. This receipt therefore
    /// binds the edited bytes to the exact committed occurrence set and source
    /// reducer revision, while leaving every acoustic occurrence and token
    /// receipt immutable. The transcript reducer remains the document author.
    pub fn record_manual_document_revision(
        &mut self,
        session_id: &str,
        source_revision: u64,
        revision: u64,
        rendered_text: &str,
        source_occurrences: &[OccurrenceIdentity],
        provenance: DocumentRevisionProvenance,
    ) -> Result<ManualDocumentRevisionReceipt, &'static str> {
        if session_id.is_empty() {
            return Err("manual_document_session_missing");
        }
        if rendered_text.trim().is_empty() {
            return Err("manual_document_text_empty");
        }
        if source_revision.checked_add(1) != Some(revision) {
            return Err("manual_document_revision_nonconsecutive");
        }
        let capture = if source_occurrences.is_empty() {
            if !matches!(
                provenance,
                DocumentRevisionProvenance::UserEdit
                    | DocumentRevisionProvenance::Retranscribe
                    | DocumentRevisionProvenance::Navigation
            ) {
                return Err("manual_document_explicit_intent_required");
            }
            let owner = self
                .manual_document_capture
                .as_ref()
                .filter(|owner| owner.session_id == session_id && owner.source_revision.is_some())
                .ok_or("manual_document_completed_capture_missing")?;
            if !self.has_manual_document_capture(session_id, owner.capture_epoch) {
                return Err("manual_document_capture_epoch_mismatch");
            }
            if !self.committed.is_empty() && self.manual_document_revisions.is_empty() {
                return Err("manual_document_committed_source_missing");
            }
            let expected = self
                .manual_document_revisions
                .last()
                .map_or(owner.source_revision, |previous| Some(previous.revision));
            if expected != Some(source_revision) {
                return Err("manual_document_source_revision_stale");
            }
            Some(owner)
        } else {
            None
        };
        if source_occurrences.iter().any(|occurrence| {
            occurrence.session != session_id
                || !self.is_qualified(occurrence)
                || !self.committed.contains_key(occurrence)
        }) {
            return Err("manual_document_occurrence_not_committed");
        }

        let source_seal_receipts = source_occurrences
            .iter()
            .filter_map(|occurrence| self.seal_of(occurrence))
            .map(|seal| seal.receipt_id.clone())
            .collect::<Vec<_>>();
        let ordinal = self.manual_document_revisions.len();
        let receipt = ManualDocumentRevisionReceipt {
            receipt_id: format!(
                "{}-{session_id}-{source_revision}-{revision}-{ordinal}",
                provenance.as_str()
            ),
            provenance: provenance.as_str().to_string(),
            session_id: session_id.to_string(),
            source_revision,
            revision,
            source_occurrences: source_occurrences.to_vec(),
            source_seal_receipts,
            rendered_text: rendered_text.to_string(),
            capture_epoch: capture.map(|owner| owner.capture_epoch),
            capture_receipt_id: capture.map(|owner| owner.receipt_id.clone()),
        };
        self.manual_document_revisions.push(receipt.clone());
        Ok(receipt)
    }

    /// Every authenticated whole-document user revision, in arrival order.
    pub fn manual_document_revisions(&self) -> &[ManualDocumentRevisionReceipt] {
        &self.manual_document_revisions
    }

    /// Authenticate an Agent answer's presentation source without rewriting
    /// acoustic labels. The caller still owes coverage and execution/history
    /// admission; this receipt proves only the exact sealed group and bytes.
    pub fn record_consultation_presentation(
        &mut self,
        input: ConsultationPresentationInput<'_>,
    ) -> Result<ConsultationPresentationReceipt, &'static str> {
        if input.consultation_id.trim().is_empty() || input.turn_id.trim().is_empty() {
            return Err("consultation_presentation_identity_missing");
        }
        if input.rendered_text.trim().is_empty() {
            return Err("consultation_presentation_text_empty");
        }
        if input.source_revision.checked_add(1) != Some(input.revision) {
            return Err("consultation_presentation_revision_nonconsecutive");
        }
        let Some(first) = input.members.first() else {
            return Err("consultation_presentation_members_missing");
        };
        let session = &first.occurrence.session;
        let epoch = first.occurrence.capture_epoch;
        if session.is_empty() || epoch == 0 {
            return Err("consultation_presentation_capture_missing");
        }
        let mut end = first.occurrence.sample_start;
        for member in input.members {
            let occurrence = &member.occurrence;
            if &occurrence.session != session
                || occurrence.capture_epoch != epoch
                || occurrence.sample_start < end
                || occurrence.sample_start >= occurrence.sample_end
            {
                return Err("consultation_presentation_member_order");
            }
            if !self.is_qualified(occurrence)
                || self.text_recovery_pending(occurrence)
                || !self
                    .frontier_of(occurrence)
                    .is_some_and(|frontier| frontier.is_closed())
                || self.text_of(occurrence) != Some(member.source_label.as_str())
                || member.source_label.trim().is_empty()
                || self
                    .seal_of(occurrence)
                    .is_none_or(|seal| seal.receipt_id != member.seal_receipt)
            {
                return Err("consultation_presentation_source_changed");
            }
            end = occurrence.sample_end;
        }
        let known = self
            .qualified_occurrences()
            .chain(self.occurrences())
            .filter(|occurrence| {
                &occurrence.session == session
                    && occurrence.capture_epoch == epoch
                    && occurrence.sample_start < end
                    && first.occurrence.sample_start < occurrence.sample_end
            })
            .collect::<BTreeSet<_>>();
        if known.len() != input.members.len()
            || !known
                .into_iter()
                .eq(input.members.iter().map(|member| &member.occurrence))
        {
            return Err("consultation_presentation_members_incomplete");
        }
        if self.consultation_presentations.iter().any(|receipt| {
            receipt.consultation_id == input.consultation_id && receipt.turn_id == input.turn_id
        }) {
            return Err("consultation_presentation_turn_repeated");
        }
        if self.consultation_presentations.iter().any(|receipt| {
            receipt.members.iter().any(|old| {
                input
                    .members
                    .iter()
                    .any(|member| old.occurrence == member.occurrence)
            })
        }) {
            return Err("consultation_presentation_group_overlaps");
        }
        let receipt = ConsultationPresentationReceipt {
            receipt_id: format!(
                "max-group-{session}-{epoch}-{}",
                self.consultation_presentations.len()
            ),
            consultation_id: input.consultation_id.to_string(),
            turn_id: input.turn_id.to_string(),
            source_revision: input.source_revision,
            revision: input.revision,
            members: input.members.to_vec(),
            rendered_text: input.rendered_text.to_string(),
        };
        self.consultation_presentations.push(receipt.clone());
        Ok(receipt)
    }

    pub fn consultation_presentations(&self) -> &[ConsultationPresentationReceipt] {
        &self.consultation_presentations
    }

    /// Authenticate one presentation shaping of a single sealed occurrence.
    ///
    /// This is deliberately *not* a whole-document revision. A live Light+ pass
    /// during capture states something much smaller and much more checkable:
    /// "the closed occurrence X, whose committed label is exactly
    /// `source_label`, is presented as `shaped_text`". The ledger keeps the
    /// label itself immutable — `text_of` still returns the spoken words — so
    /// the shaping can never be mistaken for a human correction of the
    /// transcript, and a stale shape is detectable by comparing `source_label`
    /// against the label the ledger currently holds.
    ///
    /// `left_context_sha256` pins the neighbouring committed text the casing
    /// decision was taken against, so a later audit can reproduce the shape
    /// instead of trusting it.
    ///
    /// Refuses an unqualified, unsealed, uncommitted, foreign-session or
    /// relabelled occurrence, and refuses to restate a shaping it already
    /// holds — a repeated seal observation mints no second receipt.
    ///
    /// The claim arrives as one borrowed [`IncrementalShapingInput`] so its
    /// seven facts stay named at every callsite. Grouping them moves nothing:
    /// the ledger still owns every check below, in this order, and remains the
    /// only place a receipt is minted.
    pub fn record_incremental_shaping(
        &mut self,
        input: IncrementalShapingInput<'_>,
    ) -> Result<IncrementalShapingReceipt, &'static str> {
        let IncrementalShapingInput {
            session_id,
            source_revision,
            revision,
            occurrence,
            source_label,
            left_context,
            shaped_text,
            sentence_break_before,
        } = input;
        if session_id.is_empty() {
            return Err("incremental_shaping_session_missing");
        }
        if shaped_text.trim().is_empty() {
            return Err("incremental_shaping_text_empty");
        }
        if source_revision.checked_add(1) != Some(revision) {
            return Err("incremental_shaping_revision_nonconsecutive");
        }
        if occurrence.session != session_id {
            return Err("incremental_shaping_session_mismatch");
        }
        if !self.is_qualified(occurrence) || !self.committed.contains_key(occurrence) {
            return Err("incremental_shaping_occurrence_not_committed");
        }
        if self.text_of(occurrence) != Some(source_label) {
            return Err("incremental_shaping_source_label_stale");
        }
        let source_seal_receipt = self.seal_of(occurrence).map(|seal| seal.receipt_id.clone());
        if source_label == shaped_text {
            return Err("incremental_shaping_unchanged");
        }
        if crate::pipeline::light_plus::apply_live_span(
            left_context,
            source_label,
            sentence_break_before,
        ) != shaped_text
        {
            return Err("incremental_shaping_not_deterministic");
        }
        if self
            .latest_incremental_shaping
            .get(occurrence)
            .map(|index| &self.incremental_shapings[*index])
            .is_some_and(|held| {
                held.source_label == source_label
                    && held.shaped_text == shaped_text
                    && held.left_context == left_context
            })
        {
            return Err("incremental_shaping_unchanged");
        }

        let ordinal = self.incremental_shapings.len();
        let receipt = IncrementalShapingReceipt {
            receipt_id: format!(
                "{}-incremental-{session_id}-{}-{}-{source_revision}-{revision}-{ordinal}",
                DocumentRevisionProvenance::LightPlus.as_str(),
                occurrence.sample_start,
                occurrence.sample_end,
            ),
            provenance: DocumentRevisionProvenance::LightPlus.as_str().to_string(),
            session_id: session_id.to_string(),
            source_revision,
            revision,
            occurrence: occurrence.clone(),
            source_seal_receipt,
            sentence_break_before,
            source_label: source_label.to_string(),
            left_context: left_context.to_string(),
            left_context_sha256: hex::encode(Sha256::digest(left_context.as_bytes())),
            shaped_text: shaped_text.to_string(),
        };
        self.latest_incremental_shaping
            .insert(occurrence.clone(), ordinal);
        self.incremental_shapings.push(receipt.clone());
        Ok(receipt)
    }

    /// Every authenticated per-occurrence shaping, in arrival order.
    pub fn incremental_shapings(&self) -> &[IncrementalShapingReceipt] {
        &self.incremental_shapings
    }

    /// Authenticate the exact stored receipt without comparing every prior
    /// owner's strings. The receipt's trailing ordinal is a lookup hint only;
    /// equality with the ledger-owned record remains the authority.
    pub fn authenticates_incremental_shaping(&self, receipt: &IncrementalShapingReceipt) -> bool {
        receipt
            .receipt_id
            .rsplit('-')
            .next()
            .and_then(|ordinal| ordinal.parse::<usize>().ok())
            .and_then(|ordinal| self.incremental_shapings.get(ordinal))
            == Some(receipt)
    }

    /// Record the decision the ledger just took. Called for every observation
    /// without exception, so a token can never exist without its layer history.
    pub(super) fn record_layer_decision(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        decision: &MutationReceipt,
        clock_lie_blocker: Option<OccurrenceIdentity>,
    ) {
        let ordinal = self.trail.len();
        let predecessor_ordinal = self
            .owner_history
            .get(&observation.occurrence)
            .and_then(|owner| owner.trail_ordinals.last())
            .copied();
        let serials: Vec<AcousticSerial> = self
            .evidence
            .get(&observation.occurrence)
            .cloned()
            .into_iter()
            .collect();
        let clock_lie_blocker = clock_lie_blocker.or_else(|| {
            if matches!(
                decision,
                MutationReceipt::Refuse {
                    reason: RefuseReason::ClockLie,
                    ..
                }
            ) {
                self.replace_neighbour(
                    &observation.occurrence,
                    self.slots_of(&observation.occurrence).unwrap_or(&[]),
                )
            } else {
                None
            }
        });
        let clock_lie = self.note_clock_lie(observation, decision);
        self.trail.push(LayerDecisionReceipt {
            ordinal,
            receipt_id: format!(
                "{}-{}-{}-{}",
                observation.producer.layer_label(),
                observation.request,
                observation.generation,
                ordinal
            ),
            observation: observation.clone(),
            candidate_label: text.to_string(),
            candidate_tokens: text.split_whitespace().map(str::to_string).collect(),
            serials,
            decision: decision.clone(),
            predecessor_ordinal,
            clock_lie,
            clock_lie_blocker,
        });
        self.owner_history
            .entry(observation.occurrence.clone())
            .or_default()
            .trail_ordinals
            .push(ordinal);
        let trail_input = self.trail_input.take();
        if let Some(entry) = self.trail.last() {
            crate::pipeline::trail::decision(self, entry, trail_input);
        }
        self.issued_receipts += 1;
        let reason = match decision {
            MutationReceipt::Refuse { reason, .. } => Some(reason.as_str()),
            MutationReceipt::KeepVisibleUnanchored { reason, .. } => Some(reason.as_str()),
            MutationReceipt::Preserve { .. }
            | MutationReceipt::Correct { .. }
            | MutationReceipt::Insert { .. } => {
                self.delivered_observations += 1;
                None
            }
        };
        if let Some(reason) = reason {
            *self.named_refusals.entry(reason).or_default() += 1;
        }
    }
}
