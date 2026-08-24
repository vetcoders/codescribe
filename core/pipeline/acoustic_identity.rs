//! Occurrence, observation, and mutation receipt for acoustic spans.
//!
//! Three separate identities:
//! - [`OccurrenceIdentity`] — one physical fragment of captured audio
//! - [`ObservationIdentity`] — one producer hypothesis about that fragment
//! - [`MutationReceipt`] — why that hypothesis may keep, correct, insert,
//!   stay visible, or be refused
//!
//! Replay is re-delivery of the same [`ObservationIdentity`], not merely the
//! same PCM range. Apple and Whisper on one range are two observations of one
//! occurrence. Two disjoint ranges with the text "Iwo" are two occurrences.
//!
//! Energy hops are quality evidence on the PCM axis. They never hash identity.
//! Mean dBFS is not an ID.

use std::collections::{BTreeMap, HashSet};

use crate::pipeline::contracts::{AcousticSpanGrain, AcousticTranscriptSpan};
use crate::stt::tail_provider::{
    TailEvidenceSource, TailProviderPayload, TailSampleRange, TimedTailSegment,
};

/// Physical occurrence: session, epoch, true half-open PCM range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OccurrenceIdentity {
    pub session: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl OccurrenceIdentity {
    pub fn from_range(range: &TailSampleRange) -> Self {
        Self {
            session: range.session.clone(),
            capture_epoch: range.capture_epoch,
            sample_start: range.sample_start,
            sample_end: range.sample_end,
        }
    }

    pub fn range(&self) -> TailSampleRange {
        TailSampleRange {
            session: self.session.clone(),
            capture_epoch: self.capture_epoch,
            sample_start: self.sample_start,
            sample_end: self.sample_end,
        }
    }

    pub fn is_anchored(&self) -> bool {
        self.sample_end > self.sample_start
    }

    fn partition_key(&self) -> (String, u64) {
        (self.session.clone(), self.capture_epoch)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.session == other.session
            && self.capture_epoch == other.capture_epoch
            && self.sample_start < other.sample_end
            && other.sample_start < self.sample_end
    }
}

/// Who produced a hypothesis about an occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObservationProducer {
    Apple,
    Whisper,
}

impl ObservationProducer {
    pub fn from_source(source: TailEvidenceSource) -> Self {
        match source {
            TailEvidenceSource::AppleSpeech => Self::Apple,
            TailEvidenceSource::Whisper => Self::Whisper,
        }
    }
}

/// One producer / request / generation hypothesising about one occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObservationIdentity {
    pub producer: ObservationProducer,
    pub request_id: u64,
    pub generation: u64,
    pub occurrence: OccurrenceIdentity,
}

/// Proven word pin. Overlap may clip text only through these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcousticWordPin {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
}

/// One hypothesis: identity, text, grain, optional pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcousticObservation {
    pub identity: ObservationIdentity,
    pub text: String,
    pub grain: AcousticSpanGrain,
    pub pins: Vec<AcousticWordPin>,
}

impl AcousticObservation {
    pub fn from_span(
        span: &AcousticTranscriptSpan,
        producer: ObservationProducer,
        request_id: u64,
        generation: u64,
    ) -> Self {
        Self {
            identity: ObservationIdentity {
                producer,
                request_id,
                generation,
                occurrence: OccurrenceIdentity::from_range(&span.range),
            },
            text: span.text.clone(),
            grain: span.grain,
            pins: Vec::new(),
        }
    }

    pub fn from_timed_segment(
        segment: &TimedTailSegment,
        producer: ObservationProducer,
        request_id: u64,
        generation: u64,
        grain: AcousticSpanGrain,
    ) -> Self {
        let pin = AcousticWordPin {
            text: segment.text.clone(),
            sample_start: segment.range.sample_start,
            sample_end: segment.range.sample_end,
        };
        Self {
            identity: ObservationIdentity {
                producer,
                request_id,
                generation,
                occurrence: OccurrenceIdentity::from_range(&segment.range),
            },
            text: segment.text.clone(),
            grain,
            pins: vec![pin],
        }
    }

    pub fn from_whisper_payload(payload: &TailProviderPayload) -> Vec<Self> {
        let producer = ObservationProducer::from_source(payload.evidence.source);
        let request_id = payload.identity.request_id;
        if payload.segments.is_empty() {
            return vec![Self {
                identity: ObservationIdentity {
                    producer,
                    request_id,
                    generation: 0,
                    occurrence: OccurrenceIdentity::from_range(&payload.identity.range),
                },
                text: payload.text.clone(),
                grain: AcousticSpanGrain::Phrase,
                pins: Vec::new(),
            }];
        }
        payload
            .segments
            .iter()
            .enumerate()
            .map(|(generation, segment)| {
                Self::from_timed_segment(
                    segment,
                    producer,
                    request_id,
                    generation as u64,
                    AcousticSpanGrain::Word,
                )
            })
            .collect()
    }

    pub fn as_span(&self) -> AcousticTranscriptSpan {
        AcousticTranscriptSpan {
            text: self.text.clone(),
            range: self.identity.occurrence.range(),
            grain: self.grain,
        }
    }
}

/// Why an observation may mutate, stay visible, or be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationReceipt {
    Preserve {
        occurrence: OccurrenceIdentity,
    },
    Correct {
        occurrence: OccurrenceIdentity,
        from: ObservationProducer,
        to: ObservationProducer,
    },
    Insert {
        occurrence: OccurrenceIdentity,
    },
    KeepVisibleUnanchored {
        text: String,
    },
    RefuseReplay {
        observation: ObservationIdentity,
    },
    RefuseOverlapWithoutTextMap {
        occurrence: OccurrenceIdentity,
    },
}

impl MutationReceipt {
    pub fn mutation_authority(&self) -> bool {
        matches!(
            self,
            Self::Preserve { .. } | Self::Correct { .. } | Self::Insert { .. }
        )
    }

    pub fn visible(&self) -> bool {
        !matches!(self, Self::RefuseReplay { .. })
    }
}

/// Admitted (or refused) observation plus the receipt that justified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedObservation {
    pub observation: AcousticObservation,
    pub receipt: MutationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OccurrenceSlot {
    identity: OccurrenceIdentity,
    current: AcousticObservation,
}

/// Ledger partitioned by `(session, capture_epoch)`.
#[derive(Debug, Clone, Default)]
pub struct ObservationLedger {
    partitions: BTreeMap<(String, u64), Vec<OccurrenceSlot>>,
    seen: HashSet<ObservationIdentity>,
    unanchored: Vec<AcousticObservation>,
    read_only: Vec<AcousticObservation>,
}

impl ObservationLedger {
    /// Admit a batch. Each incoming is compared with committed slots and with
    /// observations already accepted earlier in this batch.
    pub fn admit(&mut self, incoming: &[AcousticObservation]) -> Vec<AdmittedObservation> {
        let mut results = Vec::with_capacity(incoming.len());
        for observation in incoming {
            let admitted = self.admit_one(observation);
            match &admitted.receipt {
                MutationReceipt::Insert { .. } | MutationReceipt::Preserve { .. } => {
                    self.record_slot(OccurrenceSlot {
                        identity: admitted.observation.identity.occurrence.clone(),
                        current: admitted.observation.clone(),
                    });
                    self.seen.insert(admitted.observation.identity.clone());
                }
                MutationReceipt::Correct { occurrence, .. } => {
                    self.replace_slot(occurrence, admitted.observation.clone());
                    self.seen.insert(admitted.observation.identity.clone());
                }
                MutationReceipt::KeepVisibleUnanchored { .. } => {
                    self.unanchored.push(admitted.observation.clone());
                }
                MutationReceipt::RefuseOverlapWithoutTextMap { .. } => {
                    self.read_only.push(admitted.observation.clone());
                }
                MutationReceipt::RefuseReplay { .. } => {}
            }
            results.push(admitted);
        }
        results
    }

    /// Mutation-authority texts in PCM order. Unanchored and read-only evidence
    /// stay out of this projection.
    pub fn delivery_text(&self) -> String {
        let mut slots: Vec<&OccurrenceSlot> = self.partitions.values().flatten().collect();
        slots.sort_by_key(|slot| {
            (
                slot.identity.capture_epoch,
                slot.identity.sample_start,
                slot.identity.sample_end,
            )
        });
        slots
            .into_iter()
            .map(|slot| slot.current.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Visible unanchored text, without mutation authority.
    pub fn unanchored_text(&self) -> String {
        self.unanchored
            .iter()
            .map(|observation| observation.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn committed_spans(&self) -> Vec<AcousticTranscriptSpan> {
        self.partitions
            .values()
            .flatten()
            .map(|slot| slot.current.as_span())
            .collect()
    }

    fn admit_one(&self, incoming: &AcousticObservation) -> AdmittedObservation {
        if !incoming.identity.occurrence.is_anchored() {
            return AdmittedObservation {
                observation: incoming.clone(),
                receipt: MutationReceipt::KeepVisibleUnanchored {
                    text: incoming.text.clone(),
                },
            };
        }

        if self.seen.contains(&incoming.identity) {
            return AdmittedObservation {
                observation: incoming.clone(),
                receipt: MutationReceipt::RefuseReplay {
                    observation: incoming.identity.clone(),
                },
            };
        }

        let priors = self.priors_for(&incoming.identity.occurrence);

        if let Some(prior) = priors
            .iter()
            .find(|slot| slot.identity == incoming.identity.occurrence)
        {
            let from = prior.current.identity.producer;
            return AdmittedObservation {
                observation: incoming.clone(),
                receipt: MutationReceipt::Correct {
                    occurrence: incoming.identity.occurrence.clone(),
                    from,
                    to: incoming.identity.producer,
                },
            };
        }

        if let Some(prior) = priors
            .iter()
            .find(|slot| slot.identity.overlaps(&incoming.identity.occurrence))
        {
            return admit_overlap(incoming, prior);
        }

        let receipt = if priors.is_empty() {
            MutationReceipt::Preserve {
                occurrence: incoming.identity.occurrence.clone(),
            }
        } else {
            MutationReceipt::Insert {
                occurrence: incoming.identity.occurrence.clone(),
            }
        };

        AdmittedObservation {
            observation: incoming.clone(),
            receipt,
        }
    }

    fn priors_for(&self, occurrence: &OccurrenceIdentity) -> &[OccurrenceSlot] {
        self.partitions
            .get(&occurrence.partition_key())
            .map_or(&[], Vec::as_slice)
    }

    fn record_slot(&mut self, slot: OccurrenceSlot) {
        self.partitions
            .entry(slot.identity.partition_key())
            .or_default()
            .push(slot);
    }

    fn replace_slot(&mut self, occurrence: &OccurrenceIdentity, observation: AcousticObservation) {
        if let Some(slots) = self.partitions.get_mut(&occurrence.partition_key())
            && let Some(slot) = slots.iter_mut().find(|slot| slot.identity == *occurrence)
        {
            slot.current = observation;
        }
    }
}

fn admit_overlap(incoming: &AcousticObservation, prior: &OccurrenceSlot) -> AdmittedObservation {
    let clip_start = incoming
        .identity
        .occurrence
        .sample_start
        .max(prior.identity.sample_end);
    if clip_start >= incoming.identity.occurrence.sample_end {
        return AdmittedObservation {
            observation: incoming.clone(),
            receipt: MutationReceipt::RefuseOverlapWithoutTextMap {
                occurrence: incoming.identity.occurrence.clone(),
            },
        };
    }

    let exclusive = OccurrenceIdentity {
        session: incoming.identity.occurrence.session.clone(),
        capture_epoch: incoming.identity.occurrence.capture_epoch,
        sample_start: clip_start,
        sample_end: incoming.identity.occurrence.sample_end,
    };

    let Some(clipped_text) = text_mapped_to_range(incoming, &exclusive) else {
        return AdmittedObservation {
            observation: incoming.clone(),
            receipt: MutationReceipt::RefuseOverlapWithoutTextMap {
                occurrence: incoming.identity.occurrence.clone(),
            },
        };
    };

    let mut clipped = incoming.clone();
    clipped.text = clipped_text;
    clipped.identity.occurrence = exclusive.clone();
    clipped.pins.retain(|pin| {
        pin.sample_start >= exclusive.sample_start && pin.sample_end <= exclusive.sample_end
    });
    AdmittedObservation {
        observation: clipped,
        receipt: MutationReceipt::Insert {
            occurrence: exclusive,
        },
    }
}

fn text_mapped_to_range(
    observation: &AcousticObservation,
    range: &OccurrenceIdentity,
) -> Option<String> {
    if observation.pins.is_empty() {
        return None;
    }
    let kept: Vec<&str> = observation
        .pins
        .iter()
        .filter(|pin| pin.sample_start >= range.sample_start && pin.sample_end <= range.sample_end)
        .map(|pin| pin.text.as_str())
        .filter(|text| !text.is_empty())
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

/// How two PCM ranges relate. Text is not an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticSpanRelation {
    Same,
    Overlapping,
    Disjoint,
    Unanchored,
    DifferentEpoch,
}

/// Quality evidence for a range. Not part of identity. Not Eq/Hash of the span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcousticQualityEvidence {
    pub energy_db: Option<f32>,
}

/// Relate two capture ranges. Energy is ignored on purpose.
pub fn relate(left: &TailSampleRange, right: &TailSampleRange) -> AcousticSpanRelation {
    if left.sample_end <= left.sample_start || right.sample_end <= right.sample_start {
        return AcousticSpanRelation::Unanchored;
    }
    if left.session != right.session || left.capture_epoch != right.capture_epoch {
        return AcousticSpanRelation::DifferentEpoch;
    }
    if left.sample_start == right.sample_start && left.sample_end == right.sample_end {
        return AcousticSpanRelation::Same;
    }
    if left.overlaps(right) {
        return AcousticSpanRelation::Overlapping;
    }
    AcousticSpanRelation::Disjoint
}

/// Mean energy is quality evidence. It never decides Same vs Disjoint.
pub fn mean_energy_is_identity(_evidence: AcousticQualityEvidence) -> bool {
    false
}

/// Admit incoming observations against a fresh ledger seeded with `committed`.
pub fn admit_observations(
    committed: &[AcousticObservation],
    incoming: &[AcousticObservation],
) -> (ObservationLedger, Vec<AdmittedObservation>) {
    let mut ledger = ObservationLedger::default();
    ledger.admit(committed);
    let admitted = ledger.admit(incoming);
    (ledger, admitted)
}

/// Join mutation-authority texts in admit order.
pub fn mutation_authority_text(admitted: &[AdmittedObservation]) -> String {
    admitted
        .iter()
        .filter(|item| item.receipt.mutation_authority())
        .map(|item| item.observation.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Span-shaped wrapper used by Apple/L1/bus callsites that still carry
/// [`AcousticTranscriptSpan`]. Producer defaults to Apple so a second copy of
/// the same range from Whisper must go through [`AcousticObservation`].
pub fn admit_acoustic_spans(
    committed: &[AcousticTranscriptSpan],
    incoming: &[AcousticTranscriptSpan],
) -> (Vec<AcousticTranscriptSpan>, Vec<MutationReceipt>) {
    let committed_obs: Vec<_> = committed
        .iter()
        .map(|span| AcousticObservation::from_span(span, ObservationProducer::Apple, 0, 0))
        .collect();
    let incoming_obs: Vec<_> = incoming
        .iter()
        .map(|span| AcousticObservation::from_span(span, ObservationProducer::Apple, 0, 0))
        .collect();
    let (ledger, admitted) = admit_observations(&committed_obs, &incoming_obs);
    let receipts = admitted.into_iter().map(|item| item.receipt).collect();
    (ledger.committed_spans(), receipts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "take".into(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        }
    }

    fn iwo(start: u64, end: u64) -> AcousticTranscriptSpan {
        AcousticTranscriptSpan {
            text: "Iwo".into(),
            range: range(start, end),
            grain: AcousticSpanGrain::Word,
        }
    }

    fn apple_obs(span: &AcousticTranscriptSpan, request_id: u64) -> AcousticObservation {
        AcousticObservation::from_span(span, ObservationProducer::Apple, request_id, 0)
    }

    #[test]
    fn five_iwo_on_five_pcm_ranges_all_survive() {
        let incoming: Vec<_> = (0..5)
            .map(|i| apple_obs(&iwo(i * 1600, i * 1600 + 1600), i))
            .collect();
        let (ledger, receipts) = admit_observations(&[], &incoming);
        assert_eq!(ledger.committed_spans().len(), 5);
        assert!(
            receipts
                .iter()
                .all(|item| item.receipt.mutation_authority())
        );
        assert_eq!(ledger.delivery_text(), "Iwo Iwo Iwo Iwo Iwo");
    }

    #[test]
    fn replaying_the_same_observation_does_not_mint_a_sixth() {
        let first = apple_obs(&iwo(0, 1600), 7);
        let (mut ledger, _) = admit_observations(&[], std::slice::from_ref(&first));
        let again = ledger.admit(&[first]);
        assert!(ledger.committed_spans().len() == 1);
        assert!(matches!(
            again[0].receipt,
            MutationReceipt::RefuseReplay { .. }
        ));
        assert_eq!(ledger.delivery_text(), "Iwo");
    }

    #[test]
    fn apple_then_whisper_same_range_is_correction() {
        let apple = apple_obs(&iwo(0, 1600), 1);
        let mut whisper = AcousticObservation::from_span(
            &AcousticTranscriptSpan {
                text: "Ivo".into(),
                range: range(0, 1600),
                grain: AcousticSpanGrain::Word,
            },
            ObservationProducer::Whisper,
            99,
            0,
        );
        whisper.text = "Iwo".into();
        let (ledger, receipts) = admit_observations(&[apple], &[whisper]);
        assert_eq!(ledger.committed_spans().len(), 1);
        assert_eq!(ledger.delivery_text(), "Iwo");
        assert!(matches!(
            receipts[0].receipt,
            MutationReceipt::Correct {
                from: ObservationProducer::Apple,
                to: ObservationProducer::Whisper,
                ..
            }
        ));
    }

    #[test]
    fn overlap_without_pins_does_not_clip_text() {
        let committed = [apple_obs(&iwo(0, 48_000), 1)];
        let incoming = AcousticObservation {
            identity: ObservationIdentity {
                producer: ObservationProducer::Whisper,
                request_id: 2,
                generation: 0,
                occurrence: OccurrenceIdentity::from_range(&range(32_000, 64_000)),
            },
            text: "Iwo later".into(),
            grain: AcousticSpanGrain::Phrase,
            pins: Vec::new(),
        };
        let (ledger, receipts) = admit_observations(&committed, &[incoming]);
        assert_eq!(ledger.delivery_text(), "Iwo");
        assert!(
            !ledger.delivery_text().contains("later"),
            "unmapped overlap must not mint a textual duplicate"
        );
        assert!(matches!(
            receipts[0].receipt,
            MutationReceipt::RefuseOverlapWithoutTextMap { .. }
        ));
        assert_eq!(receipts[0].observation.text, "Iwo later");
    }

    #[test]
    fn overlap_with_pins_clips_range_and_text() {
        let committed = [apple_obs(&iwo(0, 48_000), 1)];
        let incoming = AcousticObservation {
            identity: ObservationIdentity {
                producer: ObservationProducer::Whisper,
                request_id: 2,
                generation: 0,
                occurrence: OccurrenceIdentity::from_range(&range(32_000, 64_000)),
            },
            text: "Iwo later".into(),
            grain: AcousticSpanGrain::Phrase,
            pins: vec![
                AcousticWordPin {
                    text: "Iwo".into(),
                    sample_start: 32_000,
                    sample_end: 40_000,
                },
                AcousticWordPin {
                    text: "later".into(),
                    sample_start: 48_000,
                    sample_end: 64_000,
                },
            ],
        };
        let (ledger, _) = admit_observations(&committed, &[incoming]);
        assert_eq!(ledger.delivery_text(), "Iwo later");
        let spans = ledger.committed_spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].text, "later");
        assert_eq!(spans[1].range.sample_start, 48_000);
        assert_eq!(spans[1].range.sample_end, 64_000);
    }

    #[test]
    fn unanchored_range_stays_visible_without_mutation() {
        let empty = AcousticObservation::from_span(
            &AcousticTranscriptSpan {
                text: "Iwo".into(),
                range: range(8, 8),
                grain: AcousticSpanGrain::Word,
            },
            ObservationProducer::Apple,
            1,
            0,
        );
        let (ledger, receipts) = admit_observations(&[], &[empty]);
        assert!(ledger.committed_spans().is_empty());
        assert_eq!(ledger.unanchored_text(), "Iwo");
        assert!(!receipts[0].receipt.mutation_authority());
        assert!(receipts[0].receipt.visible());
        assert!(matches!(
            receipts[0].receipt,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
    }

    #[test]
    fn two_identical_observations_in_one_batch_second_is_replay() {
        let first = apple_obs(&iwo(0, 1600), 3);
        let duplicate = first.clone();
        let (ledger, receipts) = admit_observations(&[], &[first, duplicate]);
        assert_eq!(ledger.committed_spans().len(), 1);
        assert!(receipts[0].receipt.mutation_authority());
        assert!(matches!(
            receipts[1].receipt,
            MutationReceipt::RefuseReplay { .. }
        ));
    }

    #[test]
    fn different_epoch_is_partitioned_not_refused() {
        let epoch_one = apple_obs(&iwo(0, 1600), 1);
        let mut epoch_two = apple_obs(&iwo(0, 1600), 2);
        epoch_two.identity.occurrence.capture_epoch = 2;
        epoch_two.identity.occurrence.session = "take".into();
        let (ledger, receipts) = admit_observations(&[epoch_one], &[epoch_two]);
        assert_eq!(ledger.committed_spans().len(), 2);
        assert!(receipts[0].receipt.mutation_authority());
    }

    #[test]
    fn mean_energy_never_decides_identity() {
        assert!(!mean_energy_is_identity(AcousticQualityEvidence {
            energy_db: Some(-18.0),
        }));
        assert_eq!(
            relate(&range(0, 1600), &range(1600, 3200)),
            AcousticSpanRelation::Disjoint
        );
        assert_eq!(
            relate(&range(0, 1600), &range(0, 1600)),
            AcousticSpanRelation::Same
        );
    }

    #[test]
    fn string_suffix_dedup_is_the_forbidden_path_for_anchored_spans() {
        let incoming: Vec<_> = (0..5)
            .map(|i| apple_obs(&iwo(i * 1600, i * 1600 + 1600), i))
            .collect();
        let (ledger, _) = admit_observations(&[], &incoming);
        let collapsed = crate::pipeline::dedup::strip_suffix_overlap_live(
            "Iwo Iwo Iwo Iwo",
            "Iwo Iwo Iwo Iwo Iwo",
        );
        assert_ne!(
            collapsed.split_whitespace().count(),
            5,
            "text overlap is the bug this module exists to refuse"
        );
        assert_eq!(ledger.delivery_text().split_whitespace().count(), 5);
    }
}
