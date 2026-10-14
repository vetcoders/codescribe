//! Capture-owned speech identity, distinct from a producer's declared clock.
//! The ledger owns these associations; they are evidence, never a document.

use super::*;
use crate::audio::capture_receipt::{AcousticAvailability, SpeechAnchor};

struct LexicalAnchorPin<'a> {
    original: &'a WordPin,
    lexical_index: usize,
    token_count: usize,
    text: &'a str,
}

impl LexicalAnchorPin<'_> {
    /// The producer's geometry for this token: a compound pin is divided
    /// equally among its tokens in lexical order. This is the producer's
    /// clock, not a measurement.
    fn declared(&self) -> (u64, u64) {
        let (start, end) = (self.original.sample_start, self.original.sample_end);
        if self.token_count <= 1 || end <= start {
            return (start, end);
        }
        let length = u128::from(end - start);
        let count = u128::from(self.token_count as u64);
        let at = |index: usize| start + (length * u128::from(index as u64) / count) as u64;
        (at(self.lexical_index), at(self.lexical_index + 1))
    }
}

impl std::ops::Deref for LexicalAnchorPin<'_> {
    type Target = WordPin;
    fn deref(&self) -> &Self::Target {
        self.original
    }
}

/// Several consecutive words may share the same burst. The burst is the
/// physical identity; where one burst carries more than one word, the
/// producer's declared clock, clipped to that burst, says which of them a
/// claim addresses. No ordinal: a count of claims is not a position in speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordAnchorIdentity {
    pub anchors: Vec<OccurrenceIdentity>,
    /// One declared span per anchor, in the same order as `anchors`: the
    /// producer's range clipped to the burst, or the declared range itself
    /// when the claim does not overlap the burst it was assigned to.
    pub spans: Vec<OccurrenceIdentity>,
}

impl WordAnchorIdentity {
    #[cfg(test)]
    pub(super) fn single(anchor: OccurrenceIdentity, declared: (u64, u64)) -> Self {
        let span = anchor_span(&anchor, declared);
        Self {
            anchors: vec![anchor],
            spans: vec![span],
        }
    }

    pub(super) fn from_anchors(anchors: Vec<OccurrenceIdentity>, declared: (u64, u64)) -> Self {
        let spans = anchors
            .iter()
            .map(|anchor| anchor_span(anchor, declared))
            .collect();
        Self { anchors, spans }
    }

    pub(super) fn shares_word(&self, other: &Self) -> bool {
        self.anchors
            .iter()
            .zip(&self.spans)
            .any(|(left, left_span)| {
                other
                    .anchors
                    .iter()
                    .zip(&other.spans)
                    .any(|(right, right_span)| {
                        left.same_capture(right)
                            && left.sample_start < right.sample_end
                            && right.sample_start < left.sample_end
                            && declared_ranges_share_word(
                                left_span.sample_start,
                                left_span.sample_end,
                                right_span.sample_start,
                                right_span.sample_end,
                            )
                    })
            })
    }
}

fn anchor_span(anchor: &OccurrenceIdentity, (start, end): (u64, u64)) -> OccurrenceIdentity {
    let clipped_start = start.max(anchor.sample_start);
    let clipped_end = end.min(anchor.sample_end);
    let (span_start, span_end) = if clipped_start < clipped_end {
        (clipped_start, clipped_end)
    } else {
        (start, end)
    };
    OccurrenceIdentity::new(&anchor.session, anchor.capture_epoch, span_start, span_end)
}

#[derive(Debug, Clone)]
struct AnchorWordRecord {
    slot: WordSlot,
    identity: WordAnchorIdentity,
    original: OccurrenceIdentity,
    original_pin: WordPin,
    lexical_index: usize,
    /// A complete ordered sequence can refine Apple's phrase-grain assignment.
    /// This addresses a native source slot, never a token-keyed document.
    sequence_source: Option<WordSlot>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SpeechAnchorState {
    floor_rms: Option<f32>,
    words: Vec<AnchorWordRecord>,
}

impl AcousticLedger {
    /// The caller supplies a floor derived from this device's calibration or
    /// observed PCM noise. Invalid input revokes the binding; no constant floor
    /// or synthetic speech is substituted when calibration is absent.
    pub fn bind_speech_anchor_floor(&mut self, floor_rms: f32) -> bool {
        if !floor_rms.is_finite() || floor_rms <= 0.0 || floor_rms > 1.0 {
            return false;
        }
        if let Some(bound) = self.anchor_word_state.floor_rms {
            return bound == floor_rms;
        }
        self.anchor_word_state.floor_rms = Some(floor_rms);
        true
    }

    pub(crate) fn authenticated_anchor_quiet_fences(&self) -> Vec<OccurrenceIdentity> {
        if let Some(energy) = self.decode_fence_energy.as_ref() {
            return energy
                .quiet_ranges
                .iter()
                .filter(|range| {
                    range.sample_end <= energy.captured_samples
                        && energy
                            .owner
                            .voiced_hops_in(
                                &range.session,
                                range.capture_epoch,
                                range.sample_start,
                                range.sample_end,
                            )
                            .is_some()
                })
                .cloned()
                .collect();
        }
        self.replay_quiet_fences.clone().unwrap_or_default()
    }

    pub(crate) fn close_speech_anchor_capture(
        &mut self,
        session: &str,
        epoch: u64,
        end: u64,
    ) -> bool {
        let Some(energy) = self.decode_fence_energy.as_ref() else {
            return false;
        };
        let Some(rate) = self.capture_rate_hz else {
            return false;
        };
        if end == 0
            || energy.captured_samples != end
            || !energy.owner.identity().matches(session, epoch)
            || energy
                .owner
                .session_active_speech_ranges(session, epoch, rate)
                .availability()
                .observed_samples()
                != Some(end)
        {
            return false;
        }
        let receipt = OccurrenceIdentity::new(session, epoch, 0, end);
        if self
            .anchor_capture_end
            .as_ref()
            .is_some_and(|held| held != &receipt)
        {
            return false;
        }
        self.anchor_capture_end = Some(receipt);
        true
    }

    /// `None` when no authenticated speech measurement covers this owner:
    /// the ledger then knows nothing about the speech under a declaration.
    /// `Some(empty)` is measured silence under the declared range.
    pub(super) fn measured_word_anchors(
        &mut self,
        owner: &OccurrenceIdentity,
    ) -> Option<Vec<SpeechAnchor>> {
        if let Some(energy) = self.decode_fence_energy.as_ref() {
            let floor = self.anchor_word_state.floor_rms.or_else(|| {
                energy
                    .owner
                    .session_noise_floor_rms(&owner.session, owner.capture_epoch)
                    .filter(|noise| noise.is_finite() && *noise >= 0.0)
                    .map(|noise| {
                        noise.clamp(
                            crate::audio::capture_receipt::DIGITAL_ZERO_ABS,
                            crate::audio::capture_receipt::SESSION_NOISE_FLOOR_CEILING_RMS,
                        ) * 10.0_f32.powf(8.0 / 20.0)
                    })
                    .filter(|floor| floor.is_finite() && *floor > 0.0 && *floor <= 1.0)
            });
            // Bind the measured fallback once too: a later window must not
            // change earlier physical anchor identities by moving its floor.
            self.anchor_word_state.floor_rms = floor;
            self.speech_anchors = floor.zip(self.capture_rate_hz).map(|(floor, rate)| {
                energy.owner.session_speech_anchors(
                    &owner.session,
                    owner.capture_epoch,
                    rate,
                    floor,
                )
            });
        }
        let evidence = self.speech_anchors.as_ref()?;
        let AcousticAvailability::Observed { observed_samples } = &evidence.availability else {
            return None;
        };
        if !evidence
            .identity
            .matches(&owner.session, owner.capture_epoch)
            || self
                .decode_fence_energy
                .as_ref()
                .is_some_and(|energy| *observed_samples < energy.captured_samples)
        {
            return None;
        }
        let anchors = evidence
            .anchors
            .iter()
            .filter(|anchor| {
                anchor.range.session == owner.session
                    && anchor.range.capture_epoch == owner.capture_epoch
                    && anchor.range.sample_start < anchor.range.sample_end
                    && anchor.range.sample_end <= *observed_samples
                    && anchor.range.sample_start < owner.sample_end
                    && owner.sample_start < anchor.range.sample_end
            })
            .cloned()
            .map(|mut anchor| {
                anchor.range.sample_start = anchor.range.sample_start.max(owner.sample_start);
                anchor.range.sample_end = anchor.range.sample_end.min(owner.sample_end);
                if self.anchor_capture_end.as_ref().is_some_and(|receipt| {
                    receipt.same_capture(owner) && receipt.sample_end == *observed_samples
                }) {
                    anchor.open_at_end = false;
                }
                anchor
            })
            .collect();
        Some(anchors)
    }

    /// Without any PCM measurement, a producer's n-word sequence whose tokens
    /// equal, in order, the n word pins another producer already holds over
    /// the same utterance-observed speech is the same sequence: it adopts the
    /// held geometry instead of founding a displaced copy of every word.
    /// Tokens alone never identify speech; the utterance observer's range is
    /// the witness that both sequences lie over one speech span.
    pub(super) fn declared_sequence_geometry<'a>(
        &self,
        observation: &ObservationIdentity,
        words: &'a [WordPin],
    ) -> Option<Vec<(WordSlot, &'a WordPin)>> {
        let owner = &observation.occurrence;
        let speech = self.speech_evidence.as_ref()?;
        if !speech
            .identity()
            .matches(&owner.session, owner.capture_epoch)
            || !speech.is_observed()
        {
            return None;
        }
        let held = self
            .slots_of(owner)?
            .iter()
            .filter(|slot| {
                slot.producer != observation.producer
                    && self.word_pin_observations.contains(&slot.observation)
            })
            .collect::<Vec<_>>();
        if held.len() < 2 || held.len() != words.len() {
            return None;
        }
        let over_speech = |start: u64, end: u64| {
            speech.ranges().iter().any(|range| {
                range.session == owner.session
                    && range.capture_epoch == owner.capture_epoch
                    && range.sample_start < end
                    && start < range.sample_end
            })
        };
        let same_sequence = held.iter().zip(words).all(|(held, pin)| {
            let token = normalize_word_token(&pin.text);
            !token.is_empty()
                && token == normalize_word_token(&held.text)
                && over_speech(held.sample_start, held.sample_end)
                && over_speech(pin.sample_start, pin.sample_end)
        });
        if !same_sequence {
            return None;
        }
        Some(
            held.iter()
                .zip(words)
                .map(|(held, pin)| {
                    (
                        WordSlot {
                            sample_start: held.sample_start,
                            sample_end: held.sample_end,
                            text: pin.text.clone(),
                            producer: observation.producer,
                            observation: observation.clone(),
                            witness: SlotWitness::Unwitnessed,
                            confidence: pin.confidence,
                            surface_rewritten: pin.surface_rewritten,
                        },
                        pin,
                    )
                })
                .collect(),
        )
    }

    pub fn word_anchor_identity(&self, slot: &WordSlot) -> Option<&WordAnchorIdentity> {
        let slot = self.geometry_preserving_word_source(slot)?;
        self.anchor_word_state
            .words
            .iter()
            .rev()
            .find_map(|record| {
                (record.slot.observation == slot.observation
                    && record.slot.sample_start == slot.sample_start
                    && record.slot.sample_end == slot.sample_end)
                    .then_some(&record.identity)
            })
    }

    pub(super) fn original_anchor_word_range(
        &self,
        slot: &WordSlot,
    ) -> Option<&OccurrenceIdentity> {
        let slot = self.geometry_preserving_word_source(slot)?;
        self.anchor_word_state
            .words
            .iter()
            .rev()
            .find_map(|record| {
                (record.slot.observation == slot.observation
                    && record.slot.sample_start == slot.sample_start
                    && record.slot.sample_end == slot.sample_end)
                    .then_some(&record.original)
            })
    }

    /// Exact native input and lexical position, including compound C2 pins.
    /// Adjudication can retrieve raw fences/text without using projection time.
    pub(super) fn word_anchor_source(&self, slot: &WordSlot) -> Option<(&WordPin, usize)> {
        self.anchor_word_state
            .words
            .iter()
            .rev()
            .find_map(|record| {
                (record.slot.observation == slot.observation
                    && record.slot.sample_start == slot.sample_start
                    && record.slot.sample_end == slot.sample_end)
                    .then_some((&record.original_pin, record.lexical_index))
            })
    }

    pub(super) fn same_anchored_word(&self, left: &WordSlot, right: &WordSlot) -> bool {
        let correspondence = |candidate: &WordSlot, source: &WordSlot| {
            self.anchor_word_state
                .words
                .iter()
                .rev()
                .find(|record| {
                    record.slot.observation == candidate.observation
                        && record.slot.sample_start == candidate.sample_start
                        && record.slot.sample_end == candidate.sample_end
                })
                .and_then(|record| record.sequence_source.as_ref())
                .and_then(|target| {
                    (target.observation == source.observation).then_some(target == source)
                })
        };
        if let Some(matches) = correspondence(left, right).or_else(|| correspondence(right, left)) {
            return matches;
        }
        match (
            self.word_anchor_identity(left),
            self.word_anchor_identity(right),
        ) {
            (Some(left), Some(right)) => left.shares_word(right),
            // Only one side was measured against speech. The measured word's
            // burst-clipped span is its geometry; the unmeasured word has only
            // its declared range. They are the same word when those ranges
            // share, over the same capture: a coarse phrase or an Apple word
            // and the Whisper word on the same speech are one word.
            (Some(measured), None) => measured.spans.iter().any(|span| {
                span.same_capture(&right.observation.occurrence)
                    && declared_ranges_share_word(
                        span.sample_start,
                        span.sample_end,
                        right.sample_start,
                        right.sample_end,
                    )
            }),
            (None, Some(measured)) => measured.spans.iter().any(|span| {
                span.same_capture(&left.observation.occurrence)
                    && declared_ranges_share_word(
                        span.sample_start,
                        span.sample_end,
                        left.sample_start,
                        left.sample_end,
                    )
            }),
            // Neither word was measured against speech: the declared ranges
            // are the only geometry either producer has.
            (None, None) => {
                left.observation
                    .occurrence
                    .same_capture(&right.observation.occurrence)
                    && declared_ranges_share_word(
                        left.sample_start,
                        left.sample_end,
                        right.sample_start,
                        right.sample_end,
                    )
            }
        }
    }

    /// Neither word carries a speech measurement: only declared geometry
    /// relates them.
    pub(super) fn unmeasured_pair(&self, left: &WordSlot, right: &WordSlot) -> bool {
        self.word_anchor_identity(left).is_none() && self.word_anchor_identity(right).is_none()
    }

    pub(super) fn same_anchor_assignment(&self, left: &WordSlot, right: &WordSlot) -> bool {
        self.word_anchor_identity(left)
            .zip(self.word_anchor_identity(right))
            .is_some_and(|(left, right)| left == right)
    }

    pub(super) fn project_anchor_decode_bounds(
        &mut self,
        observation: &ObservationIdentity,
        incoming: &[(WordSlot, &WordPin)],
    ) {
        let Some(ranges) = self.complete_decoded_words.get_mut(observation) else {
            return;
        };
        let original = ranges.clone();
        for (word, pin) in incoming {
            let owner = &observation.occurrence;
            if original.contains(&(
                pin.sample_start.max(owner.sample_start),
                pin.sample_end.min(owner.sample_end),
            )) {
                ranges.push((word.sample_start, word.sample_end));
            }
        }
    }

    /// Retain the complete correspondence used to project a declared sequence.
    /// Per-word corrections alone lose the joint provenance of that decision.
    pub(super) fn record_sequence_binding(
        &mut self,
        observation: &ObservationIdentity,
        binding: Option<&(Vec<WordSlot>, Vec<WordSlot>)>,
        pins: &[WordPin],
    ) {
        let Some((sources, projected)) = binding else {
            return;
        };
        let outputs = self.slots_of(&observation.occurrence).unwrap_or(&[]);
        if outputs.len() != projected.len()
            || !outputs.iter().zip(projected).all(|(actual, expected)| {
                actual.sample_start == expected.sample_start
                    && actual.sample_end == expected.sample_end
                    && normalize_word_token(&actual.text) == normalize_word_token(&expected.text)
            })
        {
            return;
        }
        let outputs = outputs.to_vec();
        let mut source_ranges = sources
            .iter()
            .flat_map(|source| self.slot_source_ranges(source))
            .collect::<Vec<_>>();
        source_ranges.extend(pins.iter().map(|pin| {
            OccurrenceIdentity::new(
                &observation.occurrence.session,
                observation.occurrence.capture_epoch,
                pin.sample_start,
                pin.sample_end,
            )
        }));
        source_ranges.sort();
        source_ranges.dedup();
        self.record_slot_operation(SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Correct,
            sources: sources.clone(),
            outputs,
            source_ranges,
            rule_id: "declared_sequence_correspondence/v2".into(),
        });
    }

    /// Use the existing slot-operation receipt chain for the normalized
    /// projection's native source. No producer range is rewritten in place.
    pub(super) fn record_anchor_normalizations(&mut self, observation: &ObservationIdentity) {
        let slots = self
            .slots_of(&observation.occurrence)
            .unwrap_or(&[])
            .to_vec();
        for slot in slots.iter().filter(|slot| slot.observation == *observation) {
            let Some(original) = self.original_anchor_word_range(slot).cloned() else {
                continue;
            };
            let owner = &observation.occurrence;
            if slot.sample_start == original.sample_start.max(owner.sample_start)
                && slot.sample_end == original.sample_end.min(owner.sample_end)
            {
                // Declared geometry kept: nothing was normalized.
                continue;
            }
            let mut source_ranges = self.slot_source_ranges(slot);
            if !source_ranges.contains(&original) {
                source_ranges.push(original);
            }
            self.record_slot_operation(SlotOperationReceipt {
                observation: observation.clone(),
                kind: SlotOperationKind::Correct,
                sources: vec![slot.clone()],
                outputs: vec![slot.clone()],
                source_ranges,
                rule_id: "speech_anchor_normalization/v2".into(),
            });
        }
    }

    /// Normalize a complete producer sequence before admission. Projection
    /// coordinates divide measured speech for painting/ordering only; identity
    /// is the anchor set plus ordinal, never these arithmetic subdivisions.
    pub(super) fn normalize_anchor_words<'a>(
        &mut self,
        observation: &ObservationIdentity,
        pins: &'a [WordPin],
        anchors: &[SpeechAnchor],
    ) -> Option<Vec<(WordSlot, &'a WordPin)>> {
        let owner = &observation.occurrence;
        let anchors = anchors
            .iter()
            .map(|anchor| {
                OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    anchor.range.sample_start,
                    anchor.range.sample_end,
                )
            })
            .collect::<Vec<_>>();
        let pins = pins
            .iter()
            .flat_map(|pin| {
                let token_count = pin.text.split_whitespace().count();
                pin.text
                    .split_whitespace()
                    .enumerate()
                    .map(move |(lexical_index, text)| LexicalAnchorPin {
                        original: pin,
                        lexical_index,
                        token_count,
                        text,
                    })
            })
            .collect::<Vec<_>>();
        if anchors.is_empty() || pins.is_empty() {
            return None;
        }
        let prior = self.slots_of(owner).unwrap_or(&[]).to_vec();
        let anchored_prior = prior
            .iter()
            .filter(|word| self.word_anchor_identity(word).is_some())
            .collect::<Vec<_>>();
        // One producer's later window may repeat or revise an overlapping
        // claim. A disjoint range from that producer is a new occurrence,
        // even when both words have the same spelling and share one burst.
        let same_producer_scope = |held: &WordSlot, pin: &LexicalAnchorPin<'_>| {
            held.producer != observation.producer
                || self.original_anchor_word_range(held).is_some_and(|range| {
                    range.sample_start < pin.sample_end && pin.sample_start < range.sample_end
                })
        };
        // A bijection over the same owner's measured speech sequence survives
        // arbitrary Apple clock displacement, in either arrival order. This is
        // sequence alignment, not a token set: repeated positions stay distinct.
        let shares_burst = |held: &WordSlot, pin: &LexicalAnchorPin<'_>| {
            let (pin_start, pin_end) = pin.declared();
            self.word_anchor_identity(held).is_some_and(|identity| {
                identity.anchors.iter().any(|held_anchor| {
                    anchors.iter().any(|anchor| {
                        held_anchor.same_capture(anchor)
                            && held_anchor.sample_start < anchor.sample_end
                            && anchor.sample_start < held_anchor.sample_end
                            && anchor.sample_start < pin_end
                            && pin_start < anchor.sample_end
                    })
                })
            })
        };
        let sequence = anchored_prior.len() == pins.len()
            && anchored_prior.iter().zip(&pins).all(|(held, pin)| {
                normalize_word_token(&held.text) == normalize_word_token(pin.text)
                    && same_producer_scope(held, pin)
                    && shares_burst(held, pin)
            });
        // Align partial callbacks monotonically as well. A producer may omit
        // an opening/function word; uniformly spreading that shorter phrase
        // over the whole owner would move its last word onto later speech.
        let mut correspondence = vec![None; pins.len()];
        if sequence {
            for (index, held) in anchored_prior.iter().enumerate() {
                correspondence[index] = Some(*held);
            }
        } else {
            // Bound uncertain alignment work; retaining the full phrase is
            // preferable to an unbounded allocation on a malformed callback.
            let cells = (anchored_prior.len() + 1).checked_mul(pins.len() + 1)?;
            if cells > 1_048_576 {
                return None;
            }
            let held_tokens = anchored_prior
                .iter()
                .map(|word| normalize_word_token(&word.text))
                .collect::<Vec<_>>();
            let offered_tokens = pins
                .iter()
                .map(|word| normalize_word_token(word.text))
                .collect::<Vec<_>>();
            let held_ranges = anchored_prior
                .iter()
                .map(|word| self.original_anchor_word_range(word).cloned())
                .collect::<Vec<_>>();
            let mut held_counts = BTreeMap::new();
            let mut offered_counts = BTreeMap::new();
            for token in &held_tokens {
                *held_counts.entry(token.as_str()).or_insert(0_usize) += 1;
            }
            for token in &offered_tokens {
                *offered_counts.entry(token.as_str()).or_insert(0_usize) += 1;
            }
            let tolerance = u64::from(self.capture_rate_hz?);
            let matches = |i: usize, j: usize| {
                let token = held_tokens[i].as_str();
                token == offered_tokens[j]
                    && same_producer_scope(anchored_prior[i], &pins[j])
                    && shares_burst(anchored_prior[i], &pins[j])
                    && (held_counts[token] == 1
                        || offered_counts.get(token) == held_counts.get(token))
                    && held_ranges[i].as_ref().is_some_and(|range| {
                        range.sample_start <= pins[j].sample_end.saturating_add(tolerance)
                            && pins[j].sample_start <= range.sample_end.saturating_add(tolerance)
                    })
            };
            let mut lengths = vec![vec![0_usize; pins.len() + 1]; anchored_prior.len() + 1];
            for i in (0..anchored_prior.len()).rev() {
                for j in (0..pins.len()).rev() {
                    lengths[i][j] = if matches(i, j) {
                        1 + lengths[i + 1][j + 1]
                    } else {
                        lengths[i + 1][j].max(lengths[i][j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < anchored_prior.len() && j < pins.len() {
                if matches(i, j) && lengths[i][j] == 1 + lengths[i + 1][j + 1] {
                    correspondence[j] = Some(anchored_prior[i]);
                    i += 1;
                    j += 1;
                } else if lengths[i + 1][j] > lengths[i][j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        // A partial Apple phrase has no independently timed unmatched words.
        // Retain it whole as phrase evidence instead of planting its unmatched
        // spelling on an arbitrary later anchor. Matching complete sequences
        // and isolated disputed words take the explicit paths below.
        if observation.producer == ObservationProducer::Apple
            && pins.len() > 1
            && !anchored_prior.is_empty()
            && !sequence
            && correspondence.iter().any(Option::is_none)
        {
            return None;
        }
        // A later decoder frame may resolve one broad claim into several
        // words. Its matching final token must not inherit that entire claim
        // and overlap its newly resolved siblings. Leave the partition to
        // ordinary group adjudication using the original measured pins.
        if observation.producer == ObservationProducer::Whisper && !sequence {
            for held in &mut correspondence {
                if held.is_some_and(|source| {
                    self.original_anchor_word_range(source)
                        .is_some_and(|range| {
                            pins.iter()
                                .filter(|pin| {
                                    range.sample_start <= pin.sample_start
                                        && pin.sample_end <= range.sample_end
                                })
                                .count()
                                > 1
                        })
                }) {
                    *held = None;
                }
            }
        }
        let mut identities = Vec::new();
        if sequence && observation.producer == ObservationProducer::Apple {
            identities.extend(anchored_prior.iter().map(|held| {
                self.word_anchor_identity(held)
                    .expect("anchored sequence source")
                    .clone()
            }));
        } else if observation.producer == ObservationProducer::Apple && pins.len() == 1 {
            let pin = &pins[0];
            let tolerance = u64::from(self.capture_rate_hz?);
            let matches = anchored_prior
                .iter()
                .filter(|held| {
                    normalize_word_token(&held.text) == normalize_word_token(pin.text)
                        && self.original_anchor_word_range(held).is_some_and(|range| {
                            range.sample_start <= pin.sample_end.saturating_add(tolerance)
                                && pin.sample_start <= range.sample_end.saturating_add(tolerance)
                        })
                })
                .collect::<Vec<_>>();
            if let [held] = matches.as_slice() {
                identities.push(
                    self.word_anchor_identity(held)
                        .expect("anchored isolated source")
                        .clone(),
                );
            } else if !anchored_prior.is_empty() {
                // An isolated unreliable phrase timestamp cannot reserve a new
                // ordinal on occupied speech. Keep the full hypothesis as
                // unanchored evidence rather than inventing a second word.
                let claimed = anchors
                    .iter()
                    .filter(|anchor| {
                        declared_range_contains_midpoint(
                            pin.sample_start,
                            pin.sample_end,
                            anchor.sample_start,
                            anchor.sample_end,
                        ) || declared_range_contains_midpoint(
                            anchor.sample_start,
                            anchor.sample_end,
                            pin.sample_start,
                            pin.sample_end,
                        )
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let occupied = anchored_prior
                    .iter()
                    .filter(|held| {
                        self.word_anchor_identity(held).is_some_and(|identity| {
                            identity
                                .anchors
                                .iter()
                                .any(|anchor| claimed.contains(anchor))
                        })
                    })
                    .collect::<Vec<_>>();
                if matches.is_empty()
                    && let [held] = occupied.as_slice()
                {
                    // A different spelling at one occupied acoustic word is a
                    // lexical dispute, not an additional word or a replay.
                    identities.push(
                        self.word_anchor_identity(held)
                            .expect("occupied anchored source")
                            .clone(),
                    );
                } else if matches.is_empty() && occupied.is_empty() && !claimed.is_empty() {
                    identities.push(WordAnchorIdentity::from_anchors(
                        claimed,
                        (pin.sample_start, pin.sample_end),
                    ));
                } else {
                    return None;
                }
            }
        }
        if identities.is_empty() {
            let mut sets = pins
                .iter()
                .enumerate()
                .map(|(index, pin)| {
                    if observation.producer == ObservationProducer::Apple {
                        // Apple supplies phrase grain: monotonic sequence assignment
                        // inside the native owner, never its unreliable word clocks.
                        let start = index * anchors.len() / pins.len();
                        let end = ((index + 1) * anchors.len() / pins.len()).max(start + 1);
                        anchors[start..end].to_vec()
                    } else {
                        anchors
                            .iter()
                            .filter(|anchor| {
                                // A measured voiced intersection is evidence even
                                // when decoder padding moves the pin midpoint
                                // into silence. Keep the declared span separately.
                                pin.sample_start < anchor.sample_end
                                    && anchor.sample_start < pin.sample_end
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    }
                })
                .collect::<Vec<_>>();
            // A function word in a valley shares the nearest witnessed member
            // of this same sequence. Sharing never deletes a token or repeats.
            for index in 0..sets.len() {
                if !sets[index].is_empty() {
                    continue;
                }
                if !matches!(
                    normalize_word_token(pins[index].text).as_str(),
                    "w" | "z"
                        | "i"
                        | "a"
                        | "o"
                        | "u"
                        | "do"
                        | "na"
                        | "za"
                        | "po"
                        | "od"
                        | "ze"
                        | "we"
                        | "to"
                        | "że"
                        | "nie"
                        | "się"
                        | "the"
                        | "of"
                        | "in"
                        | "at"
                        | "and"
                        | "on"
                        | "as"
                        | "or"
                        | "if"
                        | "is"
                        | "it"
                ) {
                    // No speech assignment for a lexical word is uncertainty,
                    // not permission to mint it on a neighbouring burst.
                    return None;
                }
                let nearest = (0..sets.len())
                    .filter(|other| !sets[*other].is_empty())
                    .min_by_key(|other| other.abs_diff(index));
                let nearest = nearest?;
                sets[index] = sets[nearest].clone();
            }
            for (set, pin) in sets.into_iter().zip(&pins) {
                identities.push(WordAnchorIdentity::from_anchors(set, pin.declared()));
            }
        }
        for (index, held) in correspondence.iter().enumerate() {
            if let Some(held) = held {
                // Apple's phrase assignment is provisional. A measured
                // Whisper sequence refines its physical membership while the
                // explicit correspondence still retires the exact old target.
                if observation.producer != ObservationProducer::Whisper
                    || held.producer != ObservationProducer::Apple
                {
                    identities[index] = self
                        .word_anchor_identity(held)
                        .expect("aligned acoustic source")
                        .clone();
                }
            }
        }
        let mut output = Vec::new();
        for (index, pin) in pins.iter().enumerate() {
            let held = if correspondence[index].is_some() {
                correspondence[index]
            } else if sequence {
                Some(anchored_prior[index])
            } else {
                None
            };
            // A declared range is the producer's word geometry. Only an
            // aligned source of the same acoustic word lends its geometry,
            // and a measured Whisper word never inherits Apple's clock.
            let inherited_geometry = held.filter(|source| {
                observation.producer != ObservationProducer::Whisper
                    || source.producer != ObservationProducer::Apple
            });
            let (sample_start, sample_end) = inherited_geometry.map_or_else(
                || {
                    let (start, end) = pin.declared();
                    (start.max(owner.sample_start), end.min(owner.sample_end))
                },
                |held| (held.sample_start, held.sample_end),
            );
            let slot = WordSlot {
                sample_start,
                sample_end,
                text: pin.text.to_string(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                confidence: (pin.original.text.split_whitespace().count() == 1)
                    .then_some(pin.confidence)
                    .flatten(),
                surface_rewritten: pin.surface_rewritten,
            };
            let original = OccurrenceIdentity::new(
                &owner.session,
                owner.capture_epoch,
                pin.sample_start,
                pin.sample_end,
            );
            self.anchor_word_state.words.push(AnchorWordRecord {
                slot: slot.clone(),
                identity: identities[index].clone(),
                original,
                original_pin: pin.original.clone(),
                lexical_index: pin.lexical_index,
                sequence_source: correspondence[index].map(|held| (*held).clone()),
            });
            output.push((slot, pin.original));
        }
        Some(output)
    }
}
