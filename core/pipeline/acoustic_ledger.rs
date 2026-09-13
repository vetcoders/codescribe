//! Acoustic occurrence identity, observation identity, and mutation receipts.
//!
//! The conservation law THE ENGINE is built around is a statement about
//! *physical events*, not about strings:
//!
//! > If the audio holds five distinct acoustic occurrences of a name, the
//! > transcript holds exactly five tokens for it, in the same order.
//!
//! Text cannot enforce that. Two occurrences of "Iwo" are byte-identical, so
//! every content-keyed mechanism — longest-common-subsequence, novelty
//! filtering, suffix-overlap stripping, edit-tolerant prefix matching — reads
//! the second one as a restatement of the first and deletes it. The deletion is
//! invisible in the output precisely because the two strings are the same.
//!
//! This module supplies the only key that can tell them apart, and separates
//! the three things that were previously fused into one "span":
//!
//! * [`OccurrenceIdentity`] — the *physical* event. A stretch of captured PCM
//!   in one capture epoch of one session. Nothing else lives here: no text, no
//!   producer, no confidence, no ordering. Two observations that name the same
//!   samples are observations of the same occurrence, however far apart they
//!   arrived and whichever engine produced them.
//! * [`ObservationIdentity`] — one *hypothesis about* an occurrence: who
//!   produced it, in which request/window, and at which generation. `order`
//!   lives here and never on the occurrence, because a replay that arrives
//!   later must not be able to mint a new physical event simply by being late.
//! * [`MutationReceipt`] — the one-to-one answer the ledger owes for every
//!   observation it is offered. Conservation is auditable exactly because the
//!   receipt count equals the observation count, always.
//!
//! Around those three, the ledger holds the evidence organs that make the
//! claim auditable rather than asserted:
//!
//! * [`AcousticEvidence`] and [`EnergyCalibration`] decide, from energy and VAD
//!   alone, whether a region physically exists at all.
//! * [`AcousticSerial`] is the mandatory, versioned receipt every qualified
//!   occurrence mints, and [`WordEvidenceReceipt`] is the signature every
//!   emitted token must carry back to it.
//! * [`LayerDecisionReceipt`] keeps the whole Apple -> Whisper -> retained-text
//!   chain inspectable even after the visible label changes.
//! * [`ObservationFrontier`] and [`LedgerSealReceipt`] decide finality;
//!   [`ManualEditReceipt`] is the only supersession a sealed occurrence label
//!   accepts, while [`ManualDocumentRevisionReceipt`] authenticates an explicit
//!   user rewrite of the already-sealed document without inventing a text-to-PCM
//!   alignment.
//! * [`OccurrenceDerivation`] refines coverage as provenance, and
//!   [`OccurrenceComposition`] emits the signed token sequence a reducer reads.
//!
//! What the ledger deliberately does *not* do: infer identity from text,
//! invent sub-ranges the payload does not carry, let an unanchored
//! hypothesis suppress an anchored one, or own the document those receipts are
//! projected into.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::audio::capture_receipt::{AcousticAvailability, AcousticSpeechEvidence};
use crate::stt::tail_provider::TailSampleRange;

/// A physical acoustic occurrence: a PCM range in one capture epoch.
///
/// This is the primary key of the transcript. It carries no text and no
/// ordering on purpose — see the module docs for why `order` is not here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrenceIdentity {
    /// Capture session the samples belong to.
    pub session: String,
    /// Capture epoch; a device restart or route change opens a new one and the
    /// sample clock restarts with it.
    pub capture_epoch: u64,
    /// First sample of the occurrence on the capture clock.
    pub sample_start: u64,
    /// One past the last sample of the occurrence.
    pub sample_end: u64,
}

impl OccurrenceIdentity {
    /// Build an occurrence from raw capture coordinates.
    pub fn new(
        session: impl Into<String>,
        capture_epoch: u64,
        sample_start: u64,
        sample_end: u64,
    ) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
            sample_start,
            sample_end,
        }
    }

    /// Length in samples; saturating, because a reversed range is not evidence
    /// of negative audio, it is evidence of a broken producer.
    pub fn sample_len(&self) -> u64 {
        self.sample_end.saturating_sub(self.sample_start)
    }

    /// Whether the range names real audio.
    ///
    /// A zero-width or reversed range names nothing. It is not a small
    /// occurrence — it is the *absence* of an anchor, and it therefore carries
    /// no authority over anything.
    pub fn is_anchored(&self) -> bool {
        self.sample_end > self.sample_start
    }

    /// Whether two occurrences live on the same sample clock at all.
    ///
    /// Sample numbers from different epochs are not comparable: epoch 1's
    /// sample 48000 and epoch 2's sample 48000 are different moments in the
    /// world. Comparing them is how an epoch rollover silently deletes speech.
    pub fn same_capture(&self, other: &Self) -> bool {
        self.session == other.session && self.capture_epoch == other.capture_epoch
    }

    /// Relation of `self` to an already-committed occurrence.
    pub fn relate(&self, committed: &Self) -> OccurrenceRelation {
        if !self.same_capture(committed) {
            return OccurrenceRelation::DifferentCapture;
        }
        if !self.is_anchored() || !committed.is_anchored() {
            return OccurrenceRelation::Unanchored;
        }
        if self.sample_start == committed.sample_start && self.sample_end == committed.sample_end {
            return OccurrenceRelation::Same;
        }
        let overlap_start = self.sample_start.max(committed.sample_start);
        let overlap_end = self.sample_end.min(committed.sample_end);
        if overlap_end > overlap_start {
            OccurrenceRelation::Overlapping {
                overlap_samples: overlap_end - overlap_start,
            }
        } else {
            OccurrenceRelation::Disjoint
        }
    }
}

impl From<&TailSampleRange> for OccurrenceIdentity {
    fn from(range: &TailSampleRange) -> Self {
        Self {
            session: range.session.clone(),
            capture_epoch: range.capture_epoch,
            sample_start: range.sample_start,
            sample_end: range.sample_end,
        }
    }
}

/// How an incoming occurrence stands to one already on the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceRelation {
    /// Byte-for-byte the same samples: one physical event, two observations.
    Same,
    /// Shares audio but not boundaries. Without word-level pins there is no way
    /// to say which tokens belong to the shared part, so nothing may be clipped.
    Overlapping {
        /// Samples the two ranges have in common.
        overlap_samples: u64,
    },
    /// Same clock, no shared audio: two distinct physical events.
    Disjoint,
    /// One side names no audio. Carries no authority in either direction.
    Unanchored,
    /// Different session or capture epoch: not comparable, not evidence.
    DifferentCapture,
}

/// Engine family that produced an observation.
///
/// The ordering is a *text authority* ordering, not a quality ranking: a later
/// layer is allowed to rewrite the text of a span an earlier layer committed,
/// on the same range, without changing how many occurrences exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationProducer {
    /// L0 — Apple Speech live lane.
    Apple,
    /// L1 — Whisper tail retranscription.
    Whisper,
    /// L2 — lexicon and Light+ cleanup.
    Lexicon,
    /// L3 — Responses formatter.
    Formatter,
    /// Human evidence. Fixes spelling for matching spans and is never
    /// overridden by a model prior.
    ManualHuman,
}

impl ObservationProducer {
    /// Text authority rank; higher may correct lower on the *same* occurrence.
    pub fn authority_rank(self) -> u8 {
        match self {
            Self::Apple => 0,
            Self::Whisper => 1,
            Self::Lexicon => 2,
            Self::Formatter => 3,
            Self::ManualHuman => 4,
        }
    }

    /// Layer label used by the decision trail and the runtime evidence trace.
    ///
    /// Lexicon and Formatter are two producers inside one *layer* — the
    /// retained lexical/text author. The trail keeps both producers apart; the
    /// trace groups them under the layer the falsification contract names.
    pub fn layer_label(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Whisper => "whisper",
            Self::Lexicon | Self::Formatter => "retained_text",
            Self::ManualHuman => "manual_human",
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Whisper => "whisper",
            Self::Lexicon => "lexicon",
            Self::Formatter => "formatter",
            Self::ManualHuman => "manual_human",
        }
    }
}

/// One hypothesis about one occurrence.
///
/// `generation` is the ordering axis, and it belongs here rather than on
/// [`OccurrenceIdentity`]: arriving later makes an observation newer, not
/// physically distinct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObservationIdentity {
    /// Engine family that produced the hypothesis.
    pub producer: ObservationProducer,
    /// Request or window that carried it.
    pub request: u64,
    /// Monotonic hypothesis counter within the producer's lane.
    pub generation: u64,
    /// The physical event being described.
    pub occurrence: OccurrenceIdentity,
}

impl ObservationIdentity {
    /// Build an observation identity.
    pub fn new(
        producer: ObservationProducer,
        request: u64,
        generation: u64,
        occurrence: OccurrenceIdentity,
    ) -> Self {
        Self {
            producer,
            request,
            generation,
            occurrence,
        }
    }
}

/// Why an observation was admitted without any right to mutate the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoAuthorityReason {
    /// The range names no audio (zero-width or reversed).
    ZeroWidth,
    /// The producer supplied no range at all.
    NoRange,
    /// The range shares audio with a committed occurrence but the payload
    /// carries no word pins, so no token can be attributed to the shared part.
    OverlapWithoutWordPins,
}

impl NoAuthorityReason {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZeroWidth => "zero_width",
            Self::NoRange => "no_range",
            Self::OverlapWithoutWordPins => "overlap_without_word_pins",
        }
    }
}

/// Why an observation was refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefuseReason {
    /// The occurrence is closed to this hypothesis. Either it is already held
    /// by an equal-or-higher authority at an equal-or-newer generation and this
    /// hypothesis disagrees with it, or it carries a [`LedgerSealReceipt`] and
    /// the hypothesis came from an automatic producer.
    SealedReplay,
    /// The exact same observation identity was already answered in this batch.
    BatchDuplicate,
    /// A machine observation returned no lexical evidence.
    EmptyLabel,
}

impl RefuseReason {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SealedReplay => "sealed_replay",
            Self::BatchDuplicate => "batch_duplicate",
            Self::EmptyLabel => "empty_label",
        }
    }
}

/// The one-to-one answer the ledger owes for every observation offered to it.
///
/// Exactly one receipt is produced per observation, in input order. That is
/// what makes conservation auditable rather than asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationReceipt {
    /// The occurrence is already committed and this hypothesis does not have
    /// the authority — or the need — to change it. Text stands unchanged.
    Preserve {
        /// The physical event.
        occurrence: OccurrenceIdentity,
        /// Producer currently holding the text.
        held_by: ObservationProducer,
    },
    /// Same occurrence, higher authority, different text. The committed text is
    /// replaced *in place*: one occurrence in, one occurrence out.
    Correct {
        /// The physical event.
        occurrence: OccurrenceIdentity,
        /// Producer that held the previous text.
        from: ObservationProducer,
        /// Producer supplying the new text.
        to: ObservationProducer,
    },
    /// A physical event nobody has committed yet. New text enters the canvas.
    Insert {
        /// The physical event.
        occurrence: OccurrenceIdentity,
    },
    /// No usable acoustic anchor. The text is delivered so the operator sees
    /// what was said, but it may not overwrite, clip, or delete any anchored
    /// occurrence, and it does not enter the ledger.
    KeepVisibleUnanchored {
        /// Why authority is absent.
        reason: NoAuthorityReason,
    },
    /// The observation may not touch the transcript at all.
    Refuse {
        /// The physical event it claimed.
        occurrence: OccurrenceIdentity,
        /// Why it was refused.
        reason: RefuseReason,
    },
}

impl MutationReceipt {
    /// Whether the receipt puts text on the canvas for the first time.
    pub fn is_insert(&self) -> bool {
        matches!(self, Self::Insert { .. })
    }

    /// Whether the receipt rewrites text already on the canvas.
    pub fn is_correct(&self) -> bool {
        matches!(self, Self::Correct { .. })
    }

    /// Whether the receipt grants any right to change the canvas.
    pub fn grants_mutation(&self) -> bool {
        matches!(self, Self::Insert { .. } | Self::Correct { .. })
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preserve { .. } => "preserve",
            Self::Correct { .. } => "correct",
            Self::Insert { .. } => "insert",
            Self::KeepVisibleUnanchored { .. } => "keep_visible_unanchored",
            Self::Refuse { .. } => "refuse",
        }
    }
}

/// What the ledger remembers about a committed occurrence.
#[derive(Debug, Clone)]
struct CommittedObservation {
    producer: ObservationProducer,
    request: u64,
    generation: u64,
    text: String,
}

/// Conservation accounting over one admission batch.
///
/// Counted over *physical occurrences*, never over hypotheses: a cumulative
/// engine that restates the same five occurrences twelve times still yields
/// five.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConservationTally {
    /// Observations offered.
    pub observations_in: usize,
    /// Receipts issued. Always equal to `observations_in`.
    pub receipts_out: usize,
    /// Distinct physical occurrences the ledger now holds.
    pub occurrences_held: usize,
    /// Observations admitted without mutation authority.
    pub kept_visible_unanchored: usize,
}

/// Ledger of committed acoustic occurrences.
///
/// Text is never a key here. The only key is [`OccurrenceIdentity`].
#[derive(Debug, Clone, Default)]
pub struct AcousticLedger {
    committed: BTreeMap<OccurrenceIdentity, CommittedObservation>,
    answered: Vec<ObservationIdentity>,
    kept_visible: usize,
    evidence: BTreeMap<OccurrenceIdentity, AcousticSerial>,
    frontiers: BTreeMap<OccurrenceIdentity, ObservationFrontier>,
    seals: BTreeMap<OccurrenceIdentity, LedgerSealReceipt>,
    terminal_seals: Vec<LedgerSealReceipt>,
    trail: Vec<LayerDecisionReceipt>,
    manual_edits: Vec<ManualEditReceipt>,
    manual_document_revisions: Vec<ManualDocumentRevisionReceipt>,
    incremental_shapings: Vec<IncrementalShapingReceipt>,
    derivations: Vec<OccurrenceDerivation>,
    latest_seal_coverage: Option<SealCoverageReceipt>,
    pending_text_recovery: BTreeSet<OccurrenceIdentity>,
}

impl AcousticLedger {
    /// Empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Distinct physical occurrences currently held.
    pub fn len(&self) -> usize {
        self.committed.len()
    }

    /// Whether the ledger holds nothing.
    pub fn is_empty(&self) -> bool {
        self.committed.is_empty()
    }

    /// Committed text for one occurrence, if held.
    pub fn text_of(&self, occurrence: &OccurrenceIdentity) -> Option<&str> {
        self.committed
            .get(occurrence)
            .map(|held| held.text.as_str())
    }

    /// Occurrences held, in capture order.
    pub fn occurrences(&self) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.committed.keys()
    }

    /// Render the current committed labels in capture order. This is a
    /// comparison input only; callers cannot feed it back as a mutation.
    pub fn rendered_text(&self) -> String {
        self.committed
            .values()
            .map(|held| held.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

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
        self.latest_seal_coverage = Some(receipt);
        true
    }

    pub fn latest_seal_coverage(&self) -> Option<&SealCoverageReceipt> {
        self.latest_seal_coverage.as_ref()
    }

    /// Mark an acoustically qualified occurrence whose provisional label does
    /// not account for its speech. The label stays visible, but cannot certify
    /// coverage or finality until an authorized recovery observation lands.
    pub fn require_text_recovery(&mut self, occurrence: &OccurrenceIdentity) -> bool {
        if !self.evidence.contains_key(occurrence) || self.seals.contains_key(occurrence) {
            return false;
        }
        self.pending_text_recovery.insert(occurrence.clone());
        true
    }

    pub fn text_recovery_pending(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.pending_text_recovery.contains(occurrence)
    }

    pub fn pending_text_recoveries(
        &self,
        session: &str,
        capture_epoch: u64,
    ) -> Vec<OccurrenceIdentity> {
        self.pending_text_recovery
            .iter()
            .filter(|occurrence| {
                occurrence.session == session && occurrence.capture_epoch == capture_epoch
            })
            .cloned()
            .collect()
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

        // Subtract debt from the union, not individual labels: another label
        // overlapping the same PCM must not make unresolved speech disappear.
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

    /// Offer one observation and receive exactly one receipt.
    ///
    /// `text` is what the producer heard. It is recorded, compared for
    /// equality, and never used to establish identity.
    ///
    /// Every call also appends exactly one [`LayerDecisionReceipt`], so the
    /// per-layer history can never fall behind the decisions it describes.
    pub fn admit(&mut self, observation: &ObservationIdentity, text: &str) -> MutationReceipt {
        let authorized_recovery = !text.trim().is_empty()
            && matches!(
                observation.producer,
                ObservationProducer::Whisper | ObservationProducer::ManualHuman
            )
            && self
                .committed
                .get(&observation.occurrence)
                .is_none_or(|held| {
                    observation.producer.authority_rank() > held.producer.authority_rank()
                        || (observation.producer == held.producer
                            && observation.generation > held.generation)
                });
        let decision = self.decide_observation(observation, text);
        self.record_layer_decision(observation, text, &decision);
        if authorized_recovery
            && (decision.grants_mutation() || matches!(decision, MutationReceipt::Preserve { .. }))
        {
            self.pending_text_recovery.remove(&observation.occurrence);
        }
        decision
    }

    /// The admission decision itself, without the trail bookkeeping.
    fn decide_observation(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
    ) -> MutationReceipt {
        // An observation that names no audio may be shown but may not act. It
        // is NOT written to the ledger: a zero-width prior that entered the map
        // would relate as `Unanchored` to every later span and refuse all of
        // them (the D3 poisoning mode).
        if !observation.occurrence.is_anchored() {
            self.kept_visible += 1;
            self.answered.push(observation.clone());
            return MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::ZeroWidth,
            };
        }

        if self.answered.contains(observation) {
            self.answered.push(observation.clone());
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::BatchDuplicate,
            };
        }
        self.answered.push(observation.clone());

        // A sealed occurrence is finished. Its physical claim, its serial and
        // its layer history are immutable from here. Only an explicit human
        // edit may supersede the lexical label, and it leaves provenance.
        let sealed_by = self
            .seals
            .get(&observation.occurrence)
            .map(|seal| seal.receipt_id.clone());
        if let Some(supersedes_seal) = sealed_by {
            if observation.producer != ObservationProducer::ManualHuman {
                return MutationReceipt::Refuse {
                    occurrence: observation.occurrence.clone(),
                    reason: RefuseReason::SealedReplay,
                };
            }
            let held = self.committed.get(&observation.occurrence).cloned();
            let superseded_label = held
                .as_ref()
                .map(|previous| previous.text.clone())
                .unwrap_or_default();
            let from = held
                .as_ref()
                .map_or(ObservationProducer::ManualHuman, |previous| {
                    previous.producer
                });
            let manual_ordinal = self.manual_edits.len();
            self.committed.insert(
                observation.occurrence.clone(),
                CommittedObservation {
                    producer: ObservationProducer::ManualHuman,
                    request: observation.request,
                    generation: observation.generation,
                    text: text.to_string(),
                },
            );
            self.manual_edits.push(ManualEditReceipt {
                receipt_id: format!(
                    "manual-{}-{}-{manual_ordinal}",
                    observation.request, observation.generation
                ),
                occurrence: observation.occurrence.clone(),
                supersedes_seal,
                superseded_label,
                label: text.to_string(),
                observation: observation.clone(),
            });
            return if held.is_some() {
                MutationReceipt::Correct {
                    occurrence: observation.occurrence.clone(),
                    from,
                    to: ObservationProducer::ManualHuman,
                }
            } else {
                MutationReceipt::Insert {
                    occurrence: observation.occurrence.clone(),
                }
            };
        }

        if text.trim().is_empty() && observation.producer != ObservationProducer::ManualHuman {
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::EmptyLabel,
            };
        }

        if let Some(held) = self.committed.get(&observation.occurrence) {
            let held = held.clone();
            let outranks = observation.producer.authority_rank() > held.producer.authority_rank();
            let same_lane_revision =
                observation.producer == held.producer && observation.generation > held.generation;
            if held.text == text {
                return MutationReceipt::Preserve {
                    occurrence: observation.occurrence.clone(),
                    held_by: held.producer,
                };
            }
            if outranks || same_lane_revision {
                self.committed.insert(
                    observation.occurrence.clone(),
                    CommittedObservation {
                        producer: observation.producer,
                        request: observation.request,
                        generation: observation.generation,
                        text: text.to_string(),
                    },
                );
                return MutationReceipt::Correct {
                    occurrence: observation.occurrence.clone(),
                    from: held.producer,
                    to: observation.producer,
                };
            }
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::SealedReplay,
            };
        }

        // No exact match. Only same-capture, anchored priors can say anything
        // about this occurrence; different epochs and zero-width priors are
        // skipped rather than treated as objections.
        let overlaps = self.committed.keys().any(|committed| {
            matches!(
                observation.occurrence.relate(committed),
                OccurrenceRelation::Overlapping { .. }
            )
        });
        if overlaps {
            // Shares audio with something committed, but the payload carries no
            // word pins, so no token can be attributed to the shared part.
            // Clipping here would delete speech on a guess; the honest answer is
            // to show it and grant it nothing.
            self.kept_visible += 1;
            return MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::OverlapWithoutWordPins,
            };
        }

        self.committed.insert(
            observation.occurrence.clone(),
            CommittedObservation {
                producer: observation.producer,
                request: observation.request,
                generation: observation.generation,
                text: text.to_string(),
            },
        );
        MutationReceipt::Insert {
            occurrence: observation.occurrence.clone(),
        }
    }

    /// Offer a batch and receive one receipt per item, in input order.
    pub fn admit_batch(
        &mut self,
        items: &[(ObservationIdentity, String)],
    ) -> (Vec<MutationReceipt>, ConservationTally) {
        let receipts: Vec<MutationReceipt> = items
            .iter()
            .map(|(observation, text)| self.admit(observation, text))
            .collect();
        let tally = ConservationTally {
            observations_in: items.len(),
            receipts_out: receipts.len(),
            occurrences_held: self.committed.len(),
            kept_visible_unanchored: self.kept_visible,
        };
        (receipts, tally)
    }

    // -- admission: does this region physically exist? ----------------------

    /// Qualify a physically observed region and mint its mandatory serial.
    ///
    /// This is the *only* place a serial is minted, and the only gate through
    /// which a coordinate becomes evidence. Text is not an input here, in
    /// either direction: a region qualifies because of energy and VAD, or it
    /// does not qualify at all.
    pub fn qualify(
        &mut self,
        evidence: &AcousticEvidence,
        calibration: &EnergyCalibration,
    ) -> AdmissionReceipt {
        let occurrence = evidence.occurrence.clone();
        let refuse = |reason| AdmissionReceipt::Refused {
            occurrence: evidence.occurrence.clone(),
            reason,
        };
        if !occurrence.is_anchored() {
            return refuse(AdmissionRefusal::ZeroWidth);
        }
        if evidence.evidence_calibration_version != calibration.version {
            return refuse(AdmissionRefusal::CalibrationMismatch);
        }
        if evidence.energy_integral < calibration.min_energy_integral {
            return refuse(AdmissionRefusal::BelowCalibratedEnergy);
        }
        let opened = evidence
            .vad_open_sample
            .is_some_and(|open| open <= occurrence.sample_start);
        if !opened {
            return refuse(AdmissionRefusal::VadDidNotOpen);
        }
        let serial = AcousticSerial::mint(evidence);
        self.evidence.insert(occurrence.clone(), serial.clone());
        AdmissionReceipt::Qualified { occurrence, serial }
    }

    /// The serial minted for one occurrence, if it was qualified.
    pub fn serial_of(&self, occurrence: &OccurrenceIdentity) -> Option<&AcousticSerial> {
        self.evidence.get(occurrence)
    }

    /// Whether the occurrence cleared the calibrated existence predicate.
    pub fn is_qualified(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.evidence.contains_key(occurrence)
    }

    /// Qualified occurrences, in capture order.
    pub fn qualified_occurrences(&self) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.evidence.keys()
    }

    // -- observation frontier ----------------------------------------------

    /// Declare which producers the session actually scheduled for a range.
    ///
    /// Rescheduling replaces the frontier: a range whose producer set changed
    /// has a new closure question, not an amended old one.
    pub fn schedule_frontier(
        &mut self,
        coverage: OccurrenceIdentity,
        producers: impl IntoIterator<Item = ObservationProducer>,
    ) {
        let frontier = ObservationFrontier::scheduled(coverage.clone(), producers);
        self.frontiers.insert(coverage, frontier);
    }

    /// Add one observer only after its concrete job has taken ownership of the
    /// exact occurrence. Existing returns are preserved; configuration intent
    /// alone must never call this operation. Returns `false` when the base
    /// frontier is absent, already closed, already sealed, or already contains
    /// that producer (including a producer that has already returned).
    pub fn schedule_observer(
        &mut self,
        coverage: OccurrenceIdentity,
        producer: ObservationProducer,
    ) -> bool {
        if self.seals.contains_key(&coverage) {
            return false;
        }
        let Some(frontier) = self.frontiers.get_mut(&coverage) else {
            return false;
        };
        if frontier.is_closed() {
            return false;
        }
        frontier.schedule(producer)
    }

    /// Account for real Apple words arriving after all earlier jobs returned
    /// without a label. Only this exact qualified occurrence may gain its first
    /// Apple and slice-local Lexicon observations. No old return is removed and
    /// no producer may run twice; successful seals and ordinary scheduling keep
    /// their existing fences. The caller already holds the words and immediately
    /// admits Apple followed by its Lexicon no-change observation.
    pub fn schedule_late_apple_label(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
    ) -> bool {
        let occurrence = &observation.occurrence;
        if observation.producer != ObservationProducer::Apple
            || text.trim().is_empty()
            || !self
                .serial_of(occurrence)
                .is_some_and(AcousticSerial::vad_closed)
            || self.is_sealed(occurrence)
            || self.committed.contains_key(occurrence)
            || self.answered.contains(observation)
        {
            return false;
        }
        let Some(frontier) = self.frontiers.get_mut(occurrence) else {
            return false;
        };
        if !frontier.is_closed()
            || !frontier.returned.contains(&ObservationProducer::Whisper)
            || frontier.scheduled.contains(&ObservationProducer::Apple)
            || frontier.scheduled.contains(&ObservationProducer::Lexicon)
        {
            return false;
        }
        frontier.schedule(ObservationProducer::Apple);
        frontier.schedule(ObservationProducer::Lexicon);
        true
    }

    /// Record that a scheduled producer finished with a range.
    ///
    /// Returns `true` only for the transition from open to closed. Repeated
    /// completion is idempotent and cannot cause a second seal emission. A
    /// range with no scheduled frontier answers `false`: unknown is never
    /// closed.
    pub fn note_frontier_return(
        &mut self,
        coverage: &OccurrenceIdentity,
        producer: ObservationProducer,
    ) -> bool {
        match self.frontiers.get_mut(coverage) {
            Some(frontier) => {
                let was_closed = frontier.is_closed();
                frontier.record_return(producer);
                !was_closed && frontier.is_closed()
            }
            None => false,
        }
    }

    /// The frontier kept for a range, if one was scheduled.
    pub fn frontier_of(&self, coverage: &OccurrenceIdentity) -> Option<&ObservationFrontier> {
        self.frontiers.get(coverage)
    }

    // -- seal ---------------------------------------------------------------

    /// Assemble the seal for one occurrence, or say exactly why it cannot seal.
    fn mint_seal(&self, occurrence: &OccurrenceIdentity) -> Result<LedgerSealReceipt, SealRefusal> {
        if self.text_recovery_pending(occurrence) {
            return Err(SealRefusal::TextRecoveryPending);
        }
        let serial = self
            .evidence
            .get(occurrence)
            .ok_or(SealRefusal::NotQualified)?;
        if !serial.vad_closed() {
            return Err(SealRefusal::VadDidNotClose);
        }
        let frontier = self
            .frontiers
            .get(occurrence)
            .ok_or(SealRefusal::FrontierUnknown)?;
        if !frontier.is_closed() {
            return Err(SealRefusal::FrontierOpen);
        }
        if self
            .text_of(occurrence)
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(SealRefusal::LabelMissing);
        }
        let answered = self
            .answered
            .iter()
            .filter(|observation| &observation.occurrence == occurrence)
            .count();
        let ordinals: Vec<usize> = self
            .trail
            .iter()
            .filter(|entry| &entry.observation.occurrence == occurrence)
            .map(|entry| entry.ordinal)
            .collect();
        if ordinals.len() != answered {
            return Err(SealRefusal::ObservationsWithoutReceipts);
        }
        Ok(LedgerSealReceipt {
            receipt_id: Self::seal_id(LedgerSealScope::Occurrence, occurrence),
            scope: LedgerSealScope::Occurrence,
            coverage: occurrence.clone(),
            sealed_occurrences: vec![occurrence.clone()],
            serials: vec![serial.clone()],
            vad_close_sample: serial.vad_close_sample.unwrap_or(occurrence.sample_end),
            frontier: frontier.clone(),
            layer_trail_ordinals: ordinals,
        })
    }

    /// Seal one occurrence. Idempotent: an already-sealed range returns the
    /// receipt it was sealed with, never a fresher one.
    pub fn seal(
        &mut self,
        occurrence: &OccurrenceIdentity,
    ) -> Result<&LedgerSealReceipt, SealRefusal> {
        if !self.seals.contains_key(occurrence) {
            let receipt = self.mint_seal(occurrence)?;
            self.seals.insert(occurrence.clone(), receipt);
        }
        self.seals
            .get(occurrence)
            .ok_or(SealRefusal::ObservationsWithoutReceipts)
    }

    /// Seal a whole session/capture epoch once every occurrence in it is sealed.
    ///
    /// The terminal seal is a statement about the same kind of thing an
    /// occurrence seal is — a coverage — so it carries the same receipt shape
    /// rather than minting a second finality vocabulary.
    pub fn seal_terminal(
        &mut self,
        session: &str,
        capture_epoch: u64,
    ) -> Result<LedgerSealReceipt, SealRefusal> {
        if !self
            .pending_text_recoveries(session, capture_epoch)
            .is_empty()
        {
            return Err(SealRefusal::TextRecoveryPending);
        }
        // Two distinct refusals, both terminal-blocking: measured uncovered
        // speech, and no authenticated measurement at all. Absence of evidence
        // may not certify a seal simply because it is not `Incomplete`.
        if self.latest_seal_coverage.as_ref().is_some_and(|coverage| {
            coverage.session_id == session
                && coverage.capture_epoch == capture_epoch
                && (coverage.status == SealCoverageStatus::Incomplete
                    || coverage.status.unavailable_reason().is_some())
        }) {
            return Err(SealRefusal::CoverageIncomplete);
        }
        let in_epoch: Vec<OccurrenceIdentity> = self
            .evidence
            .keys()
            .filter(|occurrence| {
                occurrence.session == session && occurrence.capture_epoch == capture_epoch
            })
            .cloned()
            .collect();
        if in_epoch.is_empty() {
            return Err(SealRefusal::NotQualified);
        }
        for occurrence in &in_epoch {
            self.seal(occurrence)?;
        }
        let mut serials = Vec::with_capacity(in_epoch.len());
        let mut ordinals = Vec::new();
        let mut vad_close = 0u64;
        for occurrence in &in_epoch {
            let seal = self
                .seals
                .get(occurrence)
                .ok_or(SealRefusal::OccurrenceStillOpen)?;
            serials.extend(seal.serials.iter().cloned());
            ordinals.extend(seal.layer_trail_ordinals.iter().copied());
            vad_close = vad_close.max(seal.vad_close_sample);
        }
        let first = in_epoch.first().expect("non-empty epoch");
        let last = in_epoch.last().expect("non-empty epoch");
        let coverage =
            OccurrenceIdentity::new(session, capture_epoch, first.sample_start, last.sample_end);
        // The terminal frontier schedules nobody: closure here is a restatement
        // of the constituent occurrence frontiers, which were each checked in
        // `seal` above. It is not independent evidence and must not be read as
        // any.
        let frontier = ObservationFrontier::scheduled(coverage.clone(), Vec::new());
        let receipt = LedgerSealReceipt {
            receipt_id: Self::seal_id(LedgerSealScope::Terminal, &coverage),
            scope: LedgerSealScope::Terminal,
            coverage,
            sealed_occurrences: in_epoch,
            serials,
            vad_close_sample: vad_close,
            frontier,
            layer_trail_ordinals: ordinals,
        };
        if !self.terminal_seals.contains(&receipt) {
            self.terminal_seals.push(receipt.clone());
        }
        Ok(receipt)
    }

    /// Compare the entire carrier with ledger-owned finality, never just its ID.
    pub fn authenticates_seal(&self, receipt: &LedgerSealReceipt) -> bool {
        match receipt.scope {
            LedgerSealScope::Occurrence => self.seal_of(&receipt.coverage) == Some(receipt),
            LedgerSealScope::Terminal => self.terminal_seals.contains(receipt),
        }
    }

    /// Read-only lookup of an already minted scope-bearing receipt.
    pub fn seal_receipt(&self, id: &str) -> Option<&LedgerSealReceipt> {
        self.seals
            .values()
            .chain(self.terminal_seals.iter())
            .find(|seal| seal.receipt_id == id)
    }

    /// A projected finality reference must cover this exact occurrence.
    pub fn authenticates_seal_reference(&self, occurrence: &OccurrenceIdentity, id: &str) -> bool {
        self.seal_of(occurrence)
            .is_some_and(|seal| seal.receipt_id == id)
            || self
                .terminal_seals
                .iter()
                .any(|seal| seal.receipt_id == id && seal.sealed_occurrences.contains(occurrence))
    }

    /// The seal held for one occurrence, if it is sealed.
    pub fn seal_of(&self, occurrence: &OccurrenceIdentity) -> Option<&LedgerSealReceipt> {
        self.seals.get(occurrence)
    }

    /// Whether the occurrence is final.
    pub fn is_sealed(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.seals.contains_key(occurrence)
    }

    /// Deterministic seal identifier for a coverage.
    fn seal_id(scope: LedgerSealScope, coverage: &OccurrenceIdentity) -> String {
        format!(
            "seal-{}-{}-{}-{}-{}",
            scope.as_str(),
            coverage.session,
            coverage.capture_epoch,
            coverage.sample_start,
            coverage.sample_end
        )
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
        self.trail
            .iter()
            .filter(move |entry| &entry.observation.occurrence == occurrence)
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

    /// Authenticate one provenance-bearing rewrite of the complete sealed document.
    ///
    /// A whole-document edit cannot honestly be divided back into occurrence
    /// labels without a new word-to-PCM alignment pass. This receipt therefore
    /// binds the edited bytes to the exact sealed occurrence set and source
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
        if source_occurrences.is_empty() {
            return Err("manual_document_occurrences_missing");
        }
        if source_occurrences.iter().any(|occurrence| {
            occurrence.session != session_id
                || !self.is_qualified(occurrence)
                || !self.is_sealed(occurrence)
                || !self.committed.contains_key(occurrence)
        }) {
            return Err("manual_document_occurrence_not_sealed");
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
        };
        self.manual_document_revisions.push(receipt.clone());
        Ok(receipt)
    }

    /// Every authenticated whole-document user revision, in arrival order.
    pub fn manual_document_revisions(&self) -> &[ManualDocumentRevisionReceipt] {
        &self.manual_document_revisions
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
        if !self.is_qualified(occurrence)
            || !self.is_sealed(occurrence)
            || !self.committed.contains_key(occurrence)
        {
            return Err("incremental_shaping_occurrence_not_sealed");
        }
        if self.text_of(occurrence) != Some(source_label) {
            return Err("incremental_shaping_source_label_stale");
        }
        let source_seal_receipt = self
            .seal_of(occurrence)
            .map(|seal| seal.receipt_id.clone())
            .ok_or("incremental_shaping_seal_missing")?;
        if source_label == shaped_text {
            return Err("incremental_shaping_unchanged");
        }
        if super::light_plus::apply_with_left_context(left_context, source_label) != shaped_text {
            return Err("incremental_shaping_not_deterministic");
        }
        if self
            .incremental_shapings
            .iter()
            .rev()
            .find(|held| &held.occurrence == occurrence)
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
            source_label: source_label.to_string(),
            left_context: left_context.to_string(),
            left_context_sha256: format!("{:x}", Sha256::digest(left_context.as_bytes())),
            shaped_text: shaped_text.to_string(),
        };
        self.incremental_shapings.push(receipt.clone());
        Ok(receipt)
    }

    /// Every authenticated per-occurrence shaping, in arrival order.
    pub fn incremental_shapings(&self) -> &[IncrementalShapingReceipt] {
        &self.incremental_shapings
    }

    /// Record the decision the ledger just took. Called for every observation
    /// without exception, so a token can never exist without its layer history.
    fn record_layer_decision(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        decision: &MutationReceipt,
    ) {
        let ordinal = self.trail.len();
        let predecessor_ordinal = self
            .trail
            .iter()
            .rposition(|entry| entry.observation.occurrence == observation.occurrence)
            .map(|position| self.trail[position].ordinal);
        let serials: Vec<AcousticSerial> = self
            .evidence
            .get(&observation.occurrence)
            .cloned()
            .into_iter()
            .collect();
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
        });
    }

    // -- derivation ---------------------------------------------------------

    /// Record a refinement of physical coverage as provenance.
    ///
    /// The parent coordinate, its serial and its decision history are left
    /// exactly as they were: this appends a derivation, it never edits PCM
    /// history. `Err` carries the exact reason the derivation was refused.
    pub fn record_derivation(
        &mut self,
        derivation: OccurrenceDerivation,
        calibration: &EnergyCalibration,
    ) -> Result<(), &'static str> {
        if let Some(reason) = derivation.rejects(calibration) {
            return Err(reason);
        }
        if derivation
            .parents()
            .iter()
            .any(|parent| !self.evidence.contains_key(*parent))
        {
            return Err("derivation_parent_not_qualified");
        }
        self.derivations.push(derivation);
        Ok(())
    }

    /// Every recorded derivation, in arrival order.
    pub fn derivations(&self) -> &[OccurrenceDerivation] {
        &self.derivations
    }

    /// Derivations that name one occurrence as a parent or a child.
    pub fn derivations_of<'a>(
        &'a self,
        occurrence: &'a OccurrenceIdentity,
    ) -> impl Iterator<Item = &'a OccurrenceDerivation> {
        self.derivations.iter().filter(move |derivation| {
            derivation
                .parents()
                .into_iter()
                .chain(derivation.children())
                .any(|coordinate| coordinate == occurrence)
        })
    }

    // -- composition --------------------------------------------------------

    /// Compose the signed token sequence for one coverage.
    ///
    /// Fails closed: an occurrence without a serial, or holding an empty label,
    /// stops the whole composition rather than quietly delivering a token that
    /// nothing signed or dropping a physical event.
    pub fn compose(
        &self,
        coverage: &OccurrenceIdentity,
    ) -> Result<OccurrenceComposition, EvidenceRefusal> {
        let mut tokens = Vec::new();
        for (occurrence, held) in &self.committed {
            if !occurrence.same_capture(coverage)
                || occurrence.sample_start < coverage.sample_start
                || occurrence.sample_end > coverage.sample_end
            {
                continue;
            }
            let serial = self
                .evidence
                .get(occurrence)
                .ok_or(EvidenceRefusal::OccurrenceNotQualified)?;
            let observation = ObservationIdentity::new(
                held.producer,
                held.request,
                held.generation,
                occurrence.clone(),
            );
            let before = tokens.len();
            // The ordinal is observation-local by definition: it says where the
            // token sat inside the label its producer emitted, not where it sat
            // in the document. Document order is the composition's own order.
            for (ordinal, word) in held.text.split_whitespace().enumerate() {
                tokens.push(WordEvidenceReceipt::cite(
                    word,
                    ordinal,
                    &observation,
                    vec![serial.clone()],
                    Some((occurrence.sample_start, occurrence.sample_end)),
                )?);
            }
            if tokens.len() == before {
                return Err(EvidenceRefusal::EmptyToken);
            }
        }
        Ok(OccurrenceComposition {
            coverage: coverage.clone(),
            tokens,
        })
    }
}

/// Decision for a cumulative engine final that restates committed text.
///
/// The Apple live lane emits finals whose text restates the whole phrase while
/// the window it declares covers only the newest audio. Splitting restated from
/// novel is therefore a *text* operation, and this enum bounds how much
/// authority that text operation is allowed to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CumulativeFinalAdmission {
    /// The declared window is disjoint from every committed occurrence: this is
    /// new audio, the whole callback is novel, and text alignment is not
    /// consulted at all.
    WhollyNovel,
    /// The window overlaps committed occurrences. Alignment is authorized, but
    /// only up to the number of canvas words those occurrences actually hold —
    /// a restatement may not claim more occurrences than exist.
    AlignInside {
        /// Canvas words, counted from the commit cursor backwards, that the
        /// overlapped occurrences account for.
        canvas_words_under_authority: usize,
    },
    /// No usable anchor on either side. Legacy text behaviour stands, because
    /// there is no acoustic authority for it to violate.
    NoAnchor {
        /// Why authority is absent.
        reason: NoAuthorityReason,
    },
}

impl CumulativeFinalAdmission {
    /// Decide how much of the canvas a cumulative final may claim to restate.
    ///
    /// `window` is the range the callback declares. `committed` pairs each
    /// canvas span with the number of canvas words it contributed, newest last.
    /// Spans whose range is unanchored are treated as absent evidence, never as
    /// objections.
    pub fn decide(window: &OccurrenceIdentity, committed: &[(OccurrenceIdentity, usize)]) -> Self {
        if !window.is_anchored() {
            return Self::NoAnchor {
                reason: NoAuthorityReason::ZeroWidth,
            };
        }
        let comparable: Vec<&(OccurrenceIdentity, usize)> = committed
            .iter()
            .filter(|(occurrence, _)| occurrence.is_anchored() && occurrence.same_capture(window))
            .collect();
        if comparable.is_empty() {
            // Nothing anchored to compare against. The legacy text lane keeps
            // its behaviour rather than being handed a false verdict.
            return Self::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            };
        }
        let mut words_under_authority = 0usize;
        let mut touched = false;
        // Walk newest-first: authority extends backwards from the commit cursor
        // only while the occurrences keep sharing audio with the window.
        for (occurrence, words) in comparable.iter().rev() {
            match window.relate(occurrence) {
                OccurrenceRelation::Same | OccurrenceRelation::Overlapping { .. } => {
                    touched = true;
                    words_under_authority += words;
                }
                OccurrenceRelation::Disjoint => break,
                OccurrenceRelation::Unanchored | OccurrenceRelation::DifferentCapture => continue,
            }
        }
        if !touched {
            return Self::WhollyNovel;
        }
        Self::AlignInside {
            canvas_words_under_authority: words_under_authority,
        }
    }

    /// Authority for a *cumulative* producer, whose text restates more audio
    /// than the window it declares.
    ///
    /// The Apple live lane emits segment-less finals that restate the whole
    /// phrase while the window they carry covers only the newest audio. Such a
    /// window cannot bound the alignment, because the producer under-declares
    /// by construction — believing it would mark the whole restatement as new
    /// audio and commit every occurrence twice.
    ///
    /// What does bound the alignment is the finality bar. A live callback may
    /// realign occurrences the live lane still holds open, and has no authority
    /// over anything already past `transcript_sealed`. `open_occurrences` is
    /// that set, each paired with the number of canvas words it contributed.
    pub fn for_cumulative_restatement(open_occurrences: &[(OccurrenceIdentity, usize)]) -> Self {
        let words: usize = open_occurrences.iter().map(|(_, words)| words).sum();
        if words == 0 {
            return Self::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            };
        }
        Self::AlignInside {
            canvas_words_under_authority: words,
        }
    }

    /// Slice the canvas down to the region the decision authorises.
    ///
    /// Alignment runs *after* authority is established and only inside the
    /// authorised span; a textual match outside it is a coincidence, not
    /// identity, and must not be reachable by the matcher at all.
    pub fn authorized_canvas<'c>(&self, canvas: &'c [&'c str]) -> &'c [&'c str] {
        match self {
            Self::WhollyNovel => &[],
            Self::AlignInside {
                canvas_words_under_authority,
            } => {
                let take = (*canvas_words_under_authority).min(canvas.len());
                &canvas[canvas.len() - take..]
            }
            Self::NoAnchor { .. } => canvas,
        }
    }

    /// Clamp a text matcher's answer to what acoustic evidence permits.
    ///
    /// This is where string-prefix and overlap heuristics lose their authority: their answer is an
    /// alignment *hint*, and on an anchored span it may never exceed the number
    /// of committed occurrences the window actually overlaps.
    pub fn clamp_known_prefix(&self, matcher_known_words: usize) -> usize {
        match self {
            Self::WhollyNovel => 0,
            Self::AlignInside {
                canvas_words_under_authority,
            } => matcher_known_words.min(*canvas_words_under_authority),
            Self::NoAnchor { .. } => matcher_known_words,
        }
    }

    /// Stable label for logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WhollyNovel => "wholly_novel",
            Self::AlignInside { .. } => "align_inside",
            Self::NoAnchor { .. } => "no_anchor",
        }
    }
}

// ---------------------------------------------------------------------------
// Physical evidence, calibration, and admission
// ---------------------------------------------------------------------------

/// The named, measured existence threshold an occurrence must clear.
///
/// * inputs: values supplied by the immutable runtime settings snapshot.
/// * outputs: the qualification verdict in [`AcousticLedger::qualify`] and the
///   valley floor in [`AcousticLedger::record_derivation`].
/// * invariants: there is no `Default`. A threshold this ledger invented itself
///   would be a runtime calibration decision, and W1 is not allowed to make
///   one; the caller must state the measured floor and the version it was
///   measured under.
/// * intended W2 consumers: the capture/VAD evidence path that qualifies PCM
///   before any producer is allowed to speak about it.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyCalibration {
    /// Version label of the calibration run these floors came from. It travels
    /// into every serial so a receipt can never be read under the wrong ruler.
    pub version: String,
    /// Minimum energy integral a region must reach to exist at all.
    pub min_energy_integral: f64,
    /// Minimum silent samples that must separate two regions before they may be
    /// called two physical events.
    pub min_valley_samples: u64,
}

impl EnergyCalibration {
    /// Build a calibration from measured floors.
    pub fn new(
        version: impl Into<String>,
        min_energy_integral: f64,
        min_valley_samples: u64,
    ) -> Self {
        Self {
            version: version.into(),
            min_energy_integral,
            min_valley_samples,
        }
    }
}

/// Physical evidence measured over one candidate PCM region.
///
/// * inputs: the capture clock, the energy hops the session already records,
///   and the VAD boundaries Silero already emits.
/// * outputs: [`AcousticSerial`] and the admission verdict.
/// * invariants: no field here is ever part of an identity key. Energy and
///   dBFS *qualify* a region; they never say which region it is.
/// * intended W2 consumers: `core/audio/streaming_recorder.rs` and the VAD
///   evidence path.
#[derive(Debug, Clone, PartialEq)]
pub struct AcousticEvidence {
    /// The physical coordinate the evidence was measured over.
    pub occurrence: OccurrenceIdentity,
    /// Duration of the region in milliseconds on the capture clock.
    pub duration_ms: f64,
    /// Summed `rms^2 * sample_count` over the region's hops.
    pub energy_integral: f64,
    /// Mean RMS of the region in dBFS.
    pub mean_rms_dbfs: f64,
    /// Peak level of the region in dBFS.
    pub peak_dbfs: f64,
    /// Sample at which VAD opened the region, when it opened one.
    pub vad_open_sample: Option<u64>,
    /// Sample at which VAD closed the region, when it closed one. `None` is the
    /// N5 shape: real energy, no closing boundary, therefore never sealable.
    pub vad_close_sample: Option<u64>,
    /// Calibration version the energy figures were measured under.
    pub evidence_calibration_version: String,
}

/// Why a candidate region was refused admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The range names no audio (zero-width or reversed).
    ZeroWidth,
    /// Energy stayed under the calibrated floor: below-threshold noise, not
    /// speech.
    BelowCalibratedEnergy,
    /// VAD never opened at or before the first sample, so nothing bounded the
    /// region.
    VadDidNotOpen,
    /// The evidence was measured under a different calibration than the one the
    /// caller is judging it with. Comparing them would silently change the
    /// meaning of the floor.
    CalibrationMismatch,
}

impl AdmissionRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZeroWidth => "zero_width",
            Self::BelowCalibratedEnergy => "below_calibrated_energy",
            Self::VadDidNotOpen => "vad_did_not_open",
            Self::CalibrationMismatch => "calibration_mismatch",
        }
    }
}

/// The ledger's answer to "did speech physically happen here?".
///
/// * inputs: [`AcousticEvidence`] and [`EnergyCalibration`].
/// * outputs: a minted [`AcousticSerial`] on qualification, a typed refusal
///   otherwise.
/// * invariants: no lexical content participates in this decision, in either
///   direction.
/// * intended W2 consumers: capture/VAD, which must qualify a region before any
///   producer may submit an observation for it.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionReceipt {
    /// The region exists. Its mandatory serial is minted and held.
    Qualified {
        /// The physical event.
        occurrence: OccurrenceIdentity,
        /// The evidence receipt every later token must be able to cite.
        serial: AcousticSerial,
    },
    /// The region does not qualify as physical speech.
    Refused {
        /// The coordinate that was offered.
        occurrence: OccurrenceIdentity,
        /// Why it was refused.
        reason: AdmissionRefusal,
    },
}

impl AdmissionReceipt {
    /// Whether an occurrence entered the ledger's evidence table.
    pub fn is_qualified(&self) -> bool {
        matches!(self, Self::Qualified { .. })
    }

    /// The minted serial, when the region qualified.
    pub fn serial(&self) -> Option<&AcousticSerial> {
        match self {
            Self::Qualified { serial, .. } => Some(serial),
            Self::Refused { .. } => None,
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Qualified { .. } => "qualified",
            Self::Refused { .. } => "refused",
        }
    }
}

// ---------------------------------------------------------------------------
// Mandatory versioned acoustic serial
// ---------------------------------------------------------------------------

/// Version of the canonical serial input below. Bump it whenever a field is
/// added, removed, or reinterpreted, so an old receipt can never be silently
/// re-read under new rules.
pub const ACOUSTIC_SERIAL_VERSION: u16 = 1;

/// Domain separator for the serial digest, so a digest of these bytes can never
/// collide with a digest taken elsewhere in the product.
const ACOUSTIC_SERIAL_DOMAIN: &str = "codescribe.acoustic-serial.v1";

/// The mandatory, versioned receipt of physical evidence for one occurrence.
///
/// * inputs: [`AcousticEvidence`] at qualification time.
/// * outputs: the digest and evidence fields every emitted token must cite
///   through a [`WordEvidenceReceipt`].
/// * invariants: **a serial is not an identity key.** It deliberately does not
///   implement `Eq`, `Hash`, or `Ord`, so the type system alone prevents it from
///   ever becoming the key of a map, a set, or a sorted ledger. The only key is
///   [`OccurrenceIdentity`]. The digest is deterministic: identical evidence
///   under an identical calibration yields an identical serial, with no clock,
///   counter, or ordering in its input.
/// * intended W2 consumers: the reducer, the Transcript Bus projection, the
///   bridge, and the Swift overlay — all of which read it and none of which
///   mint it.
#[derive(Debug, Clone, PartialEq)]
pub struct AcousticSerial {
    /// Version of the canonical input this digest was taken over.
    pub version: u16,
    /// Lowercase hex SHA-256 over the canonical input.
    pub digest: String,
    /// The physical coordinate the serial is rooted in.
    pub occurrence: OccurrenceIdentity,
    /// Duration of the region in milliseconds.
    pub duration_ms: f64,
    /// Calibrated energy integral (area) of the region.
    pub energy_integral: f64,
    /// Mean RMS of the region in dBFS.
    pub mean_rms_dbfs: f64,
    /// Peak level of the region in dBFS.
    pub peak_dbfs: f64,
    /// VAD opening boundary, when one exists.
    pub vad_open_sample: Option<u64>,
    /// VAD closing boundary, when one exists. Its absence is what keeps an
    /// energy-qualified occurrence unsealable.
    pub vad_close_sample: Option<u64>,
    /// Calibration version the energy figures were measured under.
    pub evidence_calibration_version: String,
}

impl AcousticSerial {
    /// Mint the serial for one qualified region.
    ///
    /// Called only by [`AcousticLedger::qualify`]: minting is an admission
    /// consequence, never a producer's privilege.
    pub fn mint(evidence: &AcousticEvidence) -> Self {
        let digest = Self::digest_of(evidence);
        Self {
            version: ACOUSTIC_SERIAL_VERSION,
            digest,
            occurrence: evidence.occurrence.clone(),
            duration_ms: evidence.duration_ms,
            energy_integral: evidence.energy_integral,
            mean_rms_dbfs: evidence.mean_rms_dbfs,
            peak_dbfs: evidence.peak_dbfs,
            vad_open_sample: evidence.vad_open_sample,
            vad_close_sample: evidence.vad_close_sample,
            evidence_calibration_version: evidence.evidence_calibration_version.clone(),
        }
    }

    /// The exact byte string the digest is taken over.
    ///
    /// Floats enter as their IEEE-754 bit patterns rather than as formatted
    /// decimals: a decimal rendering is locale- and precision-dependent, and a
    /// receipt whose digest depends on how a number was printed is not a
    /// receipt.
    pub fn canonical_input(evidence: &AcousticEvidence) -> String {
        let boundary = |sample: Option<u64>| match sample {
            Some(value) => value.to_string(),
            None => "none".to_string(),
        };
        [
            ACOUSTIC_SERIAL_DOMAIN.to_string(),
            ACOUSTIC_SERIAL_VERSION.to_string(),
            evidence.occurrence.session.clone(),
            evidence.occurrence.capture_epoch.to_string(),
            evidence.occurrence.sample_start.to_string(),
            evidence.occurrence.sample_end.to_string(),
            evidence.duration_ms.to_bits().to_string(),
            evidence.energy_integral.to_bits().to_string(),
            evidence.mean_rms_dbfs.to_bits().to_string(),
            evidence.peak_dbfs.to_bits().to_string(),
            boundary(evidence.vad_open_sample),
            boundary(evidence.vad_close_sample),
            evidence.evidence_calibration_version.clone(),
        ]
        .join("\n")
    }

    /// Lowercase hex SHA-256 over [`AcousticSerial::canonical_input`].
    pub fn digest_of(evidence: &AcousticEvidence) -> String {
        let mut hasher = Sha256::new();
        hasher.update(Self::canonical_input(evidence).as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether VAD closed the region at or after its last sample.
    ///
    /// This is the seal's boundary evidence, and the reason an energy-only
    /// region stays open forever.
    pub fn vad_closed(&self) -> bool {
        self.vad_close_sample
            .is_some_and(|close| close >= self.occurrence.sample_end)
    }
}

// ---------------------------------------------------------------------------
// Word evidence
// ---------------------------------------------------------------------------

/// Why a token could not be given an evidence receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRefusal {
    /// The token cited no acoustic serial at all. This is the fail-closed edge:
    /// an unsigned token is not emitted, it is refused.
    NoSerialCited,
    /// The token carried no text to attribute.
    EmptyToken,
    /// The declared token coverage falls outside every serial it cites, so the
    /// citation does not actually support the token.
    CoverageOutsideCitedSerials,
    /// A composed occurrence has no serial, so its tokens cannot be signed.
    OccurrenceNotQualified,
}

impl EvidenceRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSerialCited => "no_serial_cited",
            Self::EmptyToken => "empty_token",
            Self::CoverageOutsideCitedSerials => "coverage_outside_cited_serials",
            Self::OccurrenceNotQualified => "occurrence_not_qualified",
        }
    }
}

/// The mandatory signature one emitted lexical token carries.
///
/// * inputs: the token text, its observation-local ordinal, the producer that
///   emitted it, and one or more [`AcousticSerial`]s.
/// * outputs: the per-word evidence row the runtime trace and the overlay read.
/// * invariants: at least one serial is always cited — construction fails
///   closed otherwise. Citing a serial does **not** make the serial a second
///   identity key: many tokens may cite one occurrence, and one token may cite
///   several, precisely because the citation is a receipt and not a key.
/// * intended W2 consumers: `app/presentation/emitter.rs`,
///   `app/presentation/transcript_bus.rs`, and the overlay projection.
#[derive(Debug, Clone, PartialEq)]
pub struct WordEvidenceReceipt {
    /// The emitted lexical token. A label, never a key.
    pub token: String,
    /// Ordinal of this token inside the observation that emitted it.
    pub token_ordinal: usize,
    /// Producer that emitted the token.
    pub producer: ObservationProducer,
    /// Producer-local generation the token was emitted at.
    pub generation: u64,
    /// Serials the token is rooted in. Never empty.
    pub serials: Vec<AcousticSerial>,
    /// First sample the producer attributes to this token, when it provides one.
    pub token_sample_start: Option<u64>,
    /// One past the last sample the producer attributes to this token, when it
    /// provides one.
    pub token_sample_end: Option<u64>,
}

impl WordEvidenceReceipt {
    /// Sign one token with the serials it is rooted in, or refuse it.
    pub fn cite(
        token: impl Into<String>,
        token_ordinal: usize,
        observation: &ObservationIdentity,
        serials: Vec<AcousticSerial>,
        coverage: Option<(u64, u64)>,
    ) -> Result<Self, EvidenceRefusal> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(EvidenceRefusal::EmptyToken);
        }
        if serials.is_empty() {
            return Err(EvidenceRefusal::NoSerialCited);
        }
        if let Some((start, end)) = coverage {
            let supported = serials.iter().any(|serial| {
                start < end
                    && start >= serial.occurrence.sample_start
                    && end <= serial.occurrence.sample_end
            });
            if !supported {
                return Err(EvidenceRefusal::CoverageOutsideCitedSerials);
            }
        }
        Ok(Self {
            token,
            token_ordinal,
            producer: observation.producer,
            generation: observation.generation,
            serials,
            token_sample_start: coverage.map(|(start, _)| start),
            token_sample_end: coverage.map(|(_, end)| end),
        })
    }

    /// The occurrences this token cites, in citation order.
    pub fn cited_occurrences(&self) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.serials.iter().map(|serial| &serial.occurrence)
    }

    /// The hex digests this token cites, in citation order.
    pub fn cited_digests(&self) -> impl Iterator<Item = &str> {
        self.serials.iter().map(|serial| serial.digest.as_str())
    }
}

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
    Formatter,
    /// Deterministic Light+ sentence shaping minted by Rust at the terminal
    /// seal, before any formatter or user edit sees the document.
    LightPlus,
}

impl DocumentRevisionProvenance {
    /// Stable spelling used by ledger receipts and Bus consumers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserEdit => "user-edit",
            Self::Formatter => "formatter",
            Self::LightPlus => "light-plus",
        }
    }
}

/// Provenance for an explicit rewrite of a complete sealed transcript.
///
/// The receipt names every source occurrence and seal but deliberately carries
/// no fabricated per-word alignment for the replacement text. It is append-only
/// evidence consumed by the Rust transcript reducer and its Bus projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualDocumentRevisionReceipt {
    /// Stable identifier copied into each projected acoustic receipt.
    pub receipt_id: String,
    /// Stable origin label for quality capture and external observers.
    pub provenance: String,
    /// Recording session whose sealed document was revised.
    pub session_id: String,
    /// Reducer revision the user actually edited.
    pub source_revision: u64,
    /// New reducer revision minted by Rust.
    pub revision: u64,
    /// Exact occurrence identities that made up the source document.
    pub source_occurrences: Vec<OccurrenceIdentity>,
    /// Seals proving those source occurrences were terminal before the edit.
    pub source_seal_receipts: Vec<String>,
    /// Complete user-authored replacement bytes.
    pub rendered_text: String,
}

/// Provenance for one deterministic presentation shaping of a single sealed
/// occurrence, minted while the session lifecycle is still open.
///
/// * inputs: one closed occurrence, the exact committed label it holds, and the
///   committed left neighbourhood the casing decision saw.
/// * outputs: the presentation bytes the reducer renders for that occurrence.
/// * invariants: the acoustic label, its evidence, its geometry and its seal
///   stay untouched — this receipt sits *beside* them. It names exactly one
///   occurrence, so an open suffix can never be smuggled into a sealed source
///   set, and it is not a [`ManualDocumentRevisionReceipt`]: a partial shape
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
    /// Seal proving that occurrence was closed before it was shaped.
    pub source_seal_receipt: String,
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
///   between, the one closed occurrence being shaped, the exact committed
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
}

// ---------------------------------------------------------------------------
// Observation frontier
// ---------------------------------------------------------------------------

/// Which scheduled producers may still speak about a range.
///
/// * inputs: the schedule of producers the session actually dispatched for a
///   range, and their returns.
/// * outputs: the closed/open verdict the seal depends on.
/// * invariants: closure means *no scheduled producer can still return a valid
///   observation*. It is not a timeout and not a guess about lateness; an
///   unreturned producer keeps the frontier open however long it takes. This is
///   the N6 shape: VAD closed, frontier still open, therefore not sealed.
/// * intended W2 consumers: the streaming session scheduler, which reports
///   dispatch and completion, and never the seal itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationFrontier {
    /// Range the frontier is kept for.
    pub coverage: OccurrenceIdentity,
    scheduled: BTreeSet<ObservationProducer>,
    returned: BTreeSet<ObservationProducer>,
}

impl ObservationFrontier {
    /// Open a frontier for the producers a session actually scheduled.
    pub fn scheduled(
        coverage: OccurrenceIdentity,
        producers: impl IntoIterator<Item = ObservationProducer>,
    ) -> Self {
        Self {
            coverage,
            scheduled: producers.into_iter().collect(),
            returned: BTreeSet::new(),
        }
    }

    /// Extend an open launch contract without discarding returns already
    /// recorded for synchronous observers.
    fn schedule(&mut self, producer: ObservationProducer) -> bool {
        self.scheduled.insert(producer)
    }

    /// Record that a scheduled producer has returned everything it will return.
    pub fn record_return(&mut self, producer: ObservationProducer) {
        if self.scheduled.contains(&producer) {
            self.returned.insert(producer);
        }
    }

    /// Producers that were scheduled and have not finished.
    pub fn open_producers(&self) -> Vec<ObservationProducer> {
        self.scheduled.difference(&self.returned).copied().collect()
    }

    /// Whether every scheduled producer has finished.
    pub fn is_closed(&self) -> bool {
        self.scheduled.is_subset(&self.returned)
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        if self.is_closed() { "closed" } else { "open" }
    }
}

// ---------------------------------------------------------------------------
// Seal
// ---------------------------------------------------------------------------

/// Why a range could not be sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealRefusal {
    /// No energy-qualified occurrence exists for the range.
    NotQualified,
    /// VAD never emitted a closing boundary at or after the last sample.
    VadDidNotClose,
    /// A scheduled producer can still return a valid observation.
    FrontierOpen,
    /// No frontier was ever scheduled for the range, so closure is unknown.
    /// Unknown is not closed.
    FrontierUnknown,
    /// All scheduled observers returned, but none supplied a usable label.
    LabelMissing,
    /// Speech outlasted its provisional label and has not been recovered.
    TextRecoveryPending,
    /// An admitted observation for the range has no decision receipt.
    ObservationsWithoutReceipts,
    /// The terminal seal was asked for while an occurrence in the epoch is
    /// still unsealed.
    OccurrenceStillOpen,
    /// Measured speech still contains a material uncovered range.
    CoverageIncomplete,
}

impl SealRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotQualified => "not_qualified",
            Self::VadDidNotClose => "vad_did_not_close",
            Self::FrontierOpen => "frontier_open",
            Self::FrontierUnknown => "frontier_unknown",
            Self::LabelMissing => "label_missing",
            Self::TextRecoveryPending => "text_recovery_pending",
            Self::ObservationsWithoutReceipts => "observations_without_receipts",
            Self::OccurrenceStillOpen => "occurrence_still_open",
            Self::CoverageIncomplete => "coverage_incomplete",
        }
    }
}

/// Finality scope minted by the ledger; independent of occurrence count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerSealScope {
    Occurrence,
    Terminal,
}

impl LedgerSealScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Occurrence => "occurrence",
            Self::Terminal => "terminal",
        }
    }
}

/// An immutable ledger fact: this coverage is finished.
///
/// * inputs: the qualified serial, the VAD closing boundary, the closed
///   [`ObservationFrontier`], and the complete decision trail for the range.
/// * outputs: the fence every later automatic producer meets in
///   [`AcousticLedger::admit`], and the terminal seal a session ends on.
/// * invariants: a seal is never lifted, downgraded, or recomputed. One receipt
///   shape covers both a single occurrence and a whole session/epoch, because a
///   seal is a statement about a coverage, and a coverage is a coordinate.
/// * intended W2 consumers: the Transcript Bus terminal seal event and the
///   reducer's finality projection — both of which read this fact rather than
///   deciding finality themselves.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerSealReceipt {
    /// Ledger-minted scope. Cardinality never determines finality.
    pub scope: LedgerSealScope,
    /// Stable identifier for cross-referencing from a projection.
    pub receipt_id: String,
    /// Coordinate the seal covers.
    pub coverage: OccurrenceIdentity,
    /// Occurrences the seal makes final: one for occurrence scope, one or more
    /// for terminal scope. The length does not determine the scope.
    pub sealed_occurrences: Vec<OccurrenceIdentity>,
    /// Serials of those occurrences, in the same order.
    pub serials: Vec<AcousticSerial>,
    /// VAD closing boundary that permitted the seal.
    pub vad_close_sample: u64,
    /// The frontier as it stood, closed, at seal time.
    pub frontier: ObservationFrontier,
    /// Ordinals of the decision trail entries the seal makes final.
    pub layer_trail_ordinals: Vec<usize>,
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
        let digest = |text: &str| format!("{:x}", Sha256::digest(text.as_bytes()));
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

impl LedgerSealReceipt {
    /// Stable state label for receipts and logs. A seal only ever has one.
    pub fn state(&self) -> &'static str {
        "sealed"
    }

    /// Whether the ledger minted occurrence finality, including single-entry terminals.
    pub fn is_occurrence_seal(&self) -> bool {
        self.scope == LedgerSealScope::Occurrence
    }
}

// ---------------------------------------------------------------------------
// Derivation and composition
// ---------------------------------------------------------------------------

/// Refinement of physical coverage that mints provenance instead of rewriting
/// history.
///
/// * inputs: a later, finer or coarser segmentation of already-qualified audio.
/// * outputs: new provenance identities recorded next to — never instead of —
///   the coordinates and serials already on the ledger.
/// * invariants: a derivation never edits a parent occurrence, never re-mints a
///   parent serial, and never invents a split that the calibrated valley floor
///   does not support. Token count is not evidence of a physical boundary.
/// * intended W2 consumers: the VAD/segmentation refinement path, and nothing
///   that authors text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceDerivation {
    /// One qualified region resolved into several, separated by real valleys.
    Split {
        /// The region as it was qualified.
        parent: OccurrenceIdentity,
        /// The finer regions, in capture order.
        children: Vec<OccurrenceIdentity>,
    },
    /// Several qualified regions recognised as one physical event.
    Merge {
        /// The regions as they were qualified, in capture order.
        parents: Vec<OccurrenceIdentity>,
        /// The coarser region they compose.
        child: OccurrenceIdentity,
    },
    /// One region's boundaries tightened without changing how many events exist.
    Refine {
        /// The region as it was qualified.
        parent: OccurrenceIdentity,
        /// The tightened region.
        child: OccurrenceIdentity,
    },
}

impl OccurrenceDerivation {
    /// Coordinates this derivation reads.
    pub fn parents(&self) -> Vec<&OccurrenceIdentity> {
        match self {
            Self::Split { parent, .. } | Self::Refine { parent, .. } => vec![parent],
            Self::Merge { parents, .. } => parents.iter().collect(),
        }
    }

    /// Coordinates this derivation mints.
    pub fn children(&self) -> Vec<&OccurrenceIdentity> {
        match self {
            Self::Split { children, .. } => children.iter().collect(),
            Self::Merge { child, .. } | Self::Refine { child, .. } => vec![child],
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Split { .. } => "split",
            Self::Merge { .. } => "merge",
            Self::Refine { .. } => "refine",
        }
    }

    /// Why this derivation may not be recorded, if it may not.
    ///
    /// `None` means the derivation only adds provenance. Every other answer is
    /// a refusal to rewrite PCM history.
    pub fn rejects(&self, calibration: &EnergyCalibration) -> Option<&'static str> {
        let parents = self.parents();
        let children = self.children();
        if parents.is_empty() || children.is_empty() {
            return Some("derivation_without_endpoints");
        }
        let anchor = parents[0];
        if parents
            .iter()
            .chain(children.iter())
            .any(|coordinate| !coordinate.is_anchored())
        {
            return Some("derivation_endpoint_unanchored");
        }
        if parents
            .iter()
            .chain(children.iter())
            .any(|coordinate| !coordinate.same_capture(anchor))
        {
            return Some("derivation_crosses_capture");
        }
        match self {
            Self::Split { parent, children } => {
                if children.len() < 2 {
                    return Some("split_without_two_children");
                }
                if children.iter().any(|child| {
                    child.sample_start < parent.sample_start || child.sample_end > parent.sample_end
                }) {
                    return Some("split_child_outside_parent");
                }
                let mut ordered: Vec<&OccurrenceIdentity> = children.iter().collect();
                ordered.sort();
                let separated = ordered.windows(2).all(|pair| {
                    pair[1].sample_start.saturating_sub(pair[0].sample_end)
                        >= calibration.min_valley_samples
                });
                if !separated {
                    // A2: adjacent lexical tokens without a qualifying valley are
                    // not two physical events, however many words were heard.
                    return Some("split_without_calibrated_valley");
                }
                None
            }
            Self::Merge { parents, child } => {
                if parents.len() < 2 {
                    return Some("merge_without_two_parents");
                }
                if parents.iter().any(|parent| {
                    parent.sample_start < child.sample_start || parent.sample_end > child.sample_end
                }) {
                    return Some("merge_parent_outside_child");
                }
                None
            }
            Self::Refine { parent, child } => {
                if child.sample_start < parent.sample_start || child.sample_end > parent.sample_end
                {
                    return Some("refine_child_outside_parent");
                }
                None
            }
        }
    }
}

/// The ordered lexical composition of one coverage.
///
/// * inputs: the labels the ledger currently holds for the occurrences under a
///   coverage, plus their serials.
/// * outputs: the signed token sequence a reducer projects into a document.
/// * invariants: composition fails closed. Every token in it carries a
///   [`WordEvidenceReceipt`], so an unsigned token cannot reach a document
///   through this part. Occurrence count, not token count, is the conserved
///   quantity: five occurrences labelled `Iwo` compose five signed tokens.
/// * intended W2 consumers: `app/presentation/emitter.rs::TranscriptReducer`.
#[derive(Debug, Clone, PartialEq)]
pub struct OccurrenceComposition {
    /// Coordinate the composition covers.
    pub coverage: OccurrenceIdentity,
    /// Signed tokens, in capture order.
    pub tokens: Vec<WordEvidenceReceipt>,
}

impl OccurrenceComposition {
    /// Distinct occurrences the composed tokens cite.
    pub fn occurrence_count(&self) -> usize {
        self.tokens
            .iter()
            .flat_map(|token| token.cited_occurrences())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Number of signed tokens.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Whether every token carries at least one serial.
    ///
    /// Construction already guarantees this; the accessor exists so a
    /// projection can assert it at its own boundary instead of trusting ours.
    pub fn is_fully_signed(&self) -> bool {
        self.tokens.iter().all(|token| !token.serials.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture_receipt::CaptureEvidenceIdentity;

    fn occ(start: u64, end: u64) -> OccurrenceIdentity {
        OccurrenceIdentity::new("s1", 1, start, end)
    }

    fn obs(
        producer: ObservationProducer,
        generation: u64,
        occurrence: OccurrenceIdentity,
    ) -> ObservationIdentity {
        ObservationIdentity::new(producer, 7, generation, occurrence)
    }

    fn whisper_only_qualified_ledger() -> (AcousticLedger, OccurrenceIdentity) {
        let occurrence = occ(0, 16_000);
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("label-finality", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(0),
            vad_close_sample: Some(16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
        (ledger, occurrence)
    }

    fn debt_speech() -> AcousticSpeechEvidence {
        measured_speech(
            "s1",
            1,
            16_000,
            vec![TailSampleRange {
                session: "s1".to_string(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 16_000,
            }],
        )
    }

    #[test]
    fn text_recovery_debt_retains_apple_but_refuses_false_coverage_and_seals() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        ledger.admit(
            &obs(ObservationProducer::Apple, 0, occurrence.clone()),
            "partial",
        );
        let before = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 32_000);
        assert_eq!(before.coverage_ratio(), Some(1.0));
        assert!(ledger.record_seal_coverage(before));
        assert!(ledger.require_text_recovery(&occurrence));
        assert!(ledger.require_text_recovery(&occurrence));
        assert_eq!(
            ledger.pending_text_recoveries("s1", 1),
            vec![occurrence.clone()]
        );
        let after = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 32_000);
        assert_eq!(after.status, SealCoverageStatus::Incomplete);
        assert_eq!(after.coverage_ratio(), Some(0.0));
        assert_eq!(after.max_uncovered_samples, 16_000);
        assert_eq!(ledger.text_of(&occurrence), Some("partial"));
        ledger.admit(
            &obs(ObservationProducer::Whisper, 0, occurrence.clone()),
            " ",
        );
        ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
        assert!(ledger.text_recovery_pending(&occurrence));
        assert_eq!(
            ledger.seal(&occurrence),
            Err(SealRefusal::TextRecoveryPending)
        );
        assert_eq!(
            ledger.seal_terminal("s1", 1),
            Err(SealRefusal::TextRecoveryPending)
        );
    }

    #[test]
    fn text_recovery_debt_resolves_only_after_exact_authorized_observation() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        ledger.admit(
            &obs(ObservationProducer::Apple, 0, occurrence.clone()),
            "partial",
        );
        assert!(ledger.require_text_recovery(&occurrence));
        let foreign = OccurrenceIdentity::new("foreign", 1, 0, 16_000);
        ledger.admit(
            &obs(ObservationProducer::Whisper, 0, foreign),
            "foreign words",
        );
        assert!(ledger.text_recovery_pending(&occurrence));
        ledger.admit(
            &obs(ObservationProducer::Whisper, 0, occ(0, 8_000)),
            "tail words",
        );
        assert!(ledger.text_recovery_pending(&occurrence));
        let repaired = ledger.admit(
            &obs(ObservationProducer::Whisper, 1, occurrence.clone()),
            "entire recovered utterance",
        );
        assert!(repaired.grants_mutation());
        assert!(!ledger.text_recovery_pending(&occurrence));
        ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
        let coverage = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 0);
        assert_eq!(coverage.coverage_ratio(), Some(1.0));
        assert!(ledger.record_seal_coverage(coverage));
        assert!(ledger.seal_terminal("s1", 1).is_ok());
        assert!(!ledger.require_text_recovery(&occurrence));
        assert!(!ledger.require_text_recovery(&occ(32_000, 48_000)));
    }

    #[test]
    fn text_recovery_debt_accepts_agreement_but_not_replayed_agreement() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        ledger.admit(
            &obs(ObservationProducer::Apple, 0, occurrence.clone()),
            "same words",
        );
        assert!(ledger.require_text_recovery(&occurrence));
        let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        assert!(matches!(
            ledger.admit(&whisper, "same words"),
            MutationReceipt::Preserve { .. }
        ));
        assert!(!ledger.text_recovery_pending(&occurrence));
        assert!(ledger.require_text_recovery(&occurrence));
        assert!(matches!(
            ledger.admit(&whisper, "same words"),
            MutationReceipt::Refuse { .. }
        ));
        assert!(ledger.text_recovery_pending(&occurrence));
        ledger.admit(
            &obs(ObservationProducer::Formatter, 1, occurrence.clone()),
            "polished words",
        );
        assert!(matches!(
            ledger.admit(
                &obs(ObservationProducer::Whisper, 2, occurrence.clone()),
                "polished words"
            ),
            MutationReceipt::Preserve { .. }
        ));
        assert!(ledger.text_recovery_pending(&occurrence));
        ledger.admit(
            &obs(ObservationProducer::ManualHuman, 3, occurrence.clone()),
            "",
        );
        assert!(ledger.text_recovery_pending(&occurrence));
        ledger.admit(
            &obs(ObservationProducer::ManualHuman, 4, occurrence.clone()),
            "human words",
        );
        assert!(!ledger.text_recovery_pending(&occurrence));
    }

    #[test]
    fn text_recovery_debt_cannot_be_covered_by_an_overlapping_label() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        assert!(ledger.require_text_recovery(&occurrence));
        assert!(
            ledger
                .admit(
                    &obs(ObservationProducer::Whisper, 0, occ(0, 32_000)),
                    "words spanning both ranges",
                )
                .grants_mutation()
        );
        let speech = measured_speech(
            "s1",
            1,
            32_000,
            vec![TailSampleRange {
                session: "s1".to_string(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 32_000,
            }],
        );
        let coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
        assert_eq!(coverage.coverage_ratio(), Some(0.5));
        assert_eq!(coverage.max_uncovered_samples, 16_000);
        assert_eq!(coverage.uncovered_speech_ranges[0].sample_start, 0);
        assert_eq!(coverage.uncovered_speech_ranges[0].sample_end, 16_000);
        assert!(ledger.text_recovery_pending(&occurrence));
    }

    #[test]
    fn text_recovery_debt_requires_new_same_lane_generation() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        ledger.admit(
            &obs(ObservationProducer::Whisper, 4, occurrence.clone()),
            "heard words",
        );
        assert!(ledger.require_text_recovery(&occurrence));
        let stale =
            ObservationIdentity::new(ObservationProducer::Whisper, 8, 3, occurrence.clone());
        assert!(matches!(
            ledger.admit(&stale, "heard words"),
            MutationReceipt::Preserve { .. }
        ));
        assert!(ledger.text_recovery_pending(&occurrence));
        assert!(matches!(
            ledger.admit(
                &obs(ObservationProducer::Whisper, 5, occurrence.clone()),
                "heard words",
            ),
            MutationReceipt::Preserve { .. }
        ));
        assert!(!ledger.text_recovery_pending(&occurrence));
    }

    #[test]
    fn empty_whisper_then_real_apple_preserves_returns_and_receipt_history() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        let serial = ledger.serial_of(&occurrence).unwrap().clone();
        let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        assert_eq!(
            ledger.admit(&whisper, " "),
            MutationReceipt::Refuse {
                occurrence: occurrence.clone(),
                reason: RefuseReason::EmptyLabel,
            }
        );
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::LabelMissing));
        assert_eq!(
            ledger.seal_terminal("s1", 1),
            Err(SealRefusal::LabelMissing)
        );
        let prior = ledger.frontier_of(&occurrence).unwrap().clone();
        let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
        assert!(ledger.schedule_late_apple_label(&apple, "real words"));
        assert!(
            prior
                .returned
                .is_subset(&ledger.frontier_of(&occurrence).unwrap().returned)
        );
        assert_eq!(
            ledger.admit(&apple, "real words"),
            MutationReceipt::Insert {
                occurrence: occurrence.clone(),
            }
        );
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let lexicon = obs(ObservationProducer::Lexicon, 0, occurrence.clone());
        assert!(matches!(
            ledger.admit(&lexicon, "real words"),
            MutationReceipt::Preserve { .. }
        ));
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        assert_eq!(seal.layer_trail_ordinals, vec![0, 1, 2]);
        assert_eq!(seal.serials, vec![serial]);
        assert_eq!(seal.coverage, occurrence);
        assert_eq!(ledger.text_of(&occurrence), Some("real words"));
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn duplicate_empty_returns_do_not_seal_or_erase_evidence() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        assert!(matches!(
            ledger.admit(&whisper, ""),
            MutationReceipt::Refuse {
                reason: RefuseReason::EmptyLabel,
                ..
            }
        ));
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        let frontier = ledger.frontier_of(&occurrence).unwrap().clone();
        assert!(matches!(
            ledger.admit(&whisper, ""),
            MutationReceipt::Refuse {
                reason: RefuseReason::BatchDuplicate,
                ..
            }
        ));
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        assert_eq!(ledger.frontier_of(&occurrence), Some(&frontier));
        assert_eq!(ledger.layer_trail().len(), 2);
        assert!(ledger.is_empty());
        assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::LabelMissing));
    }

    #[test]
    fn late_apple_requires_exact_qualification_and_never_relaunches_observers() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        let frontier = ledger.frontier_of(&occurrence).unwrap().clone();
        for foreign in [
            OccurrenceIdentity::new("other", 1, 0, 16_000),
            OccurrenceIdentity::new("s1", 2, 0, 16_000),
            occ(1, 16_000),
            occ(0, 16_001),
        ] {
            assert!(
                !ledger.schedule_late_apple_label(
                    &obs(ObservationProducer::Apple, 0, foreign),
                    "words"
                )
            );
        }
        let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
        assert!(!ledger.schedule_late_apple_label(&apple, " "));
        assert!(!ledger.schedule_late_apple_label(
            &obs(ObservationProducer::Whisper, 1, occurrence.clone()),
            "words"
        ));
        assert_eq!(ledger.frontier_of(&occurrence), Some(&frontier));
        assert!(ledger.schedule_late_apple_label(&apple, "words"));
        assert!(!ledger.schedule_late_apple_label(&apple, "words"));
        assert!(!ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));
        assert!(
            ledger
                .frontier_of(&occurrence)
                .unwrap()
                .returned
                .contains(&ObservationProducer::Whisper)
        );
    }

    #[test]
    fn successful_label_seal_refuses_late_machine_but_preserves_human_provenance() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        assert!(ledger.admit(&whisper, "successful words").grants_mutation());
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
        assert!(!ledger.schedule_late_apple_label(&apple, "late words"));
        assert!(matches!(
            ledger.admit(&apple, "late words"),
            MutationReceipt::Refuse {
                reason: RefuseReason::SealedReplay,
                ..
            }
        ));
        assert_eq!(ledger.text_of(&occurrence), Some("successful words"));
        let human = obs(ObservationProducer::ManualHuman, 1, occurrence.clone());
        assert!(ledger.admit(&human, "human correction").grants_mutation());
        assert_eq!(ledger.manual_edits().len(), 1);
        assert_eq!(ledger.manual_edits()[0].supersedes_seal, seal.receipt_id);
        assert_eq!(ledger.seal(&occurrence), Ok(&seal));
        assert_eq!(ledger.post_seal_decisions(&occurrence).len(), 2);
    }

    /// Only concrete observer launches extend a frontier. A formatter setting
    /// without a launched proposal job is absent, and a repeated return cannot
    /// report a second close transition.
    #[test]
    fn frontier_contains_launched_observers_and_closes_once() {
        let occurrence = occ(0, 16_000);
        let mut ledger = AcousticLedger::new();
        ledger.schedule_frontier(
            occurrence.clone(),
            [ObservationProducer::Apple, ObservationProducer::Lexicon],
        );
        assert!(ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));

        let frontier = ledger.frontier_of(&occurrence).expect("frontier");
        assert_eq!(
            frontier.open_producers(),
            vec![
                ObservationProducer::Apple,
                ObservationProducer::Whisper,
                ObservationProducer::Lexicon,
            ]
        );
        assert!(
            !frontier
                .open_producers()
                .contains(&ObservationProducer::Formatter),
            "configured but unlaunched Formatter is not scheduled"
        );

        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        assert!(
            !ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper),
            "repeated completion is not a second close transition"
        );
        assert!(
            !ledger.schedule_observer(occurrence, ObservationProducer::Formatter),
            "a closed frontier cannot be reopened by a late configured observer"
        );
    }

    #[test]
    fn duplicate_schedule_observer_is_not_a_new_launch() {
        let occurrence = occ(0, 16_000);
        let mut ledger = AcousticLedger::new();
        ledger.schedule_frontier(
            occurrence.clone(),
            [ObservationProducer::Apple, ObservationProducer::Lexicon],
        );

        assert!(ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));
        assert!(
            !ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper),
            "a duplicate before return is not a concrete new launch"
        );
        assert!(
            !ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper),
            "Apple and Lexicon still keep the frontier open"
        );
        assert!(
            !ledger.schedule_observer(occurrence, ObservationProducer::Whisper),
            "a returned producer cannot be relaunched inside the same frontier"
        );
    }

    /// The conservation law itself, on the shape that motivated it: five
    /// distinct acoustic occurrences of one name, byte-identical text.
    #[test]
    fn five_acoustic_occurrences_of_one_word_yield_five_inserts() {
        let mut ledger = AcousticLedger::new();
        let items: Vec<(ObservationIdentity, String)> = (0..5)
            .map(|i| {
                let start = i * 16_000;
                (
                    obs(ObservationProducer::Whisper, i, occ(start, start + 8_000)),
                    "Iwo".to_string(),
                )
            })
            .collect();
        let (receipts, tally) = ledger.admit_batch(&items);
        assert_eq!(receipts.iter().filter(|r| r.is_insert()).count(), 5);
        assert_eq!(tally.observations_in, tally.receipts_out);
        assert_eq!(tally.occurrences_held, 5);
    }

    /// A cumulative producer restating the same five occurrences must not grow
    /// the count. Conservation is counted over occurrences, not hypotheses.
    #[test]
    fn restating_the_same_occurrences_does_not_grow_the_count() {
        let mut ledger = AcousticLedger::new();
        let first: Vec<(ObservationIdentity, String)> = (0..5)
            .map(|i| {
                let start = i * 16_000;
                (
                    obs(ObservationProducer::Apple, i, occ(start, start + 8_000)),
                    "Iwo".to_string(),
                )
            })
            .collect();
        ledger.admit_batch(&first);
        let (_, tally) = ledger.admit_batch(&first);
        assert_eq!(tally.occurrences_held, 5, "a replay is not a sixth event");
    }

    /// D1 — Apple hears "Ivo", Whisper hears "Iwo" on the SAME range. That is a
    /// correction of one occurrence, not a replay to be refused: the higher
    /// authority rewrites the text in place and the count is unchanged.
    #[test]
    fn d1_higher_authority_on_the_same_range_corrects_rather_than_refuses() {
        let mut ledger = AcousticLedger::new();
        let range = occ(0, 16_000);
        let apple = ledger.admit(&obs(ObservationProducer::Apple, 0, range.clone()), "Ivo");
        assert!(apple.is_insert());

        let whisper = ledger.admit(&obs(ObservationProducer::Whisper, 0, range.clone()), "Iwo");
        assert_eq!(
            whisper,
            MutationReceipt::Correct {
                occurrence: range.clone(),
                from: ObservationProducer::Apple,
                to: ObservationProducer::Whisper,
            }
        );
        assert_eq!(ledger.text_of(&range), Some("Iwo"));
        assert_eq!(ledger.len(), 1, "a correction may not mint a second event");
    }

    /// D1b — human evidence outranks every model prior, so `Iwo` is never
    /// normalised back to `Ivo` by a lower layer.
    #[test]
    fn d1b_manual_evidence_is_not_overridden_by_a_model_prior() {
        let mut ledger = AcousticLedger::new();
        let range = occ(0, 16_000);
        ledger.admit(
            &obs(ObservationProducer::ManualHuman, 0, range.clone()),
            "Iwo",
        );
        let downgrade = ledger.admit(&obs(ObservationProducer::Whisper, 9, range.clone()), "Ivo");
        assert_eq!(
            downgrade,
            MutationReceipt::Refuse {
                occurrence: range.clone(),
                reason: RefuseReason::SealedReplay,
            }
        );
        assert_eq!(ledger.text_of(&range), Some("Iwo"));
    }

    /// D2 — a range that shares audio with a committed one but carries no word
    /// pins may not clip text. It stays visible and gets no mutation right.
    #[test]
    fn d2_overlap_without_word_pins_does_not_clip_text() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(&obs(ObservationProducer::Apple, 0, occ(0, 16_000)), "Iwo");
        let overlapping = ledger.admit(
            &obs(ObservationProducer::Whisper, 0, occ(8_000, 24_000)),
            "Iwo later",
        );
        assert_eq!(
            overlapping,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::OverlapWithoutWordPins,
            },
            "clipping a range without pins invents a sub-range the payload does not carry"
        );
        assert!(!overlapping.grants_mutation());
        assert_eq!(ledger.len(), 1);
    }

    /// D3 — one zero-width prior must not poison the ledger. It never enters
    /// the map, so a later well-formed disjoint span still inserts cleanly.
    #[test]
    fn d3_a_zero_width_prior_does_not_poison_the_ledger() {
        let mut ledger = AcousticLedger::new();
        let degenerate = ledger.admit(
            &obs(ObservationProducer::Apple, 0, occ(12_000, 12_000)),
            "hm",
        );
        assert_eq!(
            degenerate,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::ZeroWidth,
            }
        );
        assert!(ledger.is_empty(), "an unanchored prior holds no occurrence");

        let good = ledger.admit(
            &obs(ObservationProducer::Whisper, 0, occ(32_000, 48_000)),
            "Iwo",
        );
        assert!(
            good.is_insert(),
            "a good disjoint span must not inherit the degenerate prior's verdict"
        );
    }

    /// D4 — capture epochs are partitioned. A new epoch restarts the sample
    /// clock, so its ranges are not comparable to the old epoch's and must not
    /// be refused wholesale.
    #[test]
    fn d4_a_new_capture_epoch_is_partitioned_not_refused() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(
            &obs(
                ObservationProducer::Apple,
                0,
                OccurrenceIdentity::new("s1", 1, 0, 16_000),
            ),
            "Iwo",
        );
        let next_epoch = ledger.admit(
            &obs(
                ObservationProducer::Apple,
                0,
                OccurrenceIdentity::new("s1", 2, 0, 16_000),
            ),
            "Iwo",
        );
        assert!(
            next_epoch.is_insert(),
            "same sample numbers in a new epoch are a different moment in the world"
        );
        assert_eq!(ledger.len(), 2);

        let other_session = ledger.admit(
            &obs(
                ObservationProducer::Apple,
                0,
                OccurrenceIdentity::new("s2", 1, 0, 16_000),
            ),
            "Iwo",
        );
        assert!(other_session.is_insert());
        assert_eq!(ledger.len(), 3);
    }

    /// D5 — text with no anchor stays visible. It is not deleted and it is not
    /// granted the right to delete.
    #[test]
    fn d5_unanchored_text_stays_visible_without_mutation_rights() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(&obs(ObservationProducer::Whisper, 0, occ(0, 16_000)), "Iwo");
        let floating = ledger.admit(
            &obs(ObservationProducer::Apple, 0, occ(99_000, 99_000)),
            "coś jeszcze",
        );
        assert_eq!(
            floating,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::ZeroWidth,
            }
        );
        assert!(!floating.grants_mutation());
        assert_eq!(
            ledger.text_of(&occ(0, 16_000)),
            Some("Iwo"),
            "unanchored text may not overwrite an anchored occurrence"
        );
    }

    /// D6 — the same observation twice in one batch is answered twice, but only
    /// counted once. The receipt count still equals the observation count.
    #[test]
    fn d6_a_duplicate_observation_in_one_batch_is_answered_but_not_double_counted() {
        let mut ledger = AcousticLedger::new();
        let observation = obs(ObservationProducer::Whisper, 0, occ(0, 16_000));
        let items = vec![
            (observation.clone(), "Iwo".to_string()),
            (observation.clone(), "Iwo".to_string()),
        ];
        let (receipts, tally) = ledger.admit_batch(&items);
        assert_eq!(receipts.len(), 2, "one receipt per observation, always");
        assert!(receipts[0].is_insert());
        assert_eq!(
            receipts[1],
            MutationReceipt::Refuse {
                occurrence: occ(0, 16_000),
                reason: RefuseReason::BatchDuplicate,
            }
        );
        assert_eq!(tally.occurrences_held, 1);
        assert_eq!(tally.observations_in, tally.receipts_out);
    }

    /// D7 — utterance-grain evidence is all-or-nothing. The ledger never
    /// answers with a sub-range the observation did not declare.
    #[test]
    fn d7_utterance_grain_is_never_clipped_into_an_invented_sub_range() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(
            &obs(ObservationProducer::Apple, 0, occ(0, 40_000)),
            "zdanie",
        );
        let straddling = ledger.admit(
            &obs(ObservationProducer::Whisper, 0, occ(20_000, 60_000)),
            "zdanie dalej",
        );
        match &straddling {
            MutationReceipt::KeepVisibleUnanchored { reason } => {
                assert_eq!(*reason, NoAuthorityReason::OverlapWithoutWordPins);
            }
            other => panic!("utterance grain must not be clipped, got {other:?}"),
        }
        let held: Vec<&OccurrenceIdentity> = ledger.occurrences().collect();
        assert_eq!(held, vec![&occ(0, 40_000)], "no invented sub-range entered");
    }

    /// Same-lane revision: Apple correcting its own final on its own range at a
    /// newer generation is a correction, not a replay.
    #[test]
    fn same_producer_at_a_newer_generation_may_revise_its_own_span() {
        let mut ledger = AcousticLedger::new();
        let range = occ(0, 16_000);
        ledger.admit(&obs(ObservationProducer::Apple, 0, range.clone()), "szuty");
        let revised = ledger.admit(&obs(ObservationProducer::Apple, 1, range.clone()), "skróty");
        assert!(revised.is_correct());
        assert_eq!(ledger.text_of(&range), Some("skróty"));
        assert_eq!(ledger.len(), 1);
    }

    /// Arriving later does not make a hypothesis a new physical event: `order`
    /// lives on the observation, not on the occurrence.
    #[test]
    fn order_lives_on_the_observation_not_on_the_occurrence() {
        let early = obs(ObservationProducer::Whisper, 0, occ(0, 16_000));
        let late = obs(ObservationProducer::Whisper, 42, occ(0, 16_000));
        assert_eq!(
            early.occurrence, late.occurrence,
            "generation must not be part of physical identity"
        );
        assert_ne!(
            early, late,
            "generation must be part of observation identity"
        );
    }

    /// A cumulative final whose window is disjoint from every committed
    /// occurrence is new audio; the text matcher is not consulted.
    #[test]
    fn a_disjoint_cumulative_window_is_wholly_novel() {
        let committed = vec![(occ(0, 16_000), 4)];
        let decision = CumulativeFinalAdmission::decide(&occ(32_000, 48_000), &committed);
        assert_eq!(decision, CumulativeFinalAdmission::WhollyNovel);
        assert_eq!(
            decision.clamp_known_prefix(5),
            0,
            "no committed occurrence overlaps, so nothing is already known"
        );
    }

    /// The clamp is the conservation guard: a restatement may not claim more
    /// occurrences than the overlapped canvas actually holds.
    #[test]
    fn alignment_may_not_claim_more_words_than_the_canvas_holds() {
        let committed = vec![(occ(0, 64_000), 4)];
        let decision = CumulativeFinalAdmission::decide(&occ(48_000, 80_000), &committed);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::AlignInside {
                canvas_words_under_authority: 4,
            }
        );
        assert_eq!(
            decision.clamp_known_prefix(5),
            4,
            "the fifth occurrence has no committed counterpart and must survive"
        );
        assert_eq!(
            decision.clamp_known_prefix(3),
            3,
            "shorter matches pass through"
        );
    }

    /// With nothing anchored to compare against, the legacy text lane keeps its
    /// behaviour — it is demoted, not replaced by a fabricated verdict.
    #[test]
    fn without_an_anchor_the_text_matcher_keeps_its_legacy_answer() {
        let decision = CumulativeFinalAdmission::decide(&occ(0, 16_000), &[]);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            }
        );
        assert_eq!(decision.clamp_known_prefix(5), 5);
    }

    /// A zero-width window carries no authority in either direction.
    #[test]
    fn a_zero_width_cumulative_window_has_no_authority() {
        let committed = vec![(occ(0, 16_000), 4)];
        let decision = CumulativeFinalAdmission::decide(&occ(16_000, 16_000), &committed);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::NoAnchor {
                reason: NoAuthorityReason::ZeroWidth,
            }
        );
    }

    /// Unanchored committed spans are absent evidence, never objections.
    #[test]
    fn unanchored_committed_spans_do_not_veto_a_window() {
        let committed = vec![(occ(5_000, 5_000), 3), (occ(32_000, 48_000), 2)];
        let decision = CumulativeFinalAdmission::decide(&occ(40_000, 56_000), &committed);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::AlignInside {
                canvas_words_under_authority: 2,
            },
            "only the anchored overlap contributes authority"
        );
    }

    /// A cumulative producer under-declares its window, so the finality bar —
    /// not the window — bounds how much it may claim to restate.
    #[test]
    fn a_cumulative_restatement_is_bounded_by_the_open_occurrences() {
        let open = vec![
            (occ(0, 16_000), 1),
            (occ(16_000, 32_000), 1),
            (occ(32_000, 48_000), 1),
            (occ(48_000, 64_000), 1),
        ];
        let decision = CumulativeFinalAdmission::for_cumulative_restatement(&open);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::AlignInside {
                canvas_words_under_authority: 4,
            }
        );
        assert_eq!(
            decision.clamp_known_prefix(5),
            4,
            "four open occurrences cannot account for a fifth token"
        );
    }

    /// With nothing open, a cumulative final restates nothing and the legacy
    /// text lane is left exactly as it was.
    #[test]
    fn a_cumulative_restatement_with_nothing_open_has_no_anchor() {
        let decision = CumulativeFinalAdmission::for_cumulative_restatement(&[]);
        assert_eq!(
            decision,
            CumulativeFinalAdmission::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            }
        );
        assert_eq!(decision.clamp_known_prefix(3), 3);
    }

    /// The authorised slice is what keeps a match in an unrelated part of the
    /// transcript out of the matcher's reach.
    #[test]
    fn the_authorized_canvas_hides_text_outside_the_authorized_span() {
        let canvas = vec![
            "zupelnie", "co", "innego", "iwo", "iwo", "iwo", "iwo", "iwo", "dalszy", "ciag",
        ];
        let decision = CumulativeFinalAdmission::AlignInside {
            canvas_words_under_authority: 2,
        };
        assert_eq!(decision.authorized_canvas(&canvas), &["dalszy", "ciag"]);

        assert!(
            CumulativeFinalAdmission::WhollyNovel
                .authorized_canvas(&canvas)
                .is_empty(),
            "new audio has no canvas to align against"
        );
        assert_eq!(
            CumulativeFinalAdmission::NoAnchor {
                reason: NoAuthorityReason::NoRange
            }
            .authorized_canvas(&canvas),
            canvas.as_slice(),
            "with no anchor the legacy matcher keeps the whole canvas"
        );
    }

    /// A `TailSampleRange` and the occurrence built from it are the same key.
    #[test]
    fn tail_sample_range_maps_onto_occurrence_identity() {
        let range = TailSampleRange {
            session: "s1".to_string(),
            capture_epoch: 1,
            sample_start: 16_000,
            sample_end: 32_000,
        };
        assert_eq!(OccurrenceIdentity::from(&range), occ(16_000, 32_000));
    }

    /// Recorded W5-A fixture from session b2b3b95e. The replay harness cannot
    /// inject historical Apple callbacks, so this pins their exact admitted
    /// occurrence receipts and proves gap recovery at the ledger boundary.
    #[test]
    fn recorded_b2b3b95e_gaps_refuse_terminal_truth_until_pcm_occurrences_land() {
        const SESSION: &str = "b2b3b95e-4ddc-4845-a5ce-149b21eec166";
        const EPOCH: u64 = 1;
        const RATE: u64 = 88_200;
        const THRESHOLD: u64 = RATE * 250 / 1_000;

        fn admit_receipted(
            ledger: &mut AcousticLedger,
            range: OccurrenceIdentity,
            producer: ObservationProducer,
            request: u64,
            label: &str,
        ) {
            let calibration = EnergyCalibration::new("recorded-calibration", 1.0, 1);
            let evidence = AcousticEvidence {
                occurrence: range.clone(),
                duration_ms: range.sample_len() as f64 * 1_000.0 / RATE as f64,
                energy_integral: range.sample_len() as f64,
                mean_rms_dbfs: -35.0,
                peak_dbfs: -19.0,
                vad_open_sample: Some(range.sample_start),
                vad_close_sample: Some(range.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            ledger.schedule_frontier(range.clone(), [producer]);
            let observation = ObservationIdentity::new(producer, request, 0, range.clone());
            assert!(ledger.admit(&observation, label).grants_mutation());
            assert!(ledger.note_frontier_return(&range, producer));
            ledger.seal(&range).expect("recorded occurrence seals");
        }

        let mut ledger = AcousticLedger::new();
        admit_receipted(
            &mut ledger,
            OccurrenceIdentity::new(SESSION, EPOCH, 304_819, 869_376),
            ObservationProducer::Apple,
            1,
            "Kurde powiem ci że dość fajne mechanizmy nam",
        );
        admit_receipted(
            &mut ledger,
            OccurrenceIdentity::new(SESSION, EPOCH, 1_196_697, 1_558_528),
            ObservationProducer::Apple,
            2,
            "Dość śmiesznym i ciekawym linie",
        );
        let speech = measured_speech(
            SESSION,
            EPOCH,
            1_602_560,
            vec![TailSampleRange {
                session: SESSION.to_string(),
                capture_epoch: EPOCH,
                sample_start: 304_819,
                sample_end: 1_602_560,
            }],
        );

        let incomplete = ledger.assess_seal_coverage(SESSION, EPOCH, &speech, THRESHOLD);
        assert_eq!(incomplete.status, SealCoverageStatus::Incomplete);
        assert_eq!(incomplete.speech_samples, 1_297_741);
        assert_eq!(incomplete.covered_samples, 926_388);
        assert_eq!(incomplete.max_uncovered_samples, 327_321);
        assert_eq!(
            incomplete
                .uncovered_speech_ranges
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect::<Vec<_>>(),
            vec![(869_376, 1_196_697), (1_558_528, 1_602_560)]
        );
        assert!(ledger.record_seal_coverage(incomplete.clone()));
        assert_eq!(
            ledger.seal_terminal(SESSION, EPOCH),
            Err(SealRefusal::CoverageIncomplete),
            "incomplete measured speech must not become terminal truth"
        );

        admit_receipted(
            &mut ledger,
            OccurrenceIdentity::new(SESSION, EPOCH, 869_376, 1_196_697),
            ObservationProducer::Whisper,
            3,
            "wychodzą spontanicznie w tym",
        );
        admit_receipted(
            &mut ledger,
            OccurrenceIdentity::new(SESSION, EPOCH, 1_558_528, 1_602_560),
            ObservationProducer::Whisper,
            4,
            "pipeline'ie. Nie sądzisz?",
        );

        let complete = ledger.assess_seal_coverage(SESSION, EPOCH, &speech, THRESHOLD);
        assert_eq!(complete.status, SealCoverageStatus::Complete);
        assert_eq!(complete.covered_samples, complete.speech_samples);
        assert!(complete.uncovered_speech_ranges.is_empty());
        let rendered = ledger.rendered_text();
        assert!(rendered.contains("wychodzą spontanicznie w tym"));
        assert!(rendered.ends_with("Nie sądzisz?"));
        assert_eq!(complete.coverage_ratio(), Some(1.0));
        assert_eq!(complete.observed_samples, Some(1_602_560));
        assert!(ledger.record_seal_coverage(complete));
        ledger
            .seal_terminal(SESSION, EPOCH)
            .expect("complete recorded coverage may become terminal truth");
    }

    /// Evidence the two acoustic observers mint. Tests use it directly so the
    /// ledger's own adjudication is exercised, not a bare range vector.
    fn measured_speech(
        session: &str,
        capture_epoch: u64,
        observed_samples: u64,
        ranges: Vec<TailSampleRange>,
    ) -> AcousticSpeechEvidence {
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(session, capture_epoch),
            "test_observer",
            AcousticAvailability::Observed { observed_samples },
            ranges,
        )
    }

    /// Measured silence is a verdict. Absence of measurement is not, and the
    /// two must not collapse into the same trivially complete receipt.
    #[test]
    fn measured_silence_seals_and_absent_measurement_refuses() {
        let ledger = AcousticLedger::new();
        let silence = ledger.assess_seal_coverage(
            "quiet",
            1,
            &measured_speech("quiet", 1, 16_000, Vec::new()),
            4_000,
        );
        assert_eq!(silence.status, SealCoverageStatus::Complete);
        assert_eq!(silence.speech_samples, 0);
        assert_eq!(silence.coverage_ratio(), Some(1.0));
        assert_eq!(silence.observed_samples, Some(16_000));

        for availability in [
            AcousticAvailability::NotObserved,
            AcousticAvailability::InvalidMeasurement { valid_samples: 0 },
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 8_000,
            },
            AcousticAvailability::Discontinuous {
                observed_samples: 8_000,
            },
        ] {
            let receipt = ledger.assess_seal_coverage(
                "quiet",
                1,
                &AcousticSpeechEvidence::unavailable(
                    CaptureEvidenceIdentity::new("quiet", 1),
                    "test_observer",
                    availability,
                ),
                4_000,
            );
            assert!(
                receipt.status.unavailable_reason().is_some(),
                "{availability:?} cannot certify a take"
            );
            assert!(!receipt.status.is_complete());
            assert_eq!(
                receipt.coverage_ratio(),
                None,
                "an unknown extent has no ratio; 1.0 would read as a perfect take"
            );
            assert_eq!(receipt.observed_samples, None);
            assert_eq!(receipt.status.as_str(), "unavailable");
        }
    }

    /// A discontinuous observer names `PartialObservation`, and so does an
    /// observer whose measured extent stops short of committed speech.
    #[test]
    fn unobserved_tails_and_foreign_evidence_refuse_terminal_truth() {
        let mut ledger = AcousticLedger::new();
        let discontinuous = ledger.assess_seal_coverage(
            "partial",
            1,
            &AcousticSpeechEvidence::unavailable(
                CaptureEvidenceIdentity::new("partial", 1),
                "test_observer",
                AcousticAvailability::Discontinuous {
                    observed_samples: 8_000,
                },
            ),
            4_000,
        );
        assert_eq!(
            discontinuous.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );

        // A speech span past the observed extent is an unobserved tail.
        let beyond = ledger.assess_seal_coverage(
            "partial",
            1,
            &measured_speech(
                "partial",
                1,
                8_000,
                vec![TailSampleRange {
                    session: "partial".into(),
                    capture_epoch: 1,
                    sample_start: 0,
                    sample_end: 16_000,
                }],
            ),
            4_000,
        );
        assert_eq!(
            beyond.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );

        // Evidence bound to another take cannot be authenticated by asking
        // about this one, and a foreign range inside otherwise-valid evidence
        // survives adjudication instead of being filtered into silence.
        let foreign_owner = ledger.assess_seal_coverage(
            "partial",
            1,
            &measured_speech("successor", 1, 16_000, Vec::new()),
            4_000,
        );
        assert_eq!(
            foreign_owner.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::IdentityMismatch)
        );
        let foreign_range = ledger.assess_seal_coverage(
            "partial",
            1,
            &measured_speech(
                "partial",
                1,
                16_000,
                vec![TailSampleRange {
                    session: "successor".into(),
                    capture_epoch: 1,
                    sample_start: 0,
                    sample_end: 16_000,
                }],
            ),
            4_000,
        );
        assert_eq!(
            foreign_range.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::IdentityMismatch)
        );

        assert!(ledger.record_seal_coverage(discontinuous));
        assert_eq!(
            ledger.seal_terminal("partial", 1),
            Err(SealRefusal::CoverageIncomplete),
            "a non-complete verdict blocks the terminal seal whatever its reason"
        );
    }

    /// A sealed occurrence, its exact committed label, and the left context the
    /// casing saw. That is the whole claim an incremental shaping makes — and
    /// the receipt states each part explicitly instead of implying it.
    fn sealed_for_shaping() -> (AcousticLedger, OccurrenceIdentity) {
        let occurrence = occ(0, 16_000);
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("incremental-shaping", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(0),
            vad_close_sample: Some(16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        let observation = obs(ObservationProducer::Apple, 0, occurrence.clone());
        assert!(ledger.admit(&observation, "jakieś słowa").grants_mutation());
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        ledger.seal(&occurrence).expect("closed occurrence seals");
        (ledger, occurrence)
    }

    #[test]
    fn an_incremental_shaping_binds_one_sealed_occurrence_and_its_seal() {
        let (mut ledger, occurrence) = sealed_for_shaping();
        let seal_receipt = ledger
            .seal_of(&occurrence)
            .expect("sealed")
            .receipt_id
            .clone();

        let receipt = ledger
            .record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "Poprzednie zdanie.",
                shaped_text: "Jakieś słowa.",
            })
            .expect("a sealed, committed, unchanged label shapes");

        assert_eq!(receipt.provenance, "light-plus");
        assert!(receipt.receipt_id.starts_with("light-plus-incremental-"));
        assert_eq!(receipt.occurrence, occurrence);
        assert_eq!(receipt.source_seal_receipt, seal_receipt);
        assert_eq!(receipt.source_label, "jakieś słowa");
        assert_eq!(receipt.shaped_text, "Jakieś słowa.");
        assert_eq!(receipt.source_revision, 4);
        assert_eq!(receipt.revision, 5);
        assert_eq!(
            receipt.left_context_sha256,
            format!("{:x}", Sha256::digest("Poprzednie zdanie.".as_bytes())),
            "the left neighbourhood is pinned so the shape can be reproduced"
        );
        assert_eq!(ledger.incremental_shapings().len(), 1);
        assert_eq!(ledger.incremental_shapings()[0], receipt);

        // The acoustic label itself is untouched — presentation, not words.
        assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
        assert!(ledger.manual_document_revisions().is_empty());
        assert!(ledger.manual_edits().is_empty());
    }

    #[test]
    fn an_identical_incremental_shaping_is_refused_instead_of_restated() {
        let (mut ledger, occurrence) = sealed_for_shaping();
        assert!(
            ledger
                .record_incremental_shaping(IncrementalShapingInput {
                    session_id: "s1",
                    source_revision: 4,
                    revision: 5,
                    occurrence: &occurrence,
                    source_label: "jakieś słowa",
                    left_context: "",
                    shaped_text: "Jakieś słowa.",
                })
                .is_ok()
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 5,
                revision: 6,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Jakieś słowa.",
            }),
            Err("incremental_shaping_unchanged"),
            "a replayed seal mints no second receipt"
        );
        assert_eq!(ledger.incremental_shapings().len(), 1);
    }

    #[test]
    fn incremental_shaping_refuses_unsealed_foreign_stale_and_empty_sources() {
        let (mut ledger, occurrence) = sealed_for_shaping();

        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "X.",
            }),
            Err("incremental_shaping_session_missing")
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "   \n ",
            }),
            Err("incremental_shaping_text_empty"),
            "an empty shape must never be allowed to erase words"
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 9,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Jakieś słowa.",
            }),
            Err("incremental_shaping_revision_nonconsecutive")
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "obca-sesja",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Jakieś słowa.",
            }),
            Err("incremental_shaping_session_mismatch")
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "zupełnie inne słowa",
                left_context: "",
                shaped_text: "Zupełnie inne słowa.",
            }),
            Err("incremental_shaping_source_label_stale"),
            "a shape must be derived from the label the ledger actually holds"
        );

        let unsealed = occ(16_000, 32_000);
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &unsealed,
                source_label: "cokolwiek",
                left_context: "",
                shaped_text: "Cokolwiek.",
            }),
            Err("incremental_shaping_occurrence_not_sealed"),
            "an open occurrence is never a sealed source"
        );
        assert!(ledger.incremental_shapings().is_empty());
    }

    /// The grouped input is a container, never a permission. Restating one
    /// honest claim with exactly one fact replaced must still meet the
    /// ledger's own checks, in the ledger's own order — and a refused claim
    /// must leave the ledger able to accept the untouched original afterwards.
    #[test]
    fn incremental_shaping_refuses_a_grouped_foreign_session_and_a_grouped_stale_label() {
        let (mut ledger, occurrence) = sealed_for_shaping();
        let honest = IncrementalShapingInput {
            session_id: "s1",
            source_revision: 11,
            revision: 12,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "Zdanie wcześniej.",
            shaped_text: "Jakieś słowa.",
        };

        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "obca-sesja",
                ..honest
            }),
            Err("incremental_shaping_session_mismatch"),
            "one field of the group may not carry a foreign session past the check"
        );
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                source_label: "zupełnie inne słowa",
                shaped_text: "Zupełnie inne słowa.",
                ..honest
            }),
            Err("incremental_shaping_source_label_stale"),
            "a shape grouped with a label the ledger does not hold is still stale"
        );
        assert!(
            ledger.incremental_shapings().is_empty(),
            "a refused grouped input mints nothing"
        );

        // Those were verdicts on the claim, not damage to the ledger: the
        // untouched original must still be admissible after both refusals.
        assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
        let receipt = ledger
            .record_incremental_shaping(honest)
            .expect("the untouched claim is still admissible after two refusals");
        assert_eq!(receipt.source_revision, 11);
        assert_eq!(receipt.revision, 12);
        assert_eq!(ledger.incremental_shapings().len(), 1);
    }

    /// Grouping the facts must not move a single byte of the receipt. The
    /// identifier encodes provenance, session, sample geometry, both revisions
    /// and the ordinal, so asserting it literally catches any silent reshuffle
    /// of the facts on their way into the ledger.
    #[test]
    fn incremental_shaping_from_a_grouped_input_mints_the_same_receipt_facts() {
        let (mut ledger, occurrence) = sealed_for_shaping();
        let left_context = "Zdanie wcześniej.";
        let shaped =
            crate::pipeline::light_plus::apply_with_left_context(left_context, "jakieś słowa");
        assert_eq!(
            shaped, "Jakieś słowa.",
            "the deterministic shaper still owns the bytes the ledger re-derives"
        );
        let seal_receipt = ledger
            .seal_of(&occurrence)
            .expect("sealed")
            .receipt_id
            .clone();

        let receipt = ledger
            .record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context,
                shaped_text: &shaped,
            })
            .expect("a grouped, sealed, deterministic claim shapes");

        assert_eq!(
            receipt.receipt_id, "light-plus-incremental-s1-0-16000-4-5-0",
            "provenance, session, sample span, both revisions and the ordinal              keep their exact places in the identifier"
        );
        assert_eq!(receipt.provenance, "light-plus");
        assert_eq!(receipt.session_id, "s1");
        assert_eq!(receipt.source_revision, 4);
        assert_eq!(receipt.revision, 5);
        assert_eq!(receipt.occurrence, occurrence);
        assert_eq!(receipt.source_seal_receipt, seal_receipt);
        assert_eq!(receipt.source_label, "jakieś słowa");
        assert_eq!(receipt.left_context, left_context);
        assert_eq!(
            receipt.left_context_sha256,
            format!("{:x}", Sha256::digest(left_context.as_bytes()))
        );
        assert_eq!(receipt.shaped_text, "Jakieś słowa.");

        // The acoustic label stays untouched: this is presentation, not words.
        assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
        assert_eq!(ledger.incremental_shapings().len(), 1);
        assert_eq!(ledger.incremental_shapings()[0], receipt);
    }

    #[test]
    fn occurrence_and_single_terminal_have_distinct_authenticated_scope() {
        let (mut ledger, occurrence) = sealed_for_shaping();
        let ordinary = ledger.seal_of(&occurrence).unwrap().clone();
        let terminal = ledger.seal_terminal("s1", 1).unwrap();
        assert!(ordinary.is_occurrence_seal());
        assert!(!terminal.is_occurrence_seal());
        assert_eq!(terminal.sealed_occurrences, vec![occurrence]);
        assert_ne!(ordinary.receipt_id, terminal.receipt_id);
        assert!(ledger.authenticates_seal(&ordinary));
        assert!(ledger.authenticates_seal(&terminal));
        assert_eq!(ledger.seal_terminal("s1", 1).unwrap(), terminal);
        assert_eq!(ledger.terminal_seals.len(), 1);
        let mut forged = ordinary;
        forged.scope = LedgerSealScope::Terminal;
        assert!(!ledger.authenticates_seal(&forged));
    }

    #[test]
    fn qualified_committed_open_source_and_non_deterministic_shape_are_refused() {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        let observation = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        assert!(ledger.admit(&observation, "jakieś słowa").grants_mutation());
        assert!(ledger.is_qualified(&occurrence));
        assert!(ledger.text_of(&occurrence).is_some());
        assert!(!ledger.is_sealed(&occurrence));
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Jakieś słowa.",
            }),
            Err("incremental_shaping_occurrence_not_sealed")
        );
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        ledger.seal(&occurrence).unwrap();
        assert_eq!(
            ledger.record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Unrelated bytes.",
            }),
            Err("incremental_shaping_not_deterministic")
        );
        assert!(ledger.incremental_shapings().is_empty());
    }
}
