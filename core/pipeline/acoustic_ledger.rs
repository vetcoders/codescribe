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
//! What the ledger deliberately does *not* do: infer identity from text,
//! invent sub-ranges the payload does not carry, or let an unanchored
//! hypothesis suppress an anchored one.

use std::collections::BTreeMap;

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
    /// The same occurrence is already held by an equal-or-higher authority at
    /// an equal-or-newer generation, and this hypothesis disagrees with it.
    SealedReplay,
    /// The exact same observation identity was already answered in this batch.
    BatchDuplicate,
}

impl RefuseReason {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SealedReplay => "sealed_replay",
            Self::BatchDuplicate => "batch_duplicate",
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

    /// Offer one observation and receive exactly one receipt.
    ///
    /// `text` is what the producer heard. It is recorded, compared for
    /// equality, and never used to establish identity.
    pub fn admit(&mut self, observation: &ObservationIdentity, text: &str) -> MutationReceipt {
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
    /// This is where `revision_tolerant_known_prefix` and
    /// `strip_suffix_overlap_live` lose their authority: their answer is an
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
