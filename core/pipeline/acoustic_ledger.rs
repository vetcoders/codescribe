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
use std::time::{SystemTime, UNIX_EPOCH};

fn unix_epoch_millis() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

use sha2::{Digest, Sha256};

use crate::audio::capture_receipt::{
    AcousticAvailability, AcousticSpeechEvidence, CaptureEnergyOwner,
};
use crate::quality::engine_contract::is_clock_lie;
use crate::stt::tail_provider::TailSampleRange;

#[path = "acoustic_ledger/slot_ops.rs"]
mod slot_ops;
#[path = "acoustic_ledger/speech_anchor.rs"]
mod speech_anchor;
pub use speech_anchor::WordAnchorIdentity;
#[cfg(test)]
#[path = "acoustic_ledger/forward_publication_tests.rs"]
mod forward_publication_tests;
#[path = "acoustic_ledger/word_adjudication.rs"]
pub mod word_adjudication;
#[cfg(test)]
#[path = "acoustic_ledger/word_adjudication_tests.rs"]
pub(crate) mod word_adjudication_tests;
#[cfg(test)]
#[path = "acoustic_ledger/word_identity_energy_tests.rs"]
mod word_identity_energy_tests;
pub use slot_ops::{
    DictionarySlotRule, GroupSpeechCoverageReceipt, SlotOperationKind, SlotOperationReceipt,
    SlotOperationRefusal, SlotTarget, SpeechPinCoverage,
};
#[path = "acoustic_ledger/word_verdict.rs"]
pub mod word_verdict;
use word_verdict::WordDeletionReceipt;
#[path = "acoustic_ledger/recovery_diagnostics.rs"]
mod recovery_diagnostics;
#[path = "acoustic_ledger/witness.rs"]
pub mod witness;
pub use recovery_diagnostics::{
    CoverageDiagnostics, RecoveryAlternativeDiagnostic, RecoveryDiagnostic, RecoveryPinDiagnostic,
};
#[path = "acoustic_ledger/anchors.rs"]
mod anchors;
#[path = "acoustic_ledger/fence.rs"]
mod fence;
#[path = "acoustic_ledger/reducer.rs"]
mod reducer;
#[path = "acoustic_ledger/replay.rs"]
mod replay;
pub use anchors::{
    ACOUSTIC_SERIAL_VERSION, AcousticEvidence, AcousticSerial, AdmissionReceipt, AdmissionRefusal,
    EnergyCalibration, EvidenceRefusal, OccurrenceComposition, OccurrenceDerivation,
    WordEvidenceReceipt,
};
pub use fence::{
    CumulativeFinalAdmission, LedgerSealReceipt, LedgerSealScope, ObservationFrontier, SealRefusal,
    TerminalFinality, TerminalFinalityRefusal, TerminalFinalityRefusalReason,
};
pub use replay::{
    AcousticEvidenceGap, ConsultationPresentationInput, ConsultationPresentationMember,
    ConsultationPresentationReceipt, DocumentRevisionProvenance, IncrementalShapingInput,
    IncrementalShapingReceipt, LayerDecisionReceipt, ManualDocumentCaptureReceipt,
    ManualDocumentRevisionReceipt, ManualEditReceipt, SealCoverageReceipt, SealCoverageStatus,
    TranscriptComparisonReceipt,
};

/// A physical acoustic occurrence: a PCM range in one capture epoch.
///
/// This is the primary key of the transcript. It carries no text and no
/// ordering on purpose — see the module docs for why `order` is not here.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
///
/// `Ord` follows declaration order. `CloudLive` sits between Apple and Whisper
/// so a heard cloud word can replace Apple and still yield to Whisper.
/// Nothing persists the discriminant; receipts and the bus use [`Self::as_str`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ObservationProducer {
    /// L0 — Apple Speech live lane.
    Apple,
    /// Live websocket finals, committed at Silero closes.
    CloudLive,
    /// Independent DictationTranscriber witness over recorder-owned PCM.
    /// Its measured ranges may contest an existing label through word
    /// adjudication; its omission of a label grants no deletion authority.
    AppleDictation,
    /// Whisper tail retranscription.
    Whisper,
    /// Lexicon and Light+ cleanup.
    Lexicon,
    /// Responses formatter.
    Formatter,
    /// Human evidence. Fixes spelling for matching spans and is never
    /// overridden by a model prior.
    ManualHuman,
}

impl ObservationProducer {
    /// Rank within an authorized lane on the *same* occurrence. Formatter's
    /// rank describes derived text; it never authorizes acoustic Raw admission.
    pub fn authority_rank(self) -> u8 {
        match self {
            Self::Apple => 0,
            Self::CloudLive => 1,
            Self::AppleDictation => 2,
            Self::Whisper => 3,
            Self::Lexicon => 4,
            Self::Formatter => 5,
            Self::ManualHuman => 6,
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
            Self::CloudLive => "cloud_live",
            Self::AppleDictation => "apple_dictation",
            Self::Whisper => "whisper",
            Self::Lexicon | Self::Formatter => "retained_text",
            Self::ManualHuman => "manual_human",
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::CloudLive => "cloud_live",
            Self::AppleDictation => "apple_dictation",
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NoAuthorityReason {
    /// The range names no audio (zero-width or reversed).
    ZeroWidth,
    /// The producer supplied no range at all.
    NoRange,
    /// The range shares audio with a committed occurrence but the payload
    /// carries no word pins, so no token can be attributed to the shared part.
    OverlapWithoutWordPins,
    /// The pin is an exclusive tail, but the occurrence's whole span is not
    /// proven, so the text stays visible and does not replace the span.
    ExclusiveTailAwaitingWholeSpan,
    /// A re-close supplied a word after its immutable owner sealed.
    LateWhisperWordSealedOwner,
    /// A cloud-live final supplied a word after its immutable owner sealed.
    LateCloudLiveWordSealedOwner,
    /// Apple supplied a word after its immutable owner sealed.
    LateAppleWordSealedOwner,
    /// A late Apple word is not in the current slice; retain its own word pin.
    LateAppleWordNotCurrent,
    /// An authenticated observer measured this capture and reported no speech
    /// overlapping the claimed range; a decoder's word on measured silence is
    /// a phantom: kept visible as evidence, granted no document.
    NoMeasuredSpeech,
}

impl NoAuthorityReason {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LateWhisperWordSealedOwner => "late_whisper_word_sealed_owner",
            Self::LateCloudLiveWordSealedOwner => "late_cloud_live_word_sealed_owner",
            Self::LateAppleWordSealedOwner => "late_apple_word_sealed_owner",
            Self::LateAppleWordNotCurrent => "late_apple_word_not_current",
            Self::ZeroWidth => "zero_width",
            Self::NoRange => "no_range",
            Self::OverlapWithoutWordPins => "overlap_without_word_pins",
            Self::ExclusiveTailAwaitingWholeSpan => "exclusive_tail_awaiting_whole_span",
            Self::NoMeasuredSpeech => "no_measured_speech",
        }
    }
}

/// What a word's own span holds when Silero heard no speech near it.
///
/// Measured on the capture clock, never a named sound: noise, breath and a
/// cough are all `SoundWithoutSpeech`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceUnderSpan {
    /// Every hop is exact digital zero.
    DigitalZero,
    /// Sound is present, all of it under the calibrated existence floor.
    BelowFloor,
    /// A hop reaches the existence floor and Silero still heard no speech.
    SoundWithoutSpeech,
    /// The energy of the span was not measured, or no floor is calibrated.
    EnergyUnmeasured,
}

impl SilenceUnderSpan {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DigitalZero => "digital_zero",
            Self::BelowFloor => "below_floor",
            Self::SoundWithoutSpeech => "sound_without_speech",
            Self::EnergyUnmeasured => "energy_unmeasured",
        }
    }
}

/// Why an observation was refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RefuseReason {
    /// The occurrence carries a seal and the hypothesis is automatic.
    /// Historical rows also used this name for unsealed rank/slot refusals.
    SealedReplay,
    /// Word evidence is retained for recovery but cannot mutate its targets.
    SlotAdmissionRejected,
    /// An unsealed occurrence is held by equal-or-higher/newer authority.
    AuthorityConflict,
    /// The exact same observation identity was already answered in this batch.
    BatchDuplicate,
    /// A machine observation returned no lexical evidence.
    EmptyLabel,
    /// The range is already covered by an admitted identity. The overlap
    /// resolver names the range; it does not compare the two strings.
    ReplayedRangeIdentity,
    /// A pin intersects the occurrence and is not an exclusive tail, so the
    /// whole span stays as it is.
    IntersectingPinNotExclusive,
    /// An Apple slot was replaced by heard Whisper words.
    ReplacedByWhisper,
    /// An Apple or lexicon slot was replaced by a heard cloud-live word.
    ReplacedByCloudLive,
    /// A clock-lie span kept its own text and was asked to replace a neighbour.
    ClockLie,
    /// Stop asked for this uncovered PCM and no wholly contained segment was
    /// admitted. The range stays speech the document does not own.
    UnrecoveredSpeech,
    /// The pin's own PCM range was measured, and no hop inside it was voiced.
    /// Mean loudness of a wider span is not this fact.
    NoVoicedHopInPin,
    /// This word has no measured Silero speech in its own sample span.
    SpanWithoutOwnSpeech,
    /// Historical receipt: a word refused for the length of its decoder pin.
    /// Pin timing is not an acoustic boundary; no current decision uses it.
    SpanExceedsSyllableCeiling,
    /// This Whisper word repeats the previous identical word, and the span
    /// was measured. It does not contain its own voiced segment: no energy
    /// valley and no Silero dip separates it from that previous word, or the
    /// span is shorter than one syllable of a repeat.
    IdenticalRepeatWithoutOwnVoicedSegment,
    /// A previous positive no-speech receipt covers the entire candidate pin.
    ConfirmedNoSpeech,
    /// Silero observed the word's span and its surroundings and heard no
    /// speech within the near radius. `distance_ms` is the distance to the
    /// nearest speech it did hear; `None` when it heard none in the epoch.
    NoSpeechNearSpan {
        silence: SilenceUnderSpan,
        distance_ms: Option<u32>,
    },
}

impl RefuseReason {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SealedReplay => "sealed_replay",
            Self::SlotAdmissionRejected => "slot_admission_rejected",
            Self::AuthorityConflict => "authority_conflict",
            Self::BatchDuplicate => "batch_duplicate",
            Self::EmptyLabel => "empty_label",
            Self::ReplayedRangeIdentity => "replayed_range_identity",
            Self::IntersectingPinNotExclusive => "intersecting_pin_not_exclusive",
            Self::ReplacedByWhisper => "replaced_by_whisper",
            Self::ReplacedByCloudLive => "replaced_by_cloud_live",
            Self::ClockLie => "clock_lie",
            Self::UnrecoveredSpeech => "unrecovered_speech",
            Self::NoVoicedHopInPin => "no_voiced_hop_in_pin",
            Self::SpanWithoutOwnSpeech => "span_without_own_speech",
            Self::SpanExceedsSyllableCeiling => "span_exceeds_syllable_ceiling",
            Self::IdenticalRepeatWithoutOwnVoicedSegment => {
                "identical_repeat_without_own_voiced_segment"
            }
            Self::ConfirmedNoSpeech => "confirmed_no_speech",
            Self::NoSpeechNearSpan { .. } => "no_speech_near_span",
        }
    }
}

/// Where one timed pin sits relative to a window's exclusive admit range.
///
/// The admit bounds are the window's non-overlapping remainder. Word pins
/// belong to the window containing their midpoint; phrase pins retain whole
/// range containment. A word still needs one open member to own its full span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapPinClass {
    /// The pin's whole text belongs to this open member.
    ExclusiveTail {
        /// Index into the open-member slice passed to the classifier.
        member_index: usize,
    },
    /// A word belongs to another window or repeats an admitted Whisper pin.
    Replay,
    /// Read-only evidence. The reason says why it cannot mutate a neighbour.
    Unanchored(NoAuthorityReason),
}

/// The one-to-one answer the ledger owes for every observation offered to it.
///
/// Exactly one receipt is produced per observation, in input order. That is
/// what makes conservation auditable rather than asserted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// No usable acoustic anchor. The text stays visible at its PCM position
    /// so the reader can see what was said, but it may not overwrite, clip, or
    /// delete any anchored occurrence, and it does not enter the ledger.
    KeepVisibleUnanchored {
        /// PCM position of the evidence. Not a committed token.
        occurrence: OccurrenceIdentity,
        /// The text that stays visible. Absent authority never deletes it.
        label: String,
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

/// Independent lexical evidence on an already resolved acoustic range.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SlotWitness {
    /// No witness verdict has been issued; absence never authorizes deletion.
    Unwitnessed,
    /// An independent decode agrees on this physical range.
    Confirmed,
    /// A timed independent decode contests the original label. The corrected
    /// slot retains the dispute and its measured scope for inspection.
    Contested {
        witness_label: String,
        range: OccurrenceIdentity,
    },
    /// The witness decoded this range without a label. This never deletes it.
    Absent { range: OccurrenceIdentity },
}

/// Ordered lexical evidence inside one physically owned occurrence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WordSlot {
    /// First sample of the speech projection used for ordering/painting.
    /// Original producer coordinates remain in slot-operation receipts.
    pub sample_start: u64,
    /// Exclusive end of the speech projection; not word-identity authority.
    pub sample_end: u64,
    /// Surface text, never an occurrence identity.
    pub text: String,
    /// Producer that supplied the word.
    pub producer: ObservationProducer,
    /// Observation that supplied the word.
    pub observation: ObservationIdentity,
    /// Speech witness status for this slot.
    pub witness: SlotWitness,
    /// Per-word acoustic confidence from the producer that emitted this slot
    /// (A6). Travels with the slot: when a higher-rank producer replaces the
    /// slot, its own confidence replaces this one; confidence is never
    /// transferred between producers. `None` = no metric, never "low".
    pub confidence: Option<crate::pipeline::word_confidence::WordConfidence>,
    /// Live lexicon rewrote the surface before admission (d5). The acoustic
    /// confidence still describes the heard audio, not the rewritten string.
    pub surface_rewritten: bool,
}

/// One word pin offered to the ledger: PCM range, surface text, and optional
/// per-word acoustic confidence (A6). Text is a label, never a key; the
/// Declared ranges are evidence; ledger speech anchors own word identity.
#[derive(Debug, Clone, PartialEq)]
pub struct WordPin {
    /// Original first sample measured for this word, before owner clipping.
    pub sample_start: u64,
    /// Original exclusive last sample measured for this word.
    pub sample_end: u64,
    /// Surface text (post live-lexicon rewrite, when one applied).
    pub text: String,
    /// Producer-supplied per-word confidence. `None` = the producer supplied
    /// no metric — never "low" (d4).
    pub confidence: Option<crate::pipeline::word_confidence::WordConfidence>,
    /// Live lexicon rewrote the surface before admission (d5).
    pub surface_rewritten: bool,
    /// Actual left fence of the PCM decoded for this candidate, before routing.
    /// Missing evidence does not imply that the window began at the word pin.
    pub decode_sample_start: Option<u64>,
    /// Actual right fence; together the fences prove a complete decoded word.
    pub decode_sample_end: Option<u64>,
}

impl WordPin {
    /// A pin with no confidence evidence and an unrewritten surface.
    pub fn new(sample_start: u64, sample_end: u64, text: impl Into<String>) -> Self {
        Self {
            sample_start,
            sample_end,
            text: text.into(),
            confidence: None,
            surface_rewritten: false,
            decode_sample_start: None,
            decode_sample_end: None,
        }
    }

    pub fn with_confidence(
        mut self,
        confidence: crate::pipeline::word_confidence::WordConfidence,
    ) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn surface_rewritten(mut self) -> Self {
        self.surface_rewritten = true;
        self
    }

    pub fn with_decode_start(mut self, sample_start: u64) -> Self {
        self.decode_sample_start = Some(sample_start);
        self
    }

    pub fn with_decode_window(mut self, sample_start: u64, sample_end: u64) -> Self {
        self.decode_sample_start = Some(sample_start);
        self.decode_sample_end = Some(sample_end);
        self
    }
}

pub(crate) fn normalize_word_token(word: &str) -> String {
    word.trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn edge_token_duplicate(
    ledger: &AcousticLedger,
    held: &WordSlot,
    word: &WordSlot,
    normalized: &str,
) -> bool {
    // A neighbouring spelling or a token somewhere in a coarse label does
    // not prove replay. Require one measured word on the same physical pin,
    // or across an occurrence cut where each owner clipped the same declared
    // word range to its side of the boundary.
    !normalized.is_empty()
        && word.text.split_whitespace().count() == 1
        && held.text.split_whitespace().count() == 1
        && held.observation != word.observation
        && held
            .observation
            .occurrence
            .same_capture(&word.observation.occurrence)
        && normalized == normalize_word_token(&held.text)
        && (ledger.pin_targets_source(held, word)
            || (held.observation.occurrence != word.observation.occurrence
                && ledger
                    .original_anchor_word_range(held)
                    .zip(ledger.original_anchor_word_range(word))
                    .is_some_and(|(left, right)| {
                        declared_ranges_share_word(
                            left.sample_start,
                            left.sample_end,
                            right.sample_start,
                            right.sample_end,
                        )
                    })))
}

/// Whether `host` contains the centre of `subject`.
///
/// A zero-length or reversed subject has no centre. This assigns an owner.
/// It supplies declared-clock routing geometry, never word identity.
/// Two declared ranges name one word when they overlap by at least half of
/// the shorter one and either contains the other's midpoint. This is the
/// producer's own geometry; it carries no speech measurement.
pub(crate) fn declared_ranges_share_word(
    left_start: u64,
    left_end: u64,
    right_start: u64,
    right_end: u64,
) -> bool {
    let left_len = left_end.saturating_sub(left_start);
    let right_len = right_end.saturating_sub(right_start);
    if left_len == 0 || right_len == 0 {
        return false;
    }
    let overlap = left_end
        .min(right_end)
        .saturating_sub(left_start.max(right_start));
    let shorter = left_len.min(right_len);
    if overlap < shorter.div_ceil(2) {
        return false;
    }
    declared_range_contains_midpoint(left_start, left_end, right_start, right_end)
        || declared_range_contains_midpoint(right_start, right_end, left_start, left_end)
}

pub(crate) fn declared_range_contains_midpoint(
    host_start: u64,
    host_end: u64,
    subject_start: u64,
    subject_end: u64,
) -> bool {
    let len = subject_end.saturating_sub(subject_start);
    if len == 0 {
        return false;
    }
    let centre = subject_start + len / 2;
    host_start <= centre && centre < host_end
}

fn compose_label(slots: &[WordSlot]) -> String {
    slots
        .iter()
        .map(|slot| slot.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reason of a [`SlotAlternative`] that only records another engine's word on
/// its own PCM range (D3: hypotheses are collected, not rejected). It is
/// evidence for adjudication and inspection, never a refused authority.
pub const COLLECTED_HYPOTHESIS: &str = "collected_hypothesis";

/// A candidate that did not prove a unique mutation target. The current words
/// remain intact; producer and generation travel with the candidate for W-6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotAlternative {
    pub observation: ObservationIdentity,
    pub candidate: String,
    pub sources: Vec<WordSlot>,
    pub reason: &'static str,
}

/// What the ledger remembers about a committed occurrence.
#[derive(Debug, Clone)]
struct CommittedObservation {
    producer: ObservationProducer,
    generation: u64,
    slots: Vec<WordSlot>,
    /// Read memo. Only `recompose` writes it, always from the slots.
    label: String,
}

impl CommittedObservation {
    fn empty(observation: &ObservationIdentity) -> Self {
        Self {
            producer: observation.producer,
            generation: observation.generation,
            slots: Vec::new(),
            label: String::new(),
        }
    }

    fn from_label(observation: &ObservationIdentity, text: &str) -> Self {
        let mut held = Self {
            producer: observation.producer,
            generation: observation.generation,
            slots: vec![WordSlot {
                sample_start: observation.occurrence.sample_start,
                sample_end: observation.occurrence.sample_end,
                text: text.to_string(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                // A whole-label slot has no per-word metric (A6).
                confidence: None,
                surface_rewritten: false,
            }],
            label: String::new(),
        };
        held.recompose();
        held
    }

    fn recompose(&mut self) {
        self.slots.sort_by(|a, b| {
            (a.sample_start, a.sample_end, &a.text).cmp(&(b.sample_start, b.sample_end, &b.text))
        });
        self.label = compose_label(&self.slots);
        debug_assert_eq!(self.label, compose_label(&self.slots));
    }
}

/// Conservation accounting over one admission batch.
///
/// Counted over *physical occurrences*, never over hypotheses: a cumulative
/// engine that restates the same five occurrences twelve times still yields
/// five.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConservationTally {
    /// Observations offered. Counted at the admission API, not from the trail.
    pub observations_in: usize,
    /// Receipts issued. Counted when a receipt is stored, not from the trail.
    pub receipts_out: usize,
    /// Distinct physical occurrences the ledger now holds.
    pub occurrences_held: usize,
    /// Observations whose receipt bound them to an occurrence.
    /// Counted when the receipt is stored, not derived from `observations_in`.
    pub observations_delivered: usize,
    /// Observations admitted without mutation authority.
    pub kept_visible_unanchored: usize,
    /// Refuse and no-authority receipts, keyed by their stable reason name.
    /// Incremented as each receipt is issued.
    pub refusals_by_reason: BTreeMap<&'static str, usize>,
}

impl ConservationTally {
    /// Admitted minus delivered minus the named observation refusals.
    pub fn residue(&self) -> i64 {
        let named: usize = self.refusals_by_reason.values().copied().sum();
        self.observations_in as i64 - self.observations_delivered as i64 - named as i64
    }
}

/// The capture writer remains authoritative; a bounded retained-PCM quiet
/// receipt only refines its block-level measurement at one decoder fence.
#[derive(Debug, Clone)]
struct DecodeFenceEnergy {
    owner: CaptureEnergyOwner,
    captured_samples: u64,
    quiet_ranges: Vec<OccurrenceIdentity>,
}

#[derive(Debug, Clone, Default)]
struct ProducerHistory {
    latest_answered: Option<(u64, u64)>,
    highest_generation: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct OwnerHistory {
    answered: usize,
    producers: BTreeMap<ObservationProducer, ProducerHistory>,
    trail_ordinals: Vec<usize>,
}

/// Ledger of committed acoustic occurrences.
///
/// Text is never a key here. The only key is [`OccurrenceIdentity`].
#[derive(Debug, Clone, Default)]
pub struct AcousticLedger {
    anchor_word_state: speech_anchor::SpeechAnchorState,
    /// Authenticated physical map, also consumed by the trail replay owner.
    pub(super) speech_anchors: Option<crate::audio::capture_receipt::SpeechAnchorEvidence>,
    /// Actual exhausted capture extent. Only STOP after draining capture sets it;
    /// a decoder window ending at the current live head is not this receipt.
    pub(super) anchor_capture_end: Option<OccurrenceIdentity>,
    /// Read-only exact-PCM fence receipts restored from a decision trail.
    /// Production measurements remain owned by CaptureEnergyOwner.
    pub(super) replay_quiet_fences: Option<Vec<OccurrenceIdentity>>,
    word_adjudication: word_adjudication::WordAdjudicationState,
    committed: BTreeMap<OccurrenceIdentity, CommittedObservation>,
    slot_alternatives: Vec<SlotAlternative>,
    /// Rejected PCM awaiting accepted word evidence, keyed by its exact owner.
    /// Repeated observations of one range do not create additional debt pins.
    rejected_word_pins: BTreeMap<OccurrenceIdentity, Vec<WordSlot>>,
    slot_operations: Vec<SlotOperationReceipt>,
    /// Positions in `slot_operations` keyed by the occurrence of each output
    /// slot. The receipt history is append-only and public; this is the only
    /// way to ask it about one slot without walking the whole take. A sealed
    /// owner drops its word components, but its receipts stay in the history,
    /// so an unindexed lookup for a slot that was never split or merged (no
    /// receipt at all) costs every receipt of the take (2026-10-12).
    slot_operation_index: BTreeMap<OccurrenceIdentity, Vec<usize>>,
    pub(super) speech_evidence: Option<AcousticSpeechEvidence>,
    /// Shared capture observer and actual extent, used only to adjudicate a
    /// decoder fence. Cloning the handle does not create another observer.
    decode_fence_energy: Option<DecodeFenceEnergy>,
    assigned_word_pins: Option<slot_ops::AssignedWordPinBatch>,
    group_speech_coverages: Vec<GroupSpeechCoverageReceipt>,
    word_deletions: Vec<WordDeletionReceipt>,
    /// Provenance only: label-wide slots do not prove individual word pins.
    word_pin_observations: std::collections::HashSet<ObservationIdentity>,
    /// Slot projections whose original words have complete decoder-fence
    /// evidence. An empty entry records only incomplete edge pins.
    complete_decoded_words: std::collections::HashMap<ObservationIdentity, Vec<(u64, u64)>>,
    /// Actual decoder fences, distinct from owner clipping and word geometry.
    decoded_word_windows: std::collections::HashMap<ObservationIdentity, (u64, u64)>,
    /// Returned Whisper and cloud fences merged per capture, so asking whether
    /// complete decode covered an owner is one lookup, not a scan per tick.
    complete_decode_spans: BTreeMap<String, BTreeMap<u64, Vec<(u64, u64)>>>,
    /// Parts of a retired coarse label that have not yet been published by
    /// an accepted measured word partition. Decoder context does not pay it.
    publication_debt: BTreeMap<OccurrenceIdentity, Vec<(u64, u64)>>,
    /// Matched quiet publication work, bound to its observation and clipped
    /// owner range. This is not an empty decoder-final receipt.
    quiet_publication_bands: std::collections::HashSet<(ObservationIdentity, OccurrenceIdentity)>,
    /// Matched successful empty finals; these observations mint no Word.
    successful_empty_decodes: std::collections::HashSet<ObservationIdentity>,
    answered: std::collections::HashSet<ObservationIdentity>,
    owner_history: BTreeMap<OccurrenceIdentity, OwnerHistory>,
    kept_visible: usize,
    evidence: BTreeMap<OccurrenceIdentity, AcousticSerial>,
    qualified_by_end: BTreeMap<u64, BTreeSet<OccurrenceIdentity>>,
    unsealed_qualified: BTreeSet<OccurrenceIdentity>,
    frontiers: BTreeMap<OccurrenceIdentity, ObservationFrontier>,
    seals: BTreeMap<OccurrenceIdentity, LedgerSealReceipt>,
    terminal_seals: Vec<LedgerSealReceipt>,
    trail: Vec<LayerDecisionReceipt>,
    manual_edits: Vec<ManualEditReceipt>,
    manual_document_revisions: Vec<ManualDocumentRevisionReceipt>,
    /// One opened capture capability; no acoustic evidence is minted here.
    manual_document_capture: Option<ManualDocumentCaptureReceipt>,
    incremental_shapings: Vec<IncrementalShapingReceipt>,
    latest_incremental_shaping: BTreeMap<OccurrenceIdentity, usize>,
    consultation_presentations: Vec<ConsultationPresentationReceipt>,
    derivations: Vec<OccurrenceDerivation>,
    latest_seal_coverage: Option<SealCoverageReceipt>,
    latest_speech_islands: Option<super::speech_islands::SpeechIslandEvidence>,
    pending_text_recovery: BTreeSet<OccurrenceIdentity>,
    /// Observations handed to an admission API. Independent of [`Self::trail`].
    offered_observations: usize,
    /// Receipts actually stored. Independent of [`Self::offered_observations`].
    issued_receipts: usize,
    /// Insert, correct, and preserve receipts. Independent of the offer counter.
    delivered_observations: usize,
    /// Named refuse and no-authority reasons, counted at issue time.
    named_refusals: BTreeMap<&'static str, usize>,
    /// Capture rate that turns a declared sample range into a duration.
    capture_rate_hz: Option<u32>,
    /// Observational admission input, consumed by the disk trail only.
    trail_input: Option<super::trail::TrailAdmission>,
    /// Occurrences whose character rate over the declared range is a clock-lie.
    clock_lie_occurrences: BTreeSet<OccurrenceIdentity>,
    /// Lowest committed sample on this ledger, once any anchored text lands.
    first_covered_sample: Option<u64>,
    /// Highest committed sample end on this ledger.
    last_covered_sample: Option<u64>,
    /// Wall time of the first successful terminal seal, milliseconds since epoch.
    transcript_seal_timestamp_ms: Option<u64>,
    /// Energy-ladder lookups whose observed clock held no voiced hop.
    energy_lookups_without_voiced_hop: u64,
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
            .map(|held| held.label.as_str())
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
            .map(|held| held.label.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
#[path = "acoustic_ledger/tests.rs"]
mod tests;
