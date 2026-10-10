//! Explicit operations address a source observation and exact PCM pins.
//! Dictionary rules authorize a merge; timed child pins authorize a split.

use super::*;

/// Align labels inside one PCM group; lexical matching never creates a pin.
/// An unpaired held token preserves the entire group at its existing accuracy.
pub(super) fn preserve_group_content(
    held: &str,
    candidate: &str,
) -> Result<(String, bool), &'static str> {
    const AMBIGUOUS: &str = "group_alignment_ambiguous";

    fn tokens(text: &str) -> Vec<&str> {
        let mut result = Vec::new();
        let mut start = None;
        let mut in_tag = false;
        for (offset, ch) in text.char_indices() {
            if ch.is_whitespace() && !in_tag {
                if let Some(begin) = start.take() {
                    result.push(&text[begin..offset]);
                }
            } else {
                start.get_or_insert(offset);
                if ch == '[' {
                    in_tag = true;
                }
                if ch == ']' {
                    in_tag = false;
                }
            }
        }
        if let Some(begin) = start {
            result.push(&text[begin..]);
        }
        result
    }

    fn normalize(token: &str) -> String {
        token
            .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '[' && ch != ']')
            .to_lowercase()
    }

    fn event(token: &str) -> bool {
        let token = normalize(token);
        token == "yyy" || (token.starts_with('[') && token.ends_with(']'))
    }

    fn similar(left: &str, right: &str) -> bool {
        if event(left) || event(right) {
            return false;
        }
        let left = normalize(left).chars().collect::<Vec<_>>();
        let right = normalize(right).chars().collect::<Vec<_>>();
        let longest = left.len().max(right.len());
        if longest == 0 || longest > 128 {
            return false;
        }
        let budget = longest / 3;
        if left.len().abs_diff(right.len()) > budget {
            return false;
        }
        let mut row = (0..=right.len()).collect::<Vec<_>>();
        for (i, a) in left.iter().enumerate() {
            let mut diagonal = row[0];
            row[0] = i + 1;
            for (j, b) in right.iter().enumerate() {
                let previous = row[j + 1];
                row[j + 1] = (diagonal + usize::from(a != b))
                    .min(row[j] + 1)
                    .min(previous + 1);
                diagonal = previous;
            }
        }
        row[right.len()] <= budget
    }

    fn gap<'a>(
        held: &[&'a str],
        candidate: &[&'a str],
        output: &mut Vec<&'a str>,
        retained: &mut bool,
    ) -> Result<(), &'static str> {
        if held.is_empty() {
            output.extend_from_slice(candidate);
            return Ok(());
        }
        if candidate.is_empty() || candidate.iter().all(|token| event(token)) {
            output.extend_from_slice(held);
            output.extend_from_slice(candidate);
            *retained = true;
            return Ok(());
        }
        let lexical = candidate
            .iter()
            .enumerate()
            .filter(|(_, token)| !event(token))
            .collect::<Vec<_>>();
        if held.len() == 1 && !event(held[0]) && lexical.len() == 1 {
            output.extend_from_slice(candidate);
            return Ok(());
        }
        // Only a mutual, unique spelling match can partition a larger gap.
        // Recurse around it so unpaired held tokens keep their document place.
        let mut pairs = Vec::new();
        for (i, token) in held.iter().enumerate() {
            let matches = lexical
                .iter()
                .filter(|(_, other)| similar(token, other))
                .collect::<Vec<_>>();
            if let [matched] = matches.as_slice() {
                let (j, other) = **matched;
                if held.iter().filter(|prior| similar(prior, other)).count() == 1 {
                    pairs.push((i, j));
                }
            }
        }
        if pairs.windows(2).any(|pair| pair[0].1 >= pair[1].1) {
            return Err(AMBIGUOUS);
        }
        if let Some(&(i, j)) = pairs.first() {
            gap(&held[..i], &candidate[..j], output, retained)?;
            output.push(candidate[j]);
            gap(&held[i + 1..], &candidate[j + 1..], output, retained)?;
            return Ok(());
        }
        Err(AMBIGUOUS)
    }

    let held_label = held;
    let held = tokens(held);
    let candidate = tokens(candidate);
    // Keep work bounded for a malformed or occurrence-wide producer payload.
    if held.len() > 256 || candidate.len() > 256 {
        return Err(AMBIGUOUS);
    }
    let held_keys = held
        .iter()
        .map(|token| normalize(token))
        .collect::<Vec<_>>();
    let candidate_keys = candidate
        .iter()
        .map(|token| normalize(token))
        .collect::<Vec<_>>();
    let mut lengths = vec![vec![0; candidate.len() + 1]; held.len() + 1];
    for i in (0..held.len()).rev() {
        for j in (0..candidate.len()).rev() {
            lengths[i][j] = if !held_keys[i].is_empty() && held_keys[i] == candidate_keys[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    // A repeated anchor in a mixed label may name different gaps. Do not
    // choose a physical word's form from whichever LCS path was visited first.
    // Uniform repetitions have no competing labels; omitted copies stay.
    let uniform_repetition = held_keys.first().is_some_and(|key| {
        !key.is_empty()
            && held_keys
                .iter()
                .chain(&candidate_keys)
                .all(|other| other == key)
    });
    if !uniform_repetition {
        let mut prefixes = vec![vec![0; candidate.len() + 1]; held.len() + 1];
        for i in 0..held.len() {
            for (j, key) in candidate_keys.iter().enumerate() {
                prefixes[i + 1][j + 1] = if !held_keys[i].is_empty() && &held_keys[i] == key {
                    prefixes[i][j] + 1
                } else {
                    prefixes[i][j + 1].max(prefixes[i + 1][j])
                };
            }
        }
        for (j, key) in candidate_keys.iter().enumerate() {
            let possible_sources = held_keys
                .iter()
                .enumerate()
                .filter(|(i, held_key)| {
                    !key.is_empty()
                        && *held_key == key
                        && prefixes[*i][j] + 1 + lengths[*i + 1][j + 1] == lengths[0][0]
                })
                .count();
            if possible_sources > 1 {
                return Err(AMBIGUOUS);
            }
        }
    }
    let mut output = Vec::new();
    let mut retained = false;
    let (mut i, mut j, mut held_start, mut candidate_start) = (0, 0, 0, 0);
    while i < held.len() && j < candidate.len() {
        if !held_keys[i].is_empty() && held_keys[i] == candidate_keys[j] {
            gap(
                &held[held_start..i],
                &candidate[candidate_start..j],
                &mut output,
                &mut retained,
            )?;
            output.push(candidate[j]);
            i += 1;
            j += 1;
            held_start = i;
            candidate_start = j;
        } else if lengths[i + 1][j] >= lengths[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    gap(
        &held[held_start..],
        &candidate[candidate_start..],
        &mut output,
        &mut retained,
    )?;
    Ok((
        if retained {
            held_label.to_owned()
        } else {
            output.join(" ")
        },
        retained,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SlotTarget {
    pub observation: ObservationIdentity,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl From<&WordSlot> for SlotTarget {
    fn from(slot: &WordSlot) -> Self {
        Self {
            observation: slot.observation.clone(),
            sample_start: slot.sample_start,
            sample_end: slot.sample_end,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOperationKind {
    Correct,
    Insert,
    Merge,
    Split,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotOperationReceipt {
    pub observation: ObservationIdentity,
    pub kind: SlotOperationKind,
    pub sources: Vec<WordSlot>,
    pub outputs: Vec<WordSlot>,
    /// Exact source ranges, including gaps. Never a merge's bounding box.
    pub source_ranges: Vec<OccurrenceIdentity>,
    pub rule_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct AssignedWordPinBatch {
    pub observation: ObservationIdentity,
    pub assignments: Vec<(OccurrenceIdentity, OccurrenceIdentity)>,
}

/// Speech intervals and the exact pins that justified a group refinement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechPinCoverage {
    pub speech_range: OccurrenceIdentity,
    pub pin_range: OccurrenceIdentity,
    pub pin_owner: OccurrenceIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpeechCoverageReceipt {
    pub operation: SlotOperationReceipt,
    pub speech: AcousticSpeechEvidence,
    pub coverage: Vec<SpeechPinCoverage>,
    pub rule_version: &'static str,
}

/// A particular Dictionary rule, resolved before the operation is proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionarySlotRule {
    pub id: String,
    pub input: Vec<String>,
    pub canonical: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOperationRefusal {
    StaleTarget,
    ProtectedHuman,
    Sealed,
    ReplayedObservation,
    InvalidEvidence,
    ProducerNotAuthorized,
}

/// A repetition picker is needed only for equal lexical evidence on the same
/// PCM. Different words over a coarse source are a partition, not a replay.
pub(super) fn repetition_target_ambiguous(sources: &[WordSlot], pins: &[WordSlot]) -> bool {
    pins.iter().any(|pin| {
        let key = normalize_word_token(&pin.text);
        !key.is_empty()
            && (sources
                .iter()
                .filter(|source| {
                    same_pcm_slot(source, pin) && normalize_word_token(&source.text) == key
                })
                .count()
                > 1
                || sources.iter().any(|source| {
                    same_pcm_slot(source, pin)
                        && pins
                            .iter()
                            .filter(|other| {
                                same_pcm_slot(source, other)
                                    && normalize_word_token(&other.text) == key
                            })
                            .count()
                            > 1
                }))
    })
}

impl AcousticLedger {
    /// Current and predecessor ranges are physical targets; labels never
    /// broaden this relation. A split child addresses its own range only.
    pub(super) fn pin_targets_source(&self, source: &WordSlot, pin: &WordSlot) -> bool {
        same_pcm_slot(source, pin)
            || self.slot_source_ranges(source).iter().any(|range| {
                range.same_capture(&pin.observation.occurrence)
                    && same_pcm_slot(
                        &WordSlot {
                            sample_start: range.sample_start,
                            sample_end: range.sample_end,
                            ..source.clone()
                        },
                        pin,
                    )
            })
    }

    pub(crate) fn word_slot_targets_pin(
        &self,
        source: &WordSlot,
        pin: &OccurrenceIdentity,
    ) -> bool {
        self.word_pin_observations.contains(&source.observation)
            && source.text.split_whitespace().count() == 1
            && pin.is_anchored()
            && pin.same_capture(&source.observation.occurrence)
            && self.pin_targets_source(
                source,
                &WordSlot {
                    sample_start: pin.sample_start,
                    sample_end: pin.sample_end,
                    ..source.clone()
                },
            )
    }

    /// Coarse ASR labels carry one unresolved group, not independent word claims.
    /// Their existing measured partition rules still own refinement.
    pub(super) fn coarse_word_source(&self, source: &WordSlot) -> bool {
        !self.word_pin_observations.contains(&source.observation)
            || (source.text.contains(char::is_whitespace)
                && self.slot_source_ranges(source).len() == 1)
    }

    pub(crate) fn complete_word_slot(&self, source: &WordSlot) -> bool {
        self.word_pin_observations.contains(&source.observation)
            && self
                .complete_decoded_words
                .get(&source.observation)
                .is_some_and(|ranges| ranges.contains(&(source.sample_start, source.sample_end)))
    }

    fn slot_descends_from(&self, pin: &WordSlot, source: &WordSlot) -> bool {
        let mut descendants = vec![source.clone()];
        for operation in &self.slot_operations {
            if operation
                .sources
                .iter()
                .any(|held| descendants.contains(held))
            {
                for output in &operation.outputs {
                    if !descendants.contains(output) {
                        descendants.push(output.clone());
                    }
                }
            }
        }
        descendants.contains(pin)
    }

    /// Authenticate the existing acoustic observer for a bounded PCM scope.
    fn measured_speech_for(&self, source: &OccurrenceIdentity) -> Option<&AcousticSpeechEvidence> {
        use crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER;
        use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

        let speech = self.speech_evidence.as_ref()?;
        let observed = speech.availability().observed_samples()?;
        (source.is_anchored()
            && speech
                .identity()
                .matches(&source.session, source.capture_epoch)
            && matches!(
                speech.producer(),
                CAPTURE_ENERGY_PRODUCER | SILERO_BOUNDARIES_PRODUCER
            )
            && observed >= source.sample_end
            && speech.ranges().iter().all(|range| {
                let range = OccurrenceIdentity::from(range);
                range.same_capture(source) && range.is_anchored() && range.sample_end <= observed
            }))
        .then_some(speech)
    }

    /// Whisper Word alignment uses 20 ms encoder frames (word_pins.rs),
    /// converted onto the capture clock with at most one sample of rounding.
    /// This uncertainty is geometry, never a wait or an extension of issued PCM.
    pub(crate) fn decode_word_fence_incomplete(
        &self,
        owner: &OccurrenceIdentity,
        start: u64,
        end: u64,
        word_start: u64,
        word_end: u64,
    ) -> bool {
        if start >= end || word_start < start || word_start >= word_end || word_end > end {
            return true;
        }
        let resolution = u64::from(self.capture_rate_hz.unwrap_or(16_000))
            .div_ceil(50)
            .saturating_add(1);
        if end - word_end > resolution {
            return false;
        }
        let window = OccurrenceIdentity::new(&owner.session, owner.capture_epoch, start, end);
        let Some(speech) = self.measured_speech_for(&window) else {
            // Keep the exact-edge safeguard. Missing measurement cannot
            // establish that speech crossed a strictly interior Word fence.
            return word_end == end;
        };
        let crosses = speech
            .ranges()
            .iter()
            .any(|range| range.sample_start < word_end && range.sample_end >= end);
        if !crosses {
            return false;
        }
        let Some(energy) = &self.decode_fence_energy else {
            return true;
        };
        if energy.captured_samples < end {
            return true;
        }
        // Clipping a classified capture block does not measure its quiet end.
        // Authenticate the writer over the entire bounded uncertainty before
        // consulting the exact retained PCM receipt. A finite downstream view
        // cannot overrule missing, foreign, invalid or discontinuous capture.
        let edge_start = end.saturating_sub(resolution).max(start);
        if energy
            .owner
            .voiced_hops_in(&window.session, window.capture_epoch, edge_start, end)
            .is_none()
        {
            return true;
        }
        !energy.quiet_range.as_ref().is_some_and(|quiet| {
            quiet.same_capture(&window)
                && quiet.sample_start == edge_start
                && quiet.sample_end == end
                && quiet.sample_end - quiet.sample_start == resolution
        })
    }

    /// An empty final can account only for measured quiet or speech already
    /// covered by accepted Word/source evidence. It supplies no lexical pin.
    fn empty_decode_scope_accounted(
        &self,
        observation: &ObservationIdentity,
        start: u64,
        end: u64,
    ) -> bool {
        let owner = &observation.occurrence;
        let window = OccurrenceIdentity::new(&owner.session, owner.capture_epoch, start, end);
        let Some(speech) = self.measured_speech_for(&window) else {
            return false;
        };
        let mut accounted = Vec::new();
        for (held_owner, committed) in &self.committed {
            if !held_owner.same_capture(owner) {
                continue;
            }
            for pin in &committed.slots {
                if !self.word_pin_observations.contains(&pin.observation)
                    || pin.producer.authority_rank() < observation.producer.authority_rank()
                    || !self.complete_word_slot(pin)
                {
                    continue;
                }
                accounted.extend(
                    self.slot_source_ranges(pin)
                        .into_iter()
                        .map(|range| (range.sample_start, range.sample_end)),
                );
                // Preserve sparse Word timestamp accounting from an accepted
                // complete decode, but never lend a rejected/partial scope.
                if !self.rejected_word_pins.contains_key(held_owner)
                    && committed
                        .slots
                        .iter()
                        .all(|held| self.complete_word_slot(held))
                    && let Some(&(lo, hi)) = self.decoded_word_windows.get(&pin.observation)
                {
                    accounted.push((lo, hi));
                }
            }
        }
        accounted.sort_unstable();
        speech.ranges().iter().all(|range| {
            let mut cursor = range.sample_start.max(start);
            let limit = range.sample_end.min(end);
            for &(lo, hi) in &accounted {
                if lo <= cursor && hi > cursor {
                    cursor = hi;
                }
            }
            cursor >= limit
        })
    }

    /// Called only for the exact launched, matched successful empty final.
    /// Preserve its work identity even before accepted Words account for its
    /// measured speech. The return value describes current scope accounting.
    pub(crate) fn record_empty_decode_work(
        &mut self,
        observation: &ObservationIdentity,
        decode: &OccurrenceIdentity,
    ) -> bool {
        let owner = &observation.occurrence;
        if observation.producer != ObservationProducer::Whisper
            || !decode.same_capture(owner)
            || !self.is_qualified(owner)
            || self.is_sealed(owner)
            || self.answered.contains(observation)
            || self.successful_empty_decodes.contains(observation)
            || self.decoded_word_windows.contains_key(observation)
            || decode.sample_start >= owner.sample_end
            || decode.sample_end <= owner.sample_start
            || self.measured_speech_for(decode).is_none()
        {
            return false;
        }
        self.successful_empty_decodes.insert(observation.clone());
        let history = self
            .owner_history
            .entry(owner.clone())
            .or_default()
            .producers
            .entry(observation.producer)
            .or_default();
        Self::reserve_generation(history, observation.generation);
        self.decoded_word_windows.insert(
            observation.clone(),
            (decode.sample_start, decode.sample_end),
        );
        let accounted =
            self.empty_decode_scope_accounted(observation, decode.sample_start, decode.sample_end);
        super::super::trail::record_decode_work(self, observation, decode, accounted);
        if !accounted {
            self.require_text_recovery(owner);
        }
        self.reconcile_returned_word_debt(owner);
        accounted
    }

    /// Authenticate the exact recorded work scope independently of accounting.
    pub(crate) fn has_returned_decode_work(
        &self,
        observation: &ObservationIdentity,
        decode: &OccurrenceIdentity,
    ) -> bool {
        decode.same_capture(&observation.occurrence)
            && self.successful_empty_decodes.contains(observation)
            && self.decoded_word_windows.get(observation)
                == Some(&(decode.sample_start, decode.sample_end))
    }

    /// Estimated word fences do not decide whether the decoded PCM cut speech.
    /// An accepted window with measured silent margins can account for work
    /// without declaring its edge-timed words complete. Its committed outputs
    /// must still account for every measured speech range inside that window.
    fn decoded_window_speech_accounted(
        &self,
        observation: &ObservationIdentity,
        start: u64,
        end: u64,
    ) -> bool {
        let owner = &observation.occurrence;
        if start >= end
            || !self.is_qualified(owner)
            || !self.word_pin_observations.contains(observation)
            || !matches!(
                observation.producer,
                ObservationProducer::Whisper | ObservationProducer::CloudLive
            )
        {
            return false;
        }
        let held = self.slots_of(owner).unwrap_or(&[]);
        let outputs = self
            .slot_operations
            .iter()
            .filter(|operation| &operation.observation == observation)
            .flat_map(|operation| &operation.outputs)
            .filter(|output| {
                &output.observation == observation
                    && output.text.split_whitespace().count() == 1
                    && start <= output.sample_start
                    && output.sample_end <= end
                    && held.iter().any(|pin| {
                        self.word_pin_observations.contains(&pin.observation)
                            && pin.text.split_whitespace().count() == 1
                            && pin.producer.authority_rank()
                                >= observation.producer.authority_rank()
                            && (pin == *output || self.slot_descends_from(pin, output))
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let window = OccurrenceIdentity::new(&owner.session, owner.capture_epoch, start, end);
        let Some((speech, _)) = self.speech_pin_coverage(observation, &window, &outputs) else {
            return false;
        };
        // Coverage authenticates identity, producer, contiguous measurement
        // and extent. Require positive silence at both actual decode fences;
        // a voiced range touching either fence supplies no silent margin.
        speech.ranges().iter().all(|range| {
            range.sample_end < start
                || range.sample_start > end
                || (start < range.sample_start && range.sample_end < end)
        })
    }

    /// Account decoder work over a source using accepted window receipts.
    /// A frontier return may be a timeout; it is never a successful decode.
    fn decoded_source_scope_accounted(
        &self,
        observation: &ObservationIdentity,
        source: &OccurrenceIdentity,
    ) -> bool {
        if !source.same_capture(&observation.occurrence) || !source.is_anchored() {
            return false;
        }
        let mut windows = self
            .decoded_word_windows
            .iter()
            .filter(|(candidate, (start, end))| {
                if candidate.occurrence != observation.occurrence
                    || candidate.producer != observation.producer
                {
                    return false;
                }
                if self.successful_empty_decodes.contains(*candidate) {
                    return self.empty_decode_scope_accounted(candidate, *start, *end);
                }
                (self.word_pin_observations.contains(*candidate)
                    || self.confirmed_word_decode(candidate, *start, *end))
                    && (self
                        .complete_decoded_words
                        .get(*candidate)
                        .is_some_and(|pins| !pins.is_empty())
                        || self.decoded_window_speech_accounted(candidate, *start, *end))
            })
            .map(|(_, window)| *window)
            .collect::<Vec<_>>();
        windows.sort_unstable();
        let mut cursor = source.sample_start;
        for (start, end) in windows {
            if start <= cursor && end > cursor {
                cursor = end;
            }
        }
        cursor >= source.sample_end
    }

    /// A confirmed trial also authenticates its earlier complete corroborating
    /// decode. The earlier label refusal stays in the trail; only its exact
    /// decoder work can account for recovery, never another PCM identity.
    fn confirmed_word_decode(
        &self,
        observation: &ObservationIdentity,
        start: u64,
        end: u64,
    ) -> bool {
        self.word_choices().iter().any(|choice| {
            choice.observation.occurrence == observation.occurrence
                && choice.accepted
                && choice.lexical_resolved
                && choice.reason == "trial_confirmed"
                && self.word_pin_observations.contains(&choice.observation)
                && self.slot_operations().iter().any(|operation| {
                    operation.observation == choice.observation
                        && operation
                            .sources
                            .iter()
                            .map(SlotTarget::from)
                            .collect::<Vec<_>>()
                            == choice.targets
                        && choice
                            .source_ranges
                            .iter()
                            .all(|range| operation.source_ranges.contains(range))
                        && operation.outputs.len() == choice.candidate.pins.len()
                        && operation.outputs.iter().zip(&choice.candidate.pins).all(
                            |(output, pin)| {
                                output.observation == choice.observation
                                    && output.sample_start == pin.sample_start
                                    && output.sample_end == pin.sample_end
                                    && output.text == pin.surface
                            },
                        )
                })
                && choice.candidate.complete
                && choice.candidate.acoustic_boundaries_complete
                && choice.candidate.decode != Some((start, end))
                && choice.support.iter().any(|support| {
                    support.observation == *observation
                        && support.decode == Some((start, end))
                        && support.complete
                        && support.acoustic_boundaries_complete
                        && support
                            .original_text
                            .as_deref()
                            .zip(choice.candidate.original_text.as_deref())
                            .is_some_and(|(earlier, confirmed)| {
                                super::word_adjudication::label_equal(earlier, confirmed)
                            })
                })
        })
    }

    /// Accepted decode windows account for transcription work. Frontier
    /// closure owns sealing; word timestamps are not an energy-density map.
    pub(super) fn returned_word_scope_accounted(&self, observation: &ObservationIdentity) -> bool {
        let owner = &observation.occurrence;
        let scope_returned = self.decoded_source_scope_accounted(observation, owner);
        scope_returned
            && matches!(
                observation.producer,
                ObservationProducer::Whisper | ObservationProducer::CloudLive
            )
            && (self.word_pin_observations.contains(observation)
                || self.successful_empty_decodes.contains(observation))
            && self.slots_of(owner).is_some_and(|pins| {
                !pins.is_empty()
                    && pins.iter().all(|pin| {
                        pin.producer == ObservationProducer::ManualHuman
                            || (self.word_pin_observations.contains(&pin.observation)
                                && pin.text.split_whitespace().count() == 1
                                && pin.producer.authority_rank()
                                    >= observation.producer.authority_rank())
                    })
            })
    }

    /// Settle only exact rejected targets and retired coarse-source lineage.
    /// Producer return cannot erase a rejected higher-layer word, a missing
    /// accepted child, or a source still retained by a refused operation.
    pub(super) fn reconcile_returned_word_debt(&mut self, owner: &OccurrenceIdentity) {
        if !self.text_recovery_pending(owner) && !self.rejected_word_pins.contains_key(owner) {
            return;
        }
        let held = self.slots_of(owner).unwrap_or(&[]);
        let rejected = self
            .rejected_word_pins
            .get(owner)
            .cloned()
            .unwrap_or_default();
        let unresolved = rejected
            .iter()
            .filter(|source| {
                let partial = self.slot_operations.iter().find(|operation| {
                    operation.observation.occurrence == *owner
                        && operation.rule_id == "acoustic_resegmentation/partial-speech/v1"
                        && operation.sources.contains(source)
                });
                if let Some(operation) = partial {
                    let source_range = OccurrenceIdentity::new(
                        &owner.session,
                        owner.capture_epoch,
                        source.sample_start,
                        source.sample_end,
                    );
                    let returned =
                        self.decoded_source_scope_accounted(&operation.observation, &source_range);
                    let children_held = !operation.outputs.is_empty()
                        && operation.outputs.iter().all(|output| {
                            held.iter().any(|pin| {
                                self.word_pin_observations.contains(&pin.observation)
                                    && pin.text.split_whitespace().count() == 1
                                    && pin.producer.authority_rank()
                                        >= operation.observation.producer.authority_rank()
                                    && self.slot_descends_from(pin, output)
                            }) || self.word_deletions.iter().any(|deletion| {
                                deletion.operation.observation.occurrence == *owner
                                    && deletion
                                        .operation
                                        .sources
                                        .iter()
                                        .any(|source| self.slot_descends_from(source, output))
                            })
                        });
                    return !returned || !children_held || held.contains(source);
                }
                // This alternative names the actual saved candidate itself,
                // rather than a complete held source rejected by another pin.
                let awaits_whole_word = self.slot_alternatives.iter().any(|alternative| {
                    alternative.observation == source.observation
                        && alternative.reason == "decode_window_clipped"
                        && alternative.sources.contains(source)
                });
                let targets = held
                    .iter()
                    .filter(|pin| {
                        self.word_pin_observations.contains(&pin.observation)
                            && pin.observation != source.observation
                            && pin.text.split_whitespace().count() == 1
                            && source.text.split_whitespace().count() == 1
                            && (pin.producer == ObservationProducer::ManualHuman
                                || pin.producer.authority_rank() > source.producer.authority_rank()
                                || (pin.producer == source.producer
                                    && pin.observation.generation > source.observation.generation))
                            && self.pin_targets_source(source, pin)
                            && (!awaits_whole_word
                                || pin.producer == ObservationProducer::ManualHuman
                                || self.complete_word_slot(pin))
                            && rejected.iter().all(|other| {
                                !self.pin_targets_source(other, pin)
                                    || (other.sample_start == source.sample_start
                                        && other.sample_end == source.sample_end)
                            })
                    })
                    .count();
                let no_speech = self.word_deletions.iter().any(|deletion| {
                    let target = deletion.verdict.target();
                    deletion.operation.observation.occurrence == *owner
                        && target.same_capture(owner)
                        && target.sample_start <= source.sample_start
                        && target.sample_end >= source.sample_end
                });
                targets != 1 && !no_speech
            })
            .cloned()
            .collect::<Vec<_>>();
        if unresolved.is_empty() {
            self.rejected_word_pins.remove(owner);
        } else {
            self.rejected_word_pins.insert(owner.clone(), unresolved);
        }
        if !self.retained_recovery_source(owner)
            && !self.rejected_word_pins.contains_key(owner)
            && self.slots_of(owner).is_some_and(|pins| {
                pins.iter()
                    .any(|pin| self.returned_word_scope_accounted(&pin.observation))
            })
        {
            self.pending_text_recovery.remove(owner);
        }
    }

    pub(super) fn retained_recovery_source(&self, owner: &OccurrenceIdentity) -> bool {
        self.slot_alternatives.iter().any(|alternative| {
            alternative.observation.occurrence == *owner
                && alternative
                    .sources
                    .iter()
                    .any(|source| self.recovery_source_is_pending(alternative, source))
        })
    }

    pub(super) fn recovery_source_is_pending(
        &self,
        alternative: &SlotAlternative,
        source: &WordSlot,
    ) -> bool {
        let owner = &alternative.observation.occurrence;
        !matches!(
            alternative.reason,
            "resegmentation_source_label"
                | "window_start_clipped"
                | "window_stub_superseded"
                | "duplicate_pcm_edge_token"
        ) && source.producer != ObservationProducer::ManualHuman
            && source.producer.authority_rank()
                <= alternative.observation.producer.authority_rank()
            && (source.producer != alternative.observation.producer
                || source.observation.generation < alternative.observation.generation)
            && self
                .slots_of(owner)
                .is_some_and(|pins| pins.contains(source))
            // A complete held Word survives an unaccepted spelling
            // or timing proposal. That lexical dispute is retained
            // in word finality; it does not prove missing speech.
            // Both the actual refused decode and accepted work
            // covering this exact owner must exist, and the
            // scheduled producer must have returned. A stub alone,
            // a coarse source, or a refused partition cannot settle
            // the source's recovery obligation.
            && !(matches!(
                alternative.reason,
                "decode_window_clipped"
                    | "incomplete_source_scope"
                    | "word_adjudication_held"
                    | "lexical_disagreement"
            )
                && self.complete_word_slot(source)
                && self.returned_word_scope_accounted(&source.observation)
                && self
                    .decoded_word_windows
                    .contains_key(&alternative.observation)
                && self.frontiers.get(owner).is_some_and(|frontier| {
                    frontier.returned.contains(&alternative.observation.producer)
                })
                && self.decoded_source_scope_accounted(&alternative.observation, owner))
    }

    /// Jitter may move either fence, but may not borrow another word's centre
    /// or turn one member of a partition into the whole source word.
    pub(super) fn ordinary_word_target(
        &self,
        sources: &[WordSlot],
        pins: &[WordSlot],
        pin: &WordSlot,
    ) -> Option<usize> {
        let targets = sources
            .iter()
            .enumerate()
            .filter(|(_, source)| self.pin_targets_source(source, pin))
            .collect::<Vec<_>>();
        let [(index, source)] = targets.as_slice() else {
            return None;
        };
        (self.word_pin_observations.contains(&source.observation)
            && source.text.split_whitespace().count() == 1
            && pin.text.split_whitespace().count() == 1
            && self
                .complete_decoded_words
                .get(&source.observation)
                .is_none_or(|ranges| ranges.contains(&(source.sample_start, source.sample_end)))
            && pins
                .iter()
                .filter(|other| self.pin_targets_source(source, other))
                .count()
                == 1
            && sources.iter().enumerate().all(|(other_index, other)| {
                let midpoint = other.sample_start + (other.sample_end - other.sample_start) / 2;
                other_index == *index || midpoint < pin.sample_start || midpoint >= pin.sample_end
            }))
        .then_some(*index)
    }

    /// An ordinary target cannot detach a Word from a crossed coarse source.
    /// Collect the entire uncertain partition before any part can commit.
    /// Intersection joins an adjudication question, never occurrence identity.
    fn crossed_source_partitions(
        &self,
        sources: &[WordSlot],
        pins: &[WordSlot],
        ordinary_targets: &[Option<usize>],
    ) -> Vec<(BTreeSet<usize>, BTreeSet<usize>)> {
        let intersects = |source: usize, pin: usize| {
            sources[source]
                .observation
                .occurrence
                .same_capture(&pins[pin].observation.occurrence)
                && sources[source].sample_start < pins[pin].sample_end
                && pins[pin].sample_start < sources[source].sample_end
        };
        let connected = |source: usize, pin: usize| {
            intersects(source, pin) || self.pin_targets_source(&sources[source], &pins[pin])
        };
        let mut partitions: Vec<(BTreeSet<usize>, BTreeSet<usize>)> = Vec::new();
        for (seed, target) in ordinary_targets.iter().enumerate() {
            let Some(target) = target else {
                continue;
            };
            if partitions.iter().any(|(_, words)| words.contains(&seed))
                || !sources.iter().enumerate().any(|(index, source)| {
                    index != *target && self.coarse_word_source(source) && intersects(index, seed)
                })
            {
                continue;
            }
            let mut word_indices = BTreeSet::from([seed]);
            let mut source_indices = BTreeSet::new();
            loop {
                let size = word_indices.len() + source_indices.len();
                for index in 0..sources.len() {
                    if word_indices.iter().any(|word| connected(index, *word)) {
                        source_indices.insert(index);
                    }
                }
                for index in 0..pins.len() {
                    if source_indices
                        .iter()
                        .any(|source| connected(*source, index))
                    {
                        word_indices.insert(index);
                    }
                }
                if size == word_indices.len() + source_indices.len() {
                    break;
                }
            }
            partitions.push((source_indices, word_indices));
        }
        partitions
    }

    /// Resolve connected intersections as one geometric operation. Slot count
    /// is not occurrence identity: several words may refine one coarse source.
    pub(super) fn resegment_word_slots(
        &mut self,
        observation: &ObservationIdentity,
        slots: &mut Vec<WordSlot>,
        incoming: &mut Vec<WordSlot>,
    ) -> (Vec<SlotOperationReceipt>, Vec<GroupSpeechCoverageReceipt>) {
        if !matches!(
            observation.producer,
            ObservationProducer::Whisper | ObservationProducer::CloudLive
        ) {
            return (Vec::new(), Vec::new());
        }
        let prior = slots.clone();
        let pins = incoming.clone();
        let mut visited = BTreeSet::new();
        let mut consumed = BTreeSet::new();
        let mut operations = Vec::new();
        let mut proposed_coverages = Vec::new();
        // One uniquely targeted measured word remains an ordinary correction.
        // Incidental overlap with a neighbour must not turn duration jitter
        // into a request to partition that neighbour's PCM.
        let ordinary_targets = pins
            .iter()
            .map(|pin| self.ordinary_word_target(&prior, &pins, pin))
            .collect::<Vec<_>>();
        let crossed_partitions = self.crossed_source_partitions(&prior, &pins, &ordinary_targets);
        for seed in 0..pins.len() {
            if visited.contains(&seed) {
                continue;
            }
            let mut word_indices = BTreeSet::from([seed]);
            let mut source_indices = BTreeSet::new();
            loop {
                // Borrow the ledger only while discovering this component.
                // Refusals and alternatives are recorded after discovery.
                let connected = |source: usize, pin: usize| {
                    if let Some((sources, _)) = crossed_partitions
                        .iter()
                        .find(|(_, words)| words.contains(&pin))
                    {
                        return sources.contains(&source);
                    }
                    ordinary_targets[pin].map_or_else(
                        || {
                            let held = &prior[source];
                            let word = &pins[pin];
                            let coarse = !self.word_pin_observations.contains(&held.observation)
                                || held.text.split_whitespace().count() != 1
                                || self
                                    .complete_decoded_words
                                    .get(&held.observation)
                                    .is_some_and(|ranges| {
                                        !ranges.contains(&(held.sample_start, held.sample_end))
                                    });
                            self.pin_targets_source(held, word)
                                || (coarse
                                    && held.sample_start < word.sample_end
                                    && word.sample_start < held.sample_end)
                        },
                        |target| source == target,
                    )
                };
                let size = word_indices.len() + source_indices.len();
                for index in 0..prior.len() {
                    if word_indices.iter().any(|word| connected(index, *word)) {
                        source_indices.insert(index);
                    }
                }
                for index in 0..pins.len() {
                    if source_indices
                        .iter()
                        .any(|source| connected(*source, index))
                    {
                        word_indices.insert(index);
                    }
                }
                if size == word_indices.len() + source_indices.len() {
                    break;
                }
            }
            visited.extend(word_indices.iter().copied());
            if source_indices.is_empty() {
                continue;
            }
            let mut sources = source_indices
                .iter()
                .map(|index| prior[*index].clone())
                .collect::<Vec<_>>();
            let mut outputs = word_indices
                .iter()
                .map(|index| pins[*index].clone())
                .collect::<Vec<_>>();
            outputs.sort_by_key(|word| (word.sample_start, word.sample_end));
            let coarse_source = |source: &WordSlot| self.coarse_word_source(source);
            let all_coarse = sources.iter().all(coarse_source);
            // A coarse hypothesis has no independent word claims. Its
            // refinement still needs the measured partition/source accounting
            // below; lexical voting applies once physical words exist.
            let source_complete =
                all_coarse || self.asr_source_scope_complete(observation, &sources);
            let authority = sources.iter().all(|source| {
                source.producer != ObservationProducer::ManualHuman
                    && (source.producer.authority_rank() < observation.producer.authority_rank()
                        || (source.producer == observation.producer
                            && source.observation.generation < observation.generation))
            });
            // Centre inclusion misses an edge-overlapping complete source that
            // this pin already targets, including through source provenance.
            let collapses_complete_words = outputs.iter().any(|word| {
                sources
                    .iter()
                    .filter(|source| {
                        let midpoint =
                            source.sample_start + (source.sample_end - source.sample_start) / 2;
                        let complete = self
                            .complete_decoded_words
                            .get(&source.observation)
                            .is_some_and(|ranges| {
                                ranges.contains(&(source.sample_start, source.sample_end))
                            });
                        complete
                            && (self.pin_targets_source(source, word)
                                || (word.sample_start <= midpoint && midpoint < word.sample_end))
                    })
                    .count()
                    > 1
            });
            // A completed word-grain decode covering every addressed source
            // is a partition receipt. It does not turn word timestamp gaps into
            // untranscribed speech or fabricate a speech-coverage receipt.
            let window_refinement =
                self.decoded_word_windows
                    .get(observation)
                    .is_some_and(|(start, end)| {
                        sources.iter().all(|source| {
                            *start <= source.sample_start && *end >= source.sample_end
                        })
                    })
                    && outputs.iter().all(|word| {
                        self.complete_decoded_words
                            .get(observation)
                            .is_some_and(|ranges| {
                                ranges.contains(&(word.sample_start, word.sample_end))
                            })
                    });
            if sources.len() == 1
                && outputs.len() == 1
                && (ordinary_targets[*word_indices.first().unwrap()].is_some()
                    || (!window_refinement
                        && sources[0].sample_start == outputs[0].sample_start
                        && sources[0].sample_end == outputs[0].sample_end))
            {
                // Equal extent alone supplies no Word completeness. A complete
                // decode instead accounts for this source through the existing
                // partition receipt, even when the prior is a coarse group.
                continue;
            }
            // A completed word-grain window proves which PCM was observed.
            // It does not authorize a merge of already distinct complete Words.
            // Pin or speech coverage alone cannot retire their word evidence.
            let geometry = !collapses_complete_words
                && source_indices.last().unwrap() - source_indices.first().unwrap() + 1
                    == sources.len()
                && outputs
                    .windows(2)
                    .all(|pair| pair[0].sample_end <= pair[1].sample_start);
            let mut coverage = sources
                .iter()
                .map(|source| self.group_speech_coverage(observation, source, &outputs))
                .collect::<Option<Vec<_>>>();
            if authority
                && geometry
                && all_coarse
                && coverage.is_none()
                && !window_refinement
                && self.speech_evidence.is_some()
            {
                outputs.retain(|word| {
                    if self
                        .group_speech_coverage(observation, word, std::slice::from_ref(word))
                        .is_some()
                    {
                        return true;
                    }
                    self.refuse_word_slot(
                        observation,
                        word,
                        sources.clone(),
                        "word_speech_unproven",
                    );
                    false
                });
                if outputs.is_empty() {
                    consumed.extend(word_indices.iter().copied());
                    continue;
                }
                sources.retain(|source| {
                    outputs.iter().any(|word| {
                        self.pin_targets_source(source, word)
                            || (source.sample_start < word.sample_end
                                && word.sample_start < source.sample_end)
                    })
                });
                coverage = sources
                    .iter()
                    .map(|source| self.group_speech_coverage(observation, source, &outputs))
                    .collect::<Option<Vec<_>>>();
            }
            let candidate = compose_label(&outputs);
            let held = compose_label(&sources);
            // Individual heard words do not need to cover the rest of a
            // coarse hypothesis. That remainder becomes explicit source debt;
            // a complete measured word still requires complete accounting.
            let partial_refinement = all_coarse
                && coverage.is_none()
                && !window_refinement
                && outputs.iter().all(|word| {
                    self.group_speech_coverage(observation, word, std::slice::from_ref(word))
                        .is_some()
                });
            // With no observer snapshot, exact pin coverage may account for
            // the source PCM. Spelling agreement supplies no missing geometry.
            // A present but unusable snapshot never acts as absent evidence.
            let range_refinement = all_coarse
                && self.speech_evidence.is_none()
                && outputs
                    .iter()
                    .all(|word| word.text.split_whitespace().count() == 1)
                && sources.iter().all(|source| {
                    let mut cursor = source.sample_start;
                    for word in &outputs {
                        if word.sample_start <= cursor && word.sample_end > cursor {
                            cursor = word.sample_end;
                        }
                    }
                    cursor >= source.sample_end
                });
            // Sparse child pins may retain every held label without claiming
            // speech coverage. Missing or ambiguous tokens still refuse the cut.
            let content_refinement = all_coarse
                && self.speech_evidence.is_none()
                && outputs
                    .iter()
                    .all(|word| word.text.split_whitespace().count() == 1)
                && preserve_group_content(&held, &candidate).is_ok_and(|(_, retained)| !retained);
            // Scope proof stays available for a real partition. It cannot
            // clear ambiguity or speech debt for an unproved contraction.
            let window_partition = window_refinement && !collapses_complete_words;
            let ambiguous = repetition_target_ambiguous(&sources, &outputs)
                && !(coverage.is_some()
                    || range_refinement
                    || content_refinement
                    || partial_refinement
                    || window_partition);
            let local_choice = if !all_coarse && source_complete && geometry && !ambiguous {
                self.adjudicate_word_sources(observation, &sources, &outputs)
            } else {
                None
            };
            let refusal = if !source_complete {
                self.record_word_choice(
                    observation,
                    &sources,
                    &outputs,
                    "incomplete_source_scope",
                    false,
                );
                Some("incomplete_source_scope")
            } else if local_choice == Some(false) {
                Some("lexical_disagreement")
            } else if !authority && local_choice != Some(true) {
                Some("protected_source")
            } else if !geometry
                || ambiguous
                || (coverage.is_none()
                    && !range_refinement
                    && !content_refinement
                    && !partial_refinement
                    && !window_partition)
            {
                Some("resegmentation_unaccounted_speech")
            } else {
                None
            };
            // Resolve a refused cluster once. Re-offering each word would
            // lose the combined candidate and its complete source provenance.
            consumed.extend(word_indices.iter().copied());
            if let Some(reason) = refusal {
                self.retain_slot_alternative(observation, &candidate, sources, reason);
                for word in &outputs {
                    self.record_word_slot_refusal(
                        observation,
                        word,
                        !matches!(reason, "incomplete_source_scope" | "lexical_disagreement"),
                    );
                }
                continue;
            }
            if partial_refinement {
                self.retain_slot_alternative(
                    observation,
                    &candidate,
                    sources.clone(),
                    "partial_group_speech_pending",
                );
            }
            let mut source_ranges = sources
                .iter()
                .flat_map(|source| self.slot_source_ranges(source))
                .collect::<Vec<_>>();
            for (owner, pin) in self.assigned_word_pin_ranges(observation) {
                if owner == observation.occurrence
                    && pin.same_capture(&owner)
                    && outputs.iter().any(|output| {
                        pin.sample_start.max(owner.sample_start) == output.sample_start
                            && pin.sample_end.min(owner.sample_end) == output.sample_end
                    })
                    && !source_ranges.contains(&pin)
                {
                    source_ranges.push(pin);
                }
            }
            let operation = SlotOperationReceipt {
                observation: observation.clone(),
                kind: if sources.len() == 1
                    && outputs.len() == 1
                    && sources[0].sample_start == outputs[0].sample_start
                    && sources[0].sample_end == outputs[0].sample_end
                {
                    SlotOperationKind::Correct
                } else if outputs.len() > sources.len() || sources.len() == 1 {
                    SlotOperationKind::Split
                } else {
                    SlotOperationKind::Merge
                },
                source_ranges,
                sources: sources.clone(),
                outputs: outputs.clone(),
                rule_id: if coverage.is_some() {
                    "acoustic_resegmentation/speech-coverage/v1"
                } else if window_refinement {
                    "acoustic_resegmentation/decode-window/v1"
                } else if partial_refinement {
                    "acoustic_resegmentation/partial-speech/v1"
                } else if content_refinement && !range_refinement {
                    "acoustic_resegmentation/content-preserved/v1"
                } else {
                    "acoustic_resegmentation/pin-coverage/v1"
                }
                .into(),
            };
            self.retain_slot_alternative(
                &sources[0].observation,
                &held,
                sources.clone(),
                "resegmentation_source_label",
            );
            if let Some(coverage) = coverage {
                for (speech, coverage) in coverage {
                    proposed_coverages.push(GroupSpeechCoverageReceipt {
                        operation: operation.clone(),
                        speech,
                        coverage,
                        rule_version: "group-speech-coverage/v1",
                    });
                }
            }
            slots.retain(|source| !sources.contains(source));
            slots.extend(outputs);
            operations.push(operation);
        }
        *incoming = incoming
            .iter()
            .enumerate()
            .filter(|(index, _)| !consumed.contains(index))
            .map(|(_, pin)| pin.clone())
            .collect();
        slots.sort_by_key(|word| (word.sample_start, word.sample_end));
        (operations, proposed_coverages)
    }

    /// Snapshot the existing capture observer; unavailable evidence replaces
    /// the previous snapshot so stale measurements cannot authorize a cut.
    pub fn record_speech_evidence(&mut self, speech: &AcousticSpeechEvidence) {
        self.speech_evidence = Some(speech.clone());
    }

    /// Retain the existing capture owner and a bounded exact-PCM quiet receipt.
    /// Coverage and successful source-work accounting keep their own snapshot.
    pub(crate) fn record_decode_fence_energy(
        &mut self,
        energy: &crate::audio::capture_receipt::CaptureEnergyOwner,
        captured_samples: u64,
        edge: Option<(&OccurrenceIdentity, &[f32])>,
    ) {
        use crate::audio::capture_receipt::ACTIVE_SPEECH_LINEAR_FLOOR;

        let resolution = u64::from(self.capture_rate_hz.unwrap_or(16_000))
            .div_ceil(50)
            .saturating_add(1);
        let quiet_range = edge.and_then(|(range, samples)| {
            if !range.is_anchored()
                || !energy
                    .identity()
                    .matches(&range.session, range.capture_epoch)
                || range.sample_end > captured_samples
                || range.sample_end - range.sample_start != resolution
                || samples.len() as u64 != resolution
                || samples.iter().any(|sample| !sample.is_finite())
            {
                return None;
            }
            // Same sum-of-squares RMS and active floor as push_samples, now
            // over actual edge PCM. A single zero crossing cannot prove quiet.
            let sum_sq = samples.iter().fold(0.0_f64, |sum, sample| {
                sum + f64::from(*sample) * f64::from(*sample)
            });
            let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
            (rms.is_finite() && rms < ACTIVE_SPEECH_LINEAR_FLOOR).then(|| range.clone())
        });
        self.decode_fence_energy = Some(DecodeFenceEnergy {
            owner: energy.clone(),
            captured_samples,
            quiet_range,
        });
    }

    /// One routed batch's original word geometry, before clipping at owner
    /// edges. Only an assignment for this exact admission may cover a neighbour.
    pub fn record_assigned_word_pins(
        &mut self,
        observation: &ObservationIdentity,
        assignments: &[(OccurrenceIdentity, OccurrenceIdentity)],
    ) {
        self.assigned_word_pins = Some(AssignedWordPinBatch {
            observation: observation.clone(),
            assignments: assignments.to_vec(),
        });
    }

    pub(super) fn group_speech_coverage(
        &self,
        observation: &ObservationIdentity,
        source: &WordSlot,
        incoming: &[WordSlot],
    ) -> Option<(AcousticSpeechEvidence, Vec<SpeechPinCoverage>)> {
        let range = OccurrenceIdentity::new(
            &source.observation.occurrence.session,
            source.observation.occurrence.capture_epoch,
            source.sample_start,
            source.sample_end,
        );
        self.speech_pin_coverage(observation, &range, incoming)
    }

    /// The same PCM accounting answers a source partition and whole-owner
    /// recovery. An occurrence extent is a scope, never a fabricated word pin.
    pub(super) fn speech_pin_coverage(
        &self,
        observation: &ObservationIdentity,
        source: &OccurrenceIdentity,
        incoming: &[WordSlot],
    ) -> Option<(AcousticSpeechEvidence, Vec<SpeechPinCoverage>)> {
        let owner = &observation.occurrence;
        if !source.same_capture(owner) {
            return None;
        }
        let speech = self.measured_speech_for(source)?;
        let observed = speech.availability().observed_samples()?;
        let mut pins = incoming
            .iter()
            .map(|word| {
                (
                    OccurrenceIdentity::new(
                        &owner.session,
                        owner.capture_epoch,
                        word.sample_start,
                        word.sample_end,
                    ),
                    owner.clone(),
                )
            })
            .collect::<Vec<_>>();
        if let Some(batch) = &self.assigned_word_pins
            && &batch.observation == observation
        {
            for (assigned_owner, pin) in &batch.assignments {
                // Routing is a proposal. Only a committed measured neighbour
                // may lend its actual pin; qualification alone proves no words.
                let midpoint = pin.sample_start.saturating_add(pin.sample_len() / 2);
                if assigned_owner != owner
                    && assigned_owner.same_capture(owner)
                    && pin.same_capture(owner)
                    && pin.is_anchored()
                    && pin.sample_end <= observed
                    && assigned_owner.is_anchored()
                    && midpoint >= assigned_owner.sample_start
                    && midpoint < assigned_owner.sample_end
                    && (assigned_owner.sample_end <= owner.sample_start
                        || assigned_owner.sample_start >= owner.sample_end)
                    && self.committed.contains_key(assigned_owner)
                    && !self.is_sealed(assigned_owner)
                    && !self.word_deletions.iter().any(|deletion| {
                        let range = deletion.verdict.target();
                        range.same_capture(assigned_owner)
                            && pin.sample_start.max(assigned_owner.sample_start)
                                >= range.sample_start
                            && pin.sample_end.min(assigned_owner.sample_end) <= range.sample_end
                    })
                    && self.slots_of(assigned_owner).is_none_or(|slots| {
                        slots.iter().all(|slot| {
                            slot.producer != ObservationProducer::ManualHuman
                                && slot.producer.authority_rank()
                                    <= observation.producer.authority_rank()
                        })
                    })
                {
                    for slot in self.slots_of(assigned_owner).unwrap_or(&[]) {
                        if self.word_pin_observations.contains(&slot.observation)
                            && slot.sample_start
                                == pin.sample_start.max(assigned_owner.sample_start)
                            && slot.sample_end == pin.sample_end.min(assigned_owner.sample_end)
                        {
                            for range in self.slot_source_ranges(slot) {
                                if range.same_capture(owner)
                                    && range.is_anchored()
                                    && range.sample_start >= pin.sample_start
                                    && range.sample_end <= pin.sample_end
                                {
                                    pins.push((range, assigned_owner.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        pins.sort_by_key(|(pin, _)| (pin.sample_start, pin.sample_end));
        let mut coverage = Vec::new();
        for range in speech.ranges() {
            let start = range.sample_start.max(source.sample_start);
            let end = range.sample_end.min(source.sample_end);
            let mut cursor = start;
            for (pin, pin_owner) in &pins {
                if cursor >= end {
                    break;
                }
                if pin.sample_end <= cursor {
                    continue;
                }
                if pin.sample_start > cursor {
                    break;
                }
                let covered_end = pin.sample_end.min(end);
                coverage.push(SpeechPinCoverage {
                    speech_range: OccurrenceIdentity::new(
                        &owner.session,
                        owner.capture_epoch,
                        cursor,
                        covered_end,
                    ),
                    pin_range: pin.clone(),
                    pin_owner: pin_owner.clone(),
                });
                cursor = covered_end;
            }
            if cursor < end {
                return None;
            }
        }
        // Measured silence stays on the explicit word-verdict path. This rule
        // accounts for measured speech, never grants deletion on an empty set.
        (!coverage.is_empty()).then(|| (speech.clone(), coverage))
    }

    pub fn group_speech_coverages(&self) -> &[GroupSpeechCoverageReceipt] {
        &self.group_speech_coverages
    }

    /// Apply count-preserving Dictionary labels to exact existing sources.
    pub(crate) fn rewrite_dictionary_slots(
        &mut self,
        observation: &ObservationIdentity,
        rewrites: &[(SlotTarget, DictionarySlotRule)],
    ) -> Result<MutationReceipt, SlotOperationRefusal> {
        if observation.producer != ObservationProducer::Lexicon {
            return Err(SlotOperationRefusal::ProducerNotAuthorized);
        }
        let mut proposed = self
            .slots_of(&observation.occurrence)
            .unwrap_or(&[])
            .to_vec();
        let had_pins = proposed
            .iter()
            .any(|source| self.word_pin_observations.contains(&source.observation));
        for (target, rule) in rewrites {
            let sources =
                self.resolve_slot_targets(observation, std::slice::from_ref(target), true)?;
            let source = &sources[0];
            if rule.id.trim().is_empty()
                || rule.input != vec![source.text.clone()]
                || rule.canonical.split_whitespace().count()
                    != source.text.split_whitespace().count()
            {
                return Err(SlotOperationRefusal::InvalidEvidence);
            }
            let output = proposed
                .iter_mut()
                .find(|slot| SlotTarget::from(&**slot) == *target)
                .ok_or(SlotOperationRefusal::StaleTarget)?;
            if output.text == rule.canonical {
                continue;
            }
            output.text = rule.canonical.clone();
            output.producer = observation.producer;
            output.observation = observation.clone();
            output.surface_rewritten = true;
        }
        let trace = super::super::trail::SlotTrace::rewrite(self, observation, rewrites);
        let label = compose_label(&proposed);
        let start = self.slot_operations.len();
        let decision = self.admit_with_slots(observation, &label, Some(proposed), true);
        if !had_pins {
            self.word_pin_observations.remove(observation);
        }
        for operation in &mut self.slot_operations[start..] {
            if let Some((_, rule)) = rewrites.iter().find(|(target, _)| {
                operation
                    .sources
                    .iter()
                    .any(|source| SlotTarget::from(source) == *target)
            }) {
                operation.rule_id = rule.id.clone();
            }
        }
        trace.finish(Ok(decision), self)
    }

    pub fn slot_operations(&self) -> &[SlotOperationReceipt] {
        &self.slot_operations
    }

    /// Resolve provenance through earlier operations, including prior merges.
    pub fn slot_source_ranges(&self, slot: &WordSlot) -> Vec<OccurrenceIdentity> {
        self.slot_operations
            .iter()
            .rev()
            .find_map(|receipt| {
                receipt
                    .outputs
                    .iter()
                    .any(|output| output == slot)
                    .then(|| {
                        // Every measured child owns its own playback PCM.
                        // The operation still retains all original source ranges.
                        if receipt.kind == SlotOperationKind::Split || receipt.outputs.len() > 1 {
                            vec![OccurrenceIdentity::new(
                                &slot.observation.occurrence.session,
                                slot.observation.occurrence.capture_epoch,
                                slot.sample_start,
                                slot.sample_end,
                            )]
                        } else {
                            receipt.source_ranges.clone()
                        }
                    })
            })
            .unwrap_or_else(|| {
                vec![OccurrenceIdentity::new(
                    &slot.observation.occurrence.session,
                    slot.observation.occurrence.capture_epoch,
                    slot.sample_start,
                    slot.sample_end,
                )]
            })
    }

    pub(super) fn resolve_slot_targets(
        &self,
        observation: &ObservationIdentity,
        targets: &[SlotTarget],
        require_label_authority: bool,
    ) -> Result<Vec<WordSlot>, SlotOperationRefusal> {
        // Geometric operations share Raw ownership with label admission;
        // even a no-speech verdict cannot make Formatter an acoustic author.
        if observation.producer == ObservationProducer::Formatter {
            return Err(SlotOperationRefusal::ProducerNotAuthorized);
        }
        if targets.is_empty() || self.answered.contains(observation) {
            return Err(SlotOperationRefusal::ReplayedObservation);
        }
        if self.is_sealed(&observation.occurrence)
            && observation.producer != ObservationProducer::ManualHuman
        {
            return Err(SlotOperationRefusal::Sealed);
        }
        let slots = self
            .slots_of(&observation.occurrence)
            .ok_or(SlotOperationRefusal::StaleTarget)?;
        let mut sources = Vec::new();
        for target in targets {
            let source = slots
                .iter()
                .find(|slot| SlotTarget::from(*slot) == *target)
                .ok_or(SlotOperationRefusal::StaleTarget)?;
            if sources.contains(source) {
                return Err(SlotOperationRefusal::InvalidEvidence);
            }
            if source.producer == ObservationProducer::ManualHuman
                && observation.producer != ObservationProducer::ManualHuman
            {
                return Err(SlotOperationRefusal::ProtectedHuman);
            }
            if require_label_authority
                && (observation.producer.authority_rank() < source.producer.authority_rank()
                    || (observation.producer == source.producer
                        && observation.generation <= source.observation.generation))
            {
                return Err(SlotOperationRefusal::ProducerNotAuthorized);
            }
            sources.push(source.clone());
        }
        // Targets must name consecutive committed slots in document order.
        if !slots.windows(sources.len()).any(|window| window == sources) {
            return Err(SlotOperationRefusal::InvalidEvidence);
        }
        Ok(sources)
    }

    pub(super) fn commit_slot_operation(&mut self, receipt: SlotOperationReceipt) {
        let observation = &receipt.observation;
        let owner = &observation.occurrence;
        let held = self
            .committed
            .get_mut(owner)
            .expect("resolved source owner");
        let old_label = held.label.clone();
        let from = held.producer;
        let first = held
            .slots
            .iter()
            .position(|slot| slot == &receipt.sources[0])
            .expect("resolved source slot");
        held.slots.splice(
            first..first + receipt.sources.len(),
            receipt.outputs.clone(),
        );
        held.producer = observation.producer;
        held.generation = observation.generation;
        held.recompose();
        let label = held.label.clone();
        let decision = if label == old_label {
            MutationReceipt::Preserve {
                occurrence: owner.clone(),
                held_by: observation.producer,
            }
        } else {
            MutationReceipt::Correct {
                occurrence: owner.clone(),
                from,
                to: observation.producer,
            }
        };
        if let Some(seal) = self.seals.get(owner)
            && observation.producer == ObservationProducer::ManualHuman
        {
            self.manual_edits.push(ManualEditReceipt {
                receipt_id: format!(
                    "manual-slot-{}-{}",
                    observation.request, observation.generation
                ),
                occurrence: owner.clone(),
                supersedes_seal: seal.receipt_id.clone(),
                superseded_label: old_label,
                label: label.clone(),
                observation: observation.clone(),
            });
        }
        self.offered_observations += 1;
        self.note_answered(observation);
        self.word_pin_observations.insert(observation.clone());
        self.record_layer_decision(observation, &label, &decision, None);
        let observation = observation.clone();
        self.slot_operations.push(receipt);
        self.remember_word_outputs(&observation);
    }

    /// Merge exactly the named sources using a specific Dictionary rule.
    /// Arbitrary ASR sentence compression has no authorization here.
    pub fn merge_word_slots(
        &mut self,
        observation: &ObservationIdentity,
        targets: &[SlotTarget],
        rule: &DictionarySlotRule,
    ) -> Result<SlotOperationReceipt, SlotOperationRefusal> {
        if observation.producer != ObservationProducer::Lexicon {
            return Err(SlotOperationRefusal::ProducerNotAuthorized);
        }
        let sources = self.resolve_slot_targets(observation, targets, true)?;
        if sources.len() < 2
            || rule.id.trim().is_empty()
            || rule.canonical.split_whitespace().count() != 1
            || sources
                .iter()
                .map(|source| source.text.clone())
                .collect::<Vec<_>>()
                != rule.input
        {
            return Err(SlotOperationRefusal::InvalidEvidence);
        }
        let source_ranges = sources
            .iter()
            .flat_map(|source| self.slot_source_ranges(source))
            .collect();
        let output = WordSlot {
            sample_start: sources[0].sample_start,
            sample_end: sources.last().unwrap().sample_end,
            text: rule.canonical.clone(),
            producer: observation.producer,
            observation: observation.clone(),
            witness: SlotWitness::Unwitnessed,
            confidence: None,
            surface_rewritten: true,
        };
        let receipt = SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Merge,
            sources,
            outputs: vec![output],
            source_ranges,
            rule_id: rule.id.clone(),
        };
        let trace = super::super::trail::SlotTrace::merge(self, observation, targets, rule);
        self.commit_slot_operation(receipt.clone());
        trace.finish(Ok(receipt), self)
    }

    /// A split requires timed child pins. Without them a multiword label stays
    /// one group slot through ordinary 1-to-1 correction.
    pub fn split_word_slot(
        &mut self,
        observation: &ObservationIdentity,
        target: &SlotTarget,
        children: &[WordPin],
    ) -> Result<SlotOperationReceipt, SlotOperationRefusal> {
        if !matches!(
            observation.producer,
            ObservationProducer::Apple
                | ObservationProducer::CloudLive
                | ObservationProducer::Whisper
                | ObservationProducer::ManualHuman
        ) {
            return Err(SlotOperationRefusal::ProducerNotAuthorized);
        }
        let sources =
            self.resolve_slot_targets(observation, std::slice::from_ref(target), false)?;
        self.prepare_word_evidence(observation, children);
        let trace = super::super::trail::SlotTrace::split(self, observation, target, children);
        if !self.asr_source_scope_complete(observation, &sources) {
            self.record_word_choice(observation, &sources, &[], "incomplete_source_scope", false);
            return trace.finish(Err(SlotOperationRefusal::InvalidEvidence), self);
        }
        let source = &sources[0];
        if children.len() < 2
            || children.iter().any(|child| {
                child.sample_end <= child.sample_start
                    || child.sample_start < source.sample_start
                    || child.sample_end > source.sample_end
                    || child.text.trim().is_empty()
            })
            || children
                .windows(2)
                .any(|pair| pair[0].sample_end > pair[1].sample_start)
            || children[0].sample_start != source.sample_start
            || children.last().unwrap().sample_end != source.sample_end
        {
            return trace.finish(Err(SlotOperationRefusal::InvalidEvidence), self);
        }
        self.record_word_decode_bounds(observation, children);
        let outputs = children
            .iter()
            .map(|pin| WordSlot {
                sample_start: pin.sample_start,
                sample_end: pin.sample_end,
                text: pin.text.clone(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                confidence: pin.confidence,
                surface_rewritten: pin.surface_rewritten,
            })
            .collect::<Vec<_>>();
        match self.adjudicate_word_sources(observation, &sources, &outputs) {
            Some(false) => {
                return trace.finish(Err(SlotOperationRefusal::ProducerNotAuthorized), self);
            }
            None => {
                if let Err(refusal) =
                    self.resolve_slot_targets(observation, std::slice::from_ref(target), true)
                {
                    return trace.finish(Err(refusal), self);
                }
            }
            Some(true) => {}
        }
        let receipt = SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Split,
            source_ranges: self.slot_source_ranges(source),
            sources,
            outputs,
            rule_id: "producer_child_pcm_boundaries/v1".to_string(),
        };
        self.commit_slot_operation(receipt.clone());
        trace.finish(Ok(receipt), self)
    }
}

#[cfg(test)]
mod slot_ops_tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::word_adjudication_tests::corroborate_candidate;

    // Root-owned independent controls: actual capture measurements, not a word floor.
    fn forensic_neighbour_capture(
        session: &str,
        capture_epoch: u64,
        quiet_neighbour: bool,
    ) -> (
        AcousticLedger,
        OccurrenceIdentity,
        OccurrenceIdentity,
        Vec<f32>,
    ) {
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        let owner = OccurrenceIdentity::new(session, capture_epoch, 0, 16_000);
        let neighbour = OccurrenceIdentity::new(session, capture_epoch, 16_000, 32_000);
        let mut pcm = vec![0.0_f32; 32_000];
        pcm[2_000..8_000].fill(0.2);
        pcm[14_000..if quiet_neighbour { 16_000 } else { 20_000 }].fill(0.2);
        let energy = CaptureEnergyOwner::bind(session, capture_epoch);
        let mut writer = CaptureLevelAccumulator::bound_to(&energy);
        for chunk in pcm.chunks(1_000) {
            writer.push_samples(chunk);
        }
        let speech = energy.session_active_speech_ranges(session, capture_epoch, 16_000);
        assert_eq!(speech.availability().observed_samples(), Some(32_000));
        assert_eq!(
            speech
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect::<Vec<_>>(),
            [
                (2_000, 8_000),
                (14_000, if quiet_neighbour { 16_000 } else { 20_000 })
            ]
        );
        let calibration = EnergyCalibration::new("forensic-neighbour-pcm", 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        for occurrence in [&owner, &neighbour] {
            let samples = &pcm[occurrence.sample_start as usize..occurrence.sample_end as usize];
            let integral = samples
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>();
            if integral == 0.0 {
                continue;
            }
            let peak = samples
                .iter()
                .map(|sample| f64::from(sample.abs()))
                .fold(0.0_f64, f64::max);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: samples.len() as f64 / 16.0,
                            energy_integral: integral,
                            mean_rms_dbfs: 20.0 * (integral / samples.len() as f64).sqrt().log10(),
                            peak_dbfs: 20.0 * peak.log10(),
                            vad_open_sample: Some(occurrence.sample_start),
                            vad_close_sample: Some(occurrence.sample_end),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration
                    )
                    .is_qualified()
            );
        }
        ledger.record_speech_evidence(&speech);
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
        assert!(
            ledger
                .admit(&apple, "czy plan weryfikowałeś")
                .grants_mutation()
        );
        (ledger, owner, neighbour, pcm)
    }

    fn forensic_neighbour_words(observation: &ObservationIdentity) -> Vec<WordSlot> {
        [(2_000, 5_000, "czy"), (5_000, 8_000, "weryfikowałeś")]
            .into_iter()
            .map(|(sample_start, sample_end, text)| WordSlot {
                sample_start,
                sample_end,
                text: text.into(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                confidence: None,
                surface_rewritten: false,
            })
            .collect()
    }

    #[test]
    fn forensic_neighbour_only_exact_held_word_lends_pcm_and_settles_source() {
        for state in ["word", "label", "qualified", "human", "sealed"] {
            for variant in 0..5 {
                let (mut ledger, owner, neighbour, _pcm) =
                    forensic_neighbour_capture(&format!("neighbour-{state}-{variant}"), 23, false);
                let original = ledger.slots_of(&owner).unwrap()[0].clone();
                let producer = if state == "human" {
                    ObservationProducer::ManualHuman
                } else {
                    ObservationProducer::Apple
                };
                let neighbour_observation =
                    ObservationIdentity::new(producer, 2, 0, neighbour.clone());
                if state == "label" {
                    assert!(
                        ledger
                            .admit(&neighbour_observation, "sąsiad")
                            .grants_mutation()
                    );
                } else if state != "qualified" {
                    assert!(
                        ledger
                            .admit_word_slots(
                                &neighbour_observation,
                                &[WordPin::new(14_000, 20_000, "sąsiad")
                                    .with_decode_window(0, 32_000)]
                            )
                            .grants_mutation()
                    );
                    if state == "sealed" {
                        ledger.schedule_frontier(neighbour.clone(), [producer]);
                        ledger.note_frontier_return(&neighbour, producer);
                        ledger.seal(&neighbour).unwrap();
                    }
                }
                let neighbour_before = ledger.slots_of(&neighbour).map(<[WordSlot]>::to_vec);
                let next =
                    ObservationIdentity::new(ObservationProducer::Whisper, 3, 1, owner.clone());
                let original_pin = OccurrenceIdentity::new(&owner.session, 23, 14_000, 20_000);
                let mut assignment = next.clone();
                match variant {
                    1 => assignment.request += 1,
                    2 => assignment.generation += 1,
                    3 => assignment.producer = ObservationProducer::CloudLive,
                    4 => assignment.occurrence.capture_epoch += 1,
                    _ => (),
                }
                ledger.record_assigned_word_pins(
                    &assignment,
                    &[(neighbour.clone(), original_pin.clone())],
                );
                let expected = state == "word" && variant == 0;
                let coverage = ledger.group_speech_coverage(
                    &next,
                    &original,
                    &forensic_neighbour_words(&next),
                );
                assert_eq!(coverage.is_some(), expected, "{state}/{variant}");
                if let Some((_, parts)) = coverage {
                    let borrowed = parts.last().unwrap();
                    assert_eq!(borrowed.pin_owner, neighbour);
                    assert_eq!(borrowed.pin_range, original_pin);
                    assert_eq!(
                        (
                            borrowed.speech_range.sample_start,
                            borrowed.speech_range.sample_end
                        ),
                        (14_000, 16_000)
                    );
                }
                ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
                let pins = [
                    WordPin::new(2_000, 5_000, "czy").with_decode_window(0, 12_000),
                    WordPin::new(5_000, 8_000, "weryfikowałeś").with_decode_window(0, 12_000),
                ];
                assert!(
                    ledger.admit_word_slots(&next, &pins).grants_mutation(),
                    "{state}/{variant}"
                );
                assert_eq!(ledger.text_of(&owner), Some("czy weryfikowałeś"));
                assert_eq!(ledger.slots_of(&neighbour), neighbour_before.as_deref());
                assert_eq!(
                    !ledger.group_speech_coverages().is_empty(),
                    expected,
                    "{state}/{variant}"
                );
                assert!(
                    ledger
                        .slot_operations()
                        .iter()
                        .any(|operation| operation.sources.contains(&original))
                );
                ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
                assert_eq!(
                    ledger.text_recovery_pending(&owner),
                    !expected,
                    "{state}/{variant}"
                );
                if expected {
                    ledger.seal(&owner).unwrap();
                } else {
                    assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                }
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_neighbour_silence_deletion_retains_uncovered_source_debt() {
        use super::word_verdict::{WordVerdict, adjudicate_word_pcm};
        use crate::audio::capture_receipt::CaptureEvidenceIdentity;
        use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;
        for crossing_word in [true, false] {
            let (mut ledger, owner, neighbour, pcm) =
                forensic_neighbour_capture(&format!("neighbour-deleted-{crossing_word}"), 23, true);
            let original = ledger.slots_of(&owner).unwrap()[0].clone();
            let neighbour_observation =
                ObservationIdentity::new(ObservationProducer::Apple, 2, 0, neighbour.clone());
            // A crossing word has voiced source PCM before the owner fence.
            // A wholly quiet source starts at the fence and can be deleted.
            let start = if crossing_word { 14_000 } else { 16_000 };
            assert!(
                ledger
                    .admit_word_slots(
                        &neighbour_observation,
                        &[WordPin::new(start, 20_000, "sąsiad").with_decode_window(0, 32_000)]
                    )
                    .grants_mutation()
            );
            let held = ledger.slots_of(&neighbour).unwrap()[0].clone();
            let next = ObservationIdentity::new(ObservationProducer::Whisper, 3, 1, owner.clone());
            let assign = [(
                neighbour.clone(),
                OccurrenceIdentity::new(&owner.session, 23, 14_000, 20_000),
            )];
            ledger.record_assigned_word_pins(&next, &assign);
            assert_eq!(
                ledger
                    .group_speech_coverage(&next, &original, &forensic_neighbour_words(&next))
                    .is_some(),
                crossing_word
            );
            let quiet = OccurrenceIdentity::new(&owner.session, 23, 16_000, 20_000);
            // Typed Silero fixture matches this PCM's measured voiced ranges.
            let ranges = [(2_000, 8_000), (14_000, 16_000)]
                .into_iter()
                .map(
                    |(sample_start, sample_end)| crate::stt::tail_provider::TailSampleRange {
                        session: owner.session.clone(),
                        capture_epoch: owner.capture_epoch,
                        sample_start,
                        sample_end,
                    },
                )
                .collect();
            let silero = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(&owner.session, 23),
                SILERO_BOUNDARIES_PRODUCER,
                AcousticAvailability::Observed {
                    observed_samples: 32_000,
                },
                ranges,
            );
            let verdict = adjudicate_word_pcm(&quiet, &quiet, &pcm[16_000..20_000], &silero);
            assert_eq!(verdict.verdict(), WordVerdict::ConfirmedNoSpeech);
            let deletion = ledger.remove_word_with_verdict(
                &ObservationIdentity::new(ObservationProducer::Whisper, 4, 0, neighbour.clone()),
                &SlotTarget::from(&held),
                &verdict,
            );
            if crossing_word {
                assert_eq!(deletion, Err(SlotOperationRefusal::InvalidEvidence));
                assert_eq!(
                    ledger.slots_of(&neighbour).unwrap(),
                    std::slice::from_ref(&held)
                );
            } else {
                assert_eq!(deletion.unwrap().operation.sources, [held]);
                assert!(ledger.slots_of(&neighbour).unwrap().is_empty());
            }
            ledger.record_assigned_word_pins(&next, &assign);
            assert_eq!(
                ledger
                    .group_speech_coverage(&next, &original, &forensic_neighbour_words(&next))
                    .is_some(),
                crossing_word
            );
            ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
            let pins = [
                WordPin::new(2_000, 5_000, "czy").with_decode_window(0, 12_000),
                WordPin::new(5_000, 8_000, "weryfikowałeś").with_decode_window(0, 12_000),
            ];
            assert!(ledger.admit_word_slots(&next, &pins).grants_mutation());
            assert_eq!(ledger.text_of(&owner), Some("czy weryfikowałeś"));
            assert_eq!(!ledger.group_speech_coverages().is_empty(), crossing_word);
            assert_eq!(ledger.word_deletions().len(), usize::from(!crossing_word));
            ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
            assert_eq!(ledger.text_recovery_pending(&owner), !crossing_word);
            if crossing_word {
                ledger.seal(&owner).unwrap();
            } else {
                assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                assert_eq!(
                    ledger.seal_terminal(&owner.session, owner.capture_epoch),
                    Err(SealRefusal::TextRecoveryPending)
                );
            }
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    fn forensic_coarse_fixture(outer_pin: bool) -> (AcousticLedger, AcousticSpeechEvidence) {
        forensic_group_fixture(outer_pin, "czy plan weryfikowałeś")
    }

    fn forensic_group_fixture(
        outer_pin: bool,
        label: &str,
    ) -> (AcousticLedger, AcousticSpeechEvidence) {
        let (measured, occurrence, _, _) = forensic_neighbour_capture("slot-test", 1, true);
        assert_eq!(occurrence, owner());
        let serial = measured.serial_of(&occurrence).unwrap();
        let calibration =
            EnergyCalibration::new(serial.evidence_calibration_version.clone(), 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: serial.duration_ms,
            energy_integral: serial.energy_integral,
            mean_rms_dbfs: serial.mean_rms_dbfs,
            peak_dbfs: serial.peak_dbfs,
            vad_open_sample: serial.vad_open_sample,
            vad_close_sample: serial.vad_close_sample,
            evidence_calibration_version: serial.evidence_calibration_version.clone(),
        };
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let apple = observation(ObservationProducer::Apple, 0);
        let receipt = if outer_pin {
            ledger.admit_pinned_label(&apple, label, &[WordPin::new(0, 16_000, label)])
        } else {
            ledger.admit(&apple, label)
        };
        assert!(receipt.grants_mutation());
        (ledger, measured.speech_evidence.unwrap())
    }

    // Root-owned decode-scope controls. Energy is measured from fixture PCM;
    // returned words are supplied decoder fixtures, not ASR/model acceptance.
    #[test]
    fn confirmed_decode_requires_admitted_operation_not_lexical_eligibility() {
        for commit in [false, true] {
            let (mut ledger, owner, _, _) =
                forensic_neighbour_capture("confirmed-operation", 9, true);
            let initial = ledger.next_word_observation(ObservationProducer::Whisper, 901, &owner);
            let initial_pin = [WordPin::new(2_000, 16_000, "Iwo").with_decode_window(0, 20_000)];
            assert!(
                ledger
                    .admit_word_slots(&initial, &initial_pin)
                    .grants_mutation()
            );
            let earlier = ledger.next_word_observation(ObservationProducer::Whisper, 902, &owner);
            let hypothesis = [WordPin::new(2_000, 16_000, "Kamil").with_decode_window(0, 24_000)];
            assert!(
                !ledger
                    .admit_word_slots(&earlier, &hypothesis)
                    .grants_mutation()
            );
            let trial = ledger.next_word_trial(true).expect("correction trial");
            let returned = ledger.next_word_observation(ObservationProducer::Whisper, 903, &owner);
            let pins = [WordPin::new(2_000, 16_000, "Kamil").with_decode_window(0, 32_000)];
            let before = ledger.slots_of(&owner).unwrap().to_vec();
            if commit {
                assert!(
                    ledger
                        .admit_word_trial(&trial, &returned, &pins, &pins)
                        .grants_mutation()
                );
            } else {
                // Exercise the real proposal phase without applying the proposed
                // operation. Eligibility must not authenticate completed work.
                ledger.stage_word_evidence(&returned, &pins, None, "unknown");
                let mut evidence = ledger.word_evidence_input(&returned).unwrap().clone();
                evidence.trial = Some(trial);
                ledger.restore_word_evidence(evidence);
                ledger.record_word_decode_bounds(&returned, &pins);
                let outputs = [WordSlot {
                    sample_start: 2_000,
                    sample_end: 16_000,
                    text: "Kamil".into(),
                    producer: returned.producer,
                    observation: returned.clone(),
                    witness: SlotWitness::Unwitnessed,
                    confidence: None,
                    surface_rewritten: false,
                }];
                assert_eq!(
                    ledger.adjudicate_word_sources(&returned, &before, &outputs),
                    Some(true)
                );
                assert_eq!(ledger.slots_of(&owner).unwrap(), before);
                assert!(
                    !ledger
                        .slot_operations()
                        .iter()
                        .any(|op| op.observation == returned)
                );
            }
            let choice = ledger.word_choices().last().unwrap();
            assert!(choice.accepted && choice.lexical_resolved);
            assert_eq!(choice.reason, "trial_confirmed");
            assert_eq!(ledger.confirmed_word_decode(&earlier, 0, 24_000), commit);
            assert_eq!(
                ledger.text_of(&owner),
                Some(if commit { "Kamil" } else { "Iwo" })
            );
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_scope_sparse_word_times_distinguish_partial_and_complete_work() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for window in [None, Some((0, 12_000)), Some((0, 16_000))] {
                let (mut ledger, owner, _, _) =
                    forensic_neighbour_capture("scope-control", 9, true);
                let speech = ledger.speech_evidence.clone().unwrap();
                let source = ledger.slots_of(&owner).unwrap().to_vec();
                ledger.schedule_frontier(owner.clone(), [producer]);
                assert!(ledger.require_text_recovery(&owner));
                let observation = ledger.next_word_observation(producer, 92, &owner);
                let pins = [
                    WordPin::new(2_000, 5_000, "czy"),
                    WordPin::new(5_000, 8_000, "weryfikowałeś"),
                ]
                .into_iter()
                .map(|pin| match window {
                    Some((lo, hi)) => pin.with_decode_window(lo, hi),
                    None => pin,
                })
                .collect::<Vec<_>>();
                let receipt = ledger.admit_word_slots(&observation, &pins);
                assert!(
                    receipt.grants_mutation()
                        || matches!(receipt, MutationReceipt::Preserve { .. }),
                    "{producer:?} {window:?}: {receipt:?}"
                );
                assert_eq!(ledger.text_of(&owner), Some("czy weryfikowałeś"));
                let operation = ledger.slot_operations().last().unwrap();
                assert_eq!(operation.sources, source);
                assert_eq!(operation.outputs.len(), 2);
                assert_eq!(
                    (
                        operation.outputs[0].sample_start,
                        operation.outputs[1].sample_end
                    ),
                    (2_000, 8_000)
                );
                assert!(operation.source_ranges.contains(&owner));
                assert!(
                    ledger.group_speech_coverages().is_empty(),
                    "work completion is not a speech-density receipt"
                );
                let complete = window == Some((0, 16_000));
                assert_eq!(ledger.text_recovery_pending(&owner), !complete);
                assert!(ledger.note_frontier_return(&owner, producer));
                let coverage =
                    ledger.assess_seal_coverage(&owner.session, owner.capture_epoch, &speech, 0);
                assert!(ledger.record_seal_coverage(coverage.clone()));
                if complete {
                    assert_eq!(coverage.status, SealCoverageStatus::Complete);
                    ledger
                        .seal(&owner)
                        .expect("full actual decoder scope returned");
                    ledger
                        .seal_terminal(&owner.session, owner.capture_epoch)
                        .expect("valid observer and full scope");
                } else {
                    assert!(
                        ledger.text_recovery_pending(&owner),
                        "frontier return cannot decode the missing scope"
                    );
                    assert_eq!(coverage.status, SealCoverageStatus::Incomplete);
                    assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                    assert_eq!(
                        ledger.seal_terminal(&owner.session, owner.capture_epoch),
                        Err(SealRefusal::TextRecoveryPending)
                    );
                }
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_scope_complete_decode_does_not_upgrade_an_unusable_observer() {
        use crate::audio::capture_receipt::CaptureEvidenceIdentity;
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for window in [None, Some((0, 12_000)), Some((0, 16_000))] {
                for variant in 0..9 {
                    let (mut ledger, owner, _, _) =
                        forensic_neighbour_capture("observer-control", 9, true);
                    let measured = ledger.speech_evidence.clone().unwrap();
                    let bad = match variant {
                        0..=3 => AcousticSpeechEvidence::measured(
                            measured.identity().clone(),
                            measured.producer(),
                            match variant {
                                0 => AcousticAvailability::NotObserved,
                                1 => AcousticAvailability::Discontinuous {
                                    observed_samples: 12_000,
                                },
                                2 => AcousticAvailability::InvalidMeasurement {
                                    valid_samples: 12_000,
                                },
                                _ => AcousticAvailability::Observed {
                                    observed_samples: 15_999,
                                },
                            },
                            measured.ranges().to_vec(),
                        ),
                        4 => AcousticSpeechEvidence::measured(
                            CaptureEvidenceIdentity::new(&owner.session, owner.capture_epoch + 1),
                            measured.producer(),
                            measured.availability(),
                            measured.ranges().to_vec(),
                        ),
                        5 => AcousticSpeechEvidence::measured(
                            measured.identity().clone(),
                            "text_producer",
                            measured.availability(),
                            measured.ranges().to_vec(),
                        ),
                        _ => {
                            let mut ranges = measured.ranges().to_vec();
                            match variant {
                                6 => ranges[1].sample_start = ranges[1].sample_end + 1,
                                7 => ranges[1].sample_end = 32_001,
                                _ => ranges[1].capture_epoch += 1,
                            }
                            AcousticSpeechEvidence::measured(
                                measured.identity().clone(),
                                measured.producer(),
                                measured.availability(),
                                ranges,
                            )
                        }
                    };
                    let source = ledger.slots_of(&owner).unwrap().to_vec();
                    ledger.record_speech_evidence(&bad);
                    ledger.schedule_frontier(owner.clone(), [producer]);
                    assert!(ledger.require_text_recovery(&owner));
                    let observation = ledger.next_word_observation(producer, 93, &owner);
                    let pins = [
                        WordPin::new(2_000, 5_000, "czy"),
                        WordPin::new(5_000, 8_000, "weryfikowałeś"),
                    ]
                    .into_iter()
                    .map(|pin| match window {
                        Some((lo, hi)) => pin.with_decode_window(lo, hi),
                        None => pin,
                    })
                    .collect::<Vec<_>>();
                    let decision = ledger.admit_word_slots(&observation, &pins);
                    let complete = window == Some((0, 16_000));
                    assert!(ledger.group_speech_coverages().is_empty());
                    assert_eq!(
                        ledger.text_recovery_pending(&owner),
                        !complete,
                        "{producer:?} {window:?} variant{variant}: {decision:?}"
                    );
                    if complete {
                        assert_eq!(ledger.text_of(&owner), Some("czy weryfikowałeś"));
                        assert!(
                            decision.grants_mutation()
                                || matches!(decision, MutationReceipt::Preserve { .. })
                        );
                    } else {
                        assert_eq!(
                            ledger.slots_of(&owner).unwrap(),
                            source,
                            "invalid observer cannot support partial partition"
                        );
                    }
                    assert!(ledger.note_frontier_return(&owner, producer));
                    let coverage =
                        ledger.assess_seal_coverage(&owner.session, owner.capture_epoch, &bad, 0);
                    assert!(
                        matches!(coverage.status, SealCoverageStatus::Unavailable(_)),
                        "variant{variant}: {coverage:?}"
                    );
                    assert_eq!(coverage.coverage_ratio(), None);
                    assert!(ledger.record_seal_coverage(coverage));
                    if complete {
                        ledger
                            .seal(&owner)
                            .expect("known full decoder work can seal its qualified owner");
                        assert_eq!(
                            ledger.seal_terminal(&owner.session, owner.capture_epoch),
                            Err(SealRefusal::CoverageIncomplete)
                        );
                        ledger.record_speech_evidence(&measured);
                        let valid = ledger.assess_seal_coverage(
                            &owner.session,
                            owner.capture_epoch,
                            &measured,
                            0,
                        );
                        assert_eq!(valid.status, SealCoverageStatus::Complete);
                        assert!(ledger.record_seal_coverage(valid));
                        ledger
                            .seal_terminal(&owner.session, owner.capture_epoch)
                            .expect("later actual valid observer");
                    } else {
                        assert!(ledger.text_recovery_pending(&owner));
                        assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                        assert_eq!(
                            ledger.seal_terminal(&owner.session, owner.capture_epoch),
                            Err(SealRefusal::TextRecoveryPending)
                        );
                    }
                    ledger.assert_slot_labels();
                    assert_eq!(ledger.conservation().residue(), 0);
                }
            }
        }
    }

    // Root-owned Relay controls: five physical sources, not a lexical word floor.
    fn forensic_relay_five_capture(
        session: &str,
    ) -> (AcousticLedger, Vec<OccurrenceIdentity>, Vec<f32>) {
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        let owners = (0..5)
            .map(|index| {
                OccurrenceIdentity::new(session, 31, index * 24_000 + 3_200, index * 24_000 + 9_600)
            })
            .collect::<Vec<_>>();
        let mut pcm = vec![0.0_f32; 120_000];
        for owner in &owners {
            pcm[owner.sample_start as usize..owner.sample_end as usize].fill(0.2);
        }
        let energy = CaptureEnergyOwner::bind(session, 31);
        let mut writer = CaptureLevelAccumulator::bound_to(&energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let speech = energy.session_active_speech_ranges(session, 31, 16_000);
        assert_eq!(
            speech.availability().observed_samples(),
            Some(pcm.len() as u64)
        );
        assert_eq!(
            speech
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect::<Vec<_>>(),
            owners
                .iter()
                .map(|owner| (owner.sample_start, owner.sample_end))
                .collect::<Vec<_>>()
        );
        let calibration = EnergyCalibration::new("forensic-relay-five-pcm", 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        for owner in &owners {
            let samples = &pcm[owner.sample_start as usize..owner.sample_end as usize];
            let integral = samples
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>();
            let peak = samples
                .iter()
                .map(|sample| f64::from(sample.abs()))
                .fold(0.0_f64, f64::max);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: owner.clone(),
                            duration_ms: samples.len() as f64 / 16.0,
                            energy_integral: integral,
                            mean_rms_dbfs: 20.0 * (integral / samples.len() as f64).sqrt().log10(),
                            peak_dbfs: 20.0 * peak.log10(),
                            vad_open_sample: Some(owner.sample_start),
                            vad_close_sample: Some(owner.sample_end),
                            evidence_calibration_version: calibration.version.clone()
                        },
                        &calibration
                    )
                    .is_qualified()
            );
        }
        ledger.record_speech_evidence(&speech);
        (ledger, owners, pcm)
    }

    fn forensic_relay_word(owner: &OccurrenceIdentity, text: &str) -> WordPin {
        WordPin::new(owner.sample_start + 320, owner.sample_end - 320, text)
            .with_decode_window(owner.sample_start, owner.sample_end)
    }

    fn forensic_relay_word_with_actual_quiet_margin(
        owner: &OccurrenceIdentity,
        text: &str,
        pcm: &[f32],
    ) -> WordPin {
        assert!(owner.sample_start >= 320 && owner.sample_end + 320 <= pcm.len() as u64);
        assert!(
            pcm[owner.sample_start as usize - 320..owner.sample_start as usize]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert!(
            pcm[owner.sample_end as usize..owner.sample_end as usize + 320]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        forensic_relay_word(owner, text)
            .with_decode_window(owner.sample_start - 320, owner.sample_end + 320)
    }

    #[test]
    fn forensic_relay_five_coarse_hypotheses_allow_heard_word_without_losing_source() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for outer_pin in [false, true] {
                for held in [
                    "czy plan weryfikowałeś",
                    "Iwo Iwo Iwo Iwo Iwo",
                    "czy yyy [śmiech głośny] plan",
                ] {
                    let (mut ledger, owners, pcm) =
                        forensic_relay_five_capture("relay-coarse-five");
                    for owner in &owners {
                        let apple = ObservationIdentity::new(
                            ObservationProducer::Apple,
                            101,
                            0,
                            owner.clone(),
                        );
                        let receipt = if outer_pin {
                            ledger.admit_word_slots(
                                &apple,
                                &[WordPin::new(owner.sample_start, owner.sample_end, held)],
                            )
                        } else {
                            ledger.admit(&apple, held)
                        };
                        assert!(receipt.grants_mutation());
                        let source = ledger.slots_of(owner).unwrap().to_vec();
                        assert_eq!(source.len(), 1);
                        assert_eq!(source[0].text, held);
                        ledger.schedule_frontier(owner.clone(), [producer]);
                        assert!(ledger.require_text_recovery(owner));
                        let next = ledger.next_word_observation(producer, 102, owner);
                        let pin = forensic_relay_word_with_actual_quiet_margin(owner, "Iwo", &pcm);
                        assert!(
                            pcm[owner.sample_start as usize..owner.sample_end as usize]
                                .iter()
                                .all(|sample| *sample == 0.2)
                        );
                        let decision = ledger.admit_word_slots(&next, std::slice::from_ref(&pin));
                        assert!(
                            decision.grants_mutation(),
                            "{producer:?} outer={outer_pin} held={held}: {decision:?}"
                        );
                        let output = ledger.slots_of(owner).unwrap();
                        assert_eq!(output.len(), 1);
                        assert_eq!(
                            (output[0].sample_start, output[0].sample_end),
                            (pin.sample_start, pin.sample_end)
                        );
                        assert_eq!(output[0].text, "Iwo");
                        assert_eq!(output[0].observation, next);
                        let operations = ledger
                            .slot_operations()
                            .iter()
                            .filter(|operation| {
                                operation.observation == next && operation.sources == source
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(operations.len(), 1);
                        assert_eq!(operations[0].outputs, output);
                        assert!(operations[0].source_ranges.contains(owner));
                        assert!(ledger.slot_descends_from(&output[0], &source[0]));
                        assert_eq!(
                            ledger.slot_source_ranges(&output[0]),
                            vec![OccurrenceIdentity::new(
                                &owner.session,
                                owner.capture_epoch,
                                pin.sample_start,
                                pin.sample_end
                            )]
                        );
                        assert!(
                            ledger.group_speech_coverages().is_empty(),
                            "complete work is not dense word timestamps"
                        );
                        assert!(!ledger.text_recovery_pending(owner));
                        assert!(ledger.note_frontier_return(owner, producer));
                        ledger.seal(owner).unwrap();
                        ledger.assert_slot_labels();
                        assert_eq!(ledger.conservation().residue(), 0);
                    }
                    assert_eq!(ledger.len(), 5);
                    assert!(owners.iter().all(
                        |owner| ledger.text_of(owner) == Some("Iwo") && ledger.is_sealed(owner)
                    ));
                    let speech = ledger.speech_evidence.clone().unwrap();
                    let coverage = ledger.assess_seal_coverage("relay-coarse-five", 31, &speech, 0);
                    assert_eq!(coverage.status, SealCoverageStatus::Complete);
                    assert!(ledger.record_seal_coverage(coverage));
                    ledger.seal_terminal("relay-coarse-five", 31).unwrap();
                }
            }
        }
    }

    #[test]
    fn forensic_relay_five_word_sources_survive_empty_and_decode_cut_until_recovery() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for cut in [false, true] {
                let (mut ledger, owners, pcm) = forensic_relay_five_capture("relay-word-five");
                for (index, owner) in owners.iter().enumerate() {
                    let apple =
                        ObservationIdentity::new(ObservationProducer::Apple, 201, 0, owner.clone());
                    assert!(
                        ledger
                            .admit_word_slots(
                                &apple,
                                &[forensic_relay_word_with_actual_quiet_margin(
                                    owner, "Iwo", &pcm
                                )]
                            )
                            .grants_mutation()
                    );
                    let source = ledger.slots_of(owner).unwrap().to_vec();
                    assert!(ledger.complete_word_slot(&source[0]));
                    ledger.schedule_frontier(owner.clone(), [producer]);
                    assert!(ledger.require_text_recovery(owner));
                    let next = ledger.next_word_observation(producer, 202, owner);
                    if index == 4 {
                        let pins = if cut {
                            vec![
                                WordPin::new(
                                    owner.sample_start + 480,
                                    owner.sample_end - 320,
                                    "urwany",
                                )
                                .with_decode_window(owner.sample_start + 480, owner.sample_end),
                            ]
                        } else {
                            Vec::new()
                        };
                        ledger.admit_word_slots(&next, &pins);
                        assert_eq!(ledger.slots_of(owner).unwrap(), source);
                        assert_eq!(ledger.text_of(owner), Some("Iwo"));
                        assert!(ledger.text_recovery_pending(owner));
                        assert!(ledger.note_frontier_return(owner, producer));
                        assert_eq!(ledger.seal(owner), Err(SealRefusal::TextRecoveryPending));
                    } else {
                        let decision = ledger.admit_word_slots(
                            &next,
                            &[forensic_relay_word_with_actual_quiet_margin(
                                owner, "Iwo", &pcm,
                            )],
                        );
                        assert!(
                            decision.grants_mutation()
                                || matches!(decision, MutationReceipt::Preserve { .. })
                        );
                        assert!(!ledger.text_recovery_pending(owner));
                        assert!(ledger.note_frontier_return(owner, producer));
                        ledger.seal(owner).unwrap();
                    }
                }
                assert_eq!(ledger.len(), 5);
                assert!(
                    owners
                        .iter()
                        .all(|owner| ledger.text_of(owner) == Some("Iwo"))
                );
                let speech = ledger.speech_evidence.clone().unwrap();
                let coverage = ledger.assess_seal_coverage("relay-word-five", 31, &speech, 0);
                assert_eq!(coverage.status, SealCoverageStatus::Incomplete);
                assert!(ledger.record_seal_coverage(coverage));
                assert_eq!(
                    ledger.seal_terminal("relay-word-five", 31),
                    Err(SealRefusal::TextRecoveryPending)
                );
                let last = owners.last().unwrap();
                let original = ledger.slots_of(last).unwrap().to_vec();
                let recovery = ledger.next_word_observation(producer, 203, last);
                ledger.admit_word_slots(
                    &recovery,
                    &[forensic_relay_word_with_actual_quiet_margin(
                        last, "Iwo", &pcm,
                    )],
                );
                assert!(!ledger.text_recovery_pending(last));
                assert!(
                    ledger
                        .slot_source_ranges(&ledger.slots_of(last).unwrap()[0])
                        .contains(&OccurrenceIdentity::new(
                            "relay-word-five",
                            31,
                            original[0].sample_start,
                            original[0].sample_end
                        ))
                );
                ledger.seal(last).unwrap();
                let coverage = ledger.assess_seal_coverage("relay-word-five", 31, &speech, 0);
                assert_eq!(coverage.status, SealCoverageStatus::Complete);
                assert!(ledger.record_seal_coverage(coverage));
                ledger.seal_terminal("relay-word-five", 31).unwrap();
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_relay_human_and_passed_layers_cannot_be_rewritten_by_acoustic_labels() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for state in ["human", "sealed", "lower"] {
                let (mut ledger, owners, pcm) = forensic_relay_five_capture("relay-protected-five");
                for owner in &owners {
                    let first =
                        ObservationIdentity::new(ObservationProducer::Apple, 301, 0, owner.clone());
                    ledger.admit_word_slots(&first, &[forensic_relay_word(owner, "Iwo")]);
                    if state == "human" {
                        let human = ledger.next_word_observation(
                            ObservationProducer::ManualHuman,
                            302,
                            owner,
                        );
                        assert!(
                            ledger
                                .admit_word_slots(&human, &[forensic_relay_word(owner, "ręcznie")])
                                .grants_mutation()
                        );
                    } else if state == "lower" {
                        let mut next = ledger.next_word_observation(producer, 302, owner);
                        assert!(
                            corroborate_candidate(
                                &mut ledger,
                                &mut next,
                                &[forensic_relay_word_with_actual_quiet_margin(
                                    owner,
                                    "poprawione",
                                    &pcm
                                )],
                                (0, pcm.len() as u64),
                            )
                            .grants_mutation()
                        );
                    } else {
                        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
                        assert!(ledger.note_frontier_return(owner, ObservationProducer::Apple));
                        ledger.seal(owner).unwrap();
                    }
                }
                // Complete setup before offering refusals: those refusals now
                // retain real conflicts for their own words and later trials.
                for owner in &owners {
                    let source = ledger.slots_of(owner).unwrap().to_vec();
                    let lineage = ledger.slot_source_ranges(&source[0]);
                    let offered = if state == "lower" {
                        ObservationProducer::Apple
                    } else {
                        producer
                    };
                    let next = ledger.next_word_observation(offered, 303, owner);
                    let decision = ledger.admit_word_slots(
                        &next,
                        &[WordPin::new(
                            owner.sample_start,
                            owner.sample_end,
                            "czy zmieniam wszystko",
                        )],
                    );
                    assert!(
                        !decision.grants_mutation(),
                        "{producer:?}/{state}: {decision:?}"
                    );
                    assert_eq!(ledger.slots_of(owner).unwrap(), source);
                    assert_eq!(ledger.slot_source_ranges(&source[0]), lineage);
                    assert!(ledger.word_deletions().is_empty());
                    ledger.assert_slot_labels();
                    assert_eq!(ledger.conservation().residue(), 0);
                }
                assert_eq!(ledger.len(), 5);
            }
        }
    }

    // Root-owned merge controls: source relationships, not output word count.
    fn forensic_merge_capture(
        session: &str,
        count: u64,
    ) -> (AcousticLedger, OccurrenceIdentity, Vec<f32>) {
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        let owner = OccurrenceIdentity::new(session, 37, 3_200, 9_600);
        let mut pcm = vec![0.0_f32; 16_000];
        pcm[3_200..9_600].fill(0.2);
        let energy = CaptureEnergyOwner::bind(session, 37);
        let mut writer = CaptureLevelAccumulator::bound_to(&energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let speech = energy.session_active_speech_ranges(session, 37, 16_000);
        assert_eq!(
            speech.availability().observed_samples(),
            Some(pcm.len() as u64)
        );
        assert_eq!(
            speech
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect::<Vec<_>>(),
            [(3_200, 9_600)]
        );
        let samples = &pcm[owner.sample_start as usize..owner.sample_end as usize];
        let integral = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        let peak = samples
            .iter()
            .map(|sample| f64::from(sample.abs()))
            .fold(0.0_f64, f64::max);
        let calibration = EnergyCalibration::new("forensic-merge-pcm", 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: samples.len() as f64 / 16.0,
                        energy_integral: integral,
                        mean_rms_dbfs: 20.0 * (integral / samples.len() as f64).sqrt().log10(),
                        peak_dbfs: 20.0 * peak.log10(),
                        vad_open_sample: Some(3_200),
                        vad_close_sample: Some(9_600),
                        evidence_calibration_version: calibration.version.clone()
                    },
                    &calibration
                )
                .is_qualified()
        );
        ledger.record_speech_evidence(&speech);
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 701, 0, owner.clone());
        let width = 6_400 / count;
        assert_eq!(width * count, 6_400);
        let pins = (0..count)
            .map(|index| {
                WordPin::new(
                    3_200 + index * width,
                    3_200 + (index + 1) * width,
                    if count == 2 {
                        if index == 0 { "na" } else { "prawdę" }
                    } else {
                        "Iwo"
                    },
                )
                .with_decode_window(0, pcm.len() as u64)
            })
            .collect::<Vec<_>>();
        assert!(ledger.admit_word_slots(&apple, &pins).grants_mutation());
        let slots = ledger.slots_of(&owner).unwrap();
        assert_eq!(slots.len(), count as usize);
        assert!(slots.iter().all(|slot| ledger.complete_word_slot(slot)));
        (ledger, owner, pcm)
    }

    #[test]
    fn forensic_merge_label_without_word_targets_preserves_five_sources() {
        for producer in [
            ObservationProducer::Whisper,
            ObservationProducer::CloudLive,
            ObservationProducer::Formatter,
        ] {
            let (mut ledger, owner, _) = forensic_merge_capture("merge-no-targets", 5);
            let source = ledger.slots_of(&owner).unwrap().to_vec();
            let lineage = source
                .iter()
                .map(|slot| ledger.slot_source_ranges(slot))
                .collect::<Vec<_>>();
            let next = ledger.next_word_observation(producer, 702, &owner);
            let decision = ledger.admit_pinned_label(&next, "Iwo", &[]);
            assert!(!decision.grants_mutation());
            assert_eq!(ledger.slots_of(&owner).unwrap(), source);
            assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
            assert_eq!(
                source
                    .iter()
                    .map(|slot| ledger.slot_source_ranges(slot))
                    .collect::<Vec<_>>(),
                lineage
            );
            assert!(ledger.word_deletions().is_empty());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_merge_coarse_phrase_split_cannot_license_five_word_contraction() {
        let (mut ledger, owner, pcm) = forensic_split_empty_capture("coarse-parent-five");
        let first = ObservationIdentity::new(ObservationProducer::Apple, 1001, 0, owner.clone());
        assert!(
            ledger
                .admit(&first, "Iwo Iwo Iwo Iwo Iwo")
                .grants_mutation()
        );
        let parent = ledger.slots_of(&owner).unwrap()[0].clone();
        assert!(!ledger.complete_word_slot(&parent));
        let split = ObservationIdentity::new(ObservationProducer::Whisper, 1002, 0, owner.clone());
        let pins = (0..5)
            .map(|i| {
                WordPin::new(3200 + i * 1280, 3200 + (i + 1) * 1280, "Iwo")
                    .with_decode_window(0, pcm.len() as u64)
            })
            .collect::<Vec<_>>();
        let split_decision = ledger.admit_word_slots(&split, &pins);
        println!(
            "coarse split decision={split_decision:?} slots={:?} ops={:?}",
            ledger.slots_of(&owner),
            ledger.slot_operations()
        );
        let sources = ledger.slots_of(&owner).unwrap().to_vec();
        assert_eq!(sources.len(), 5);
        assert!(sources.iter().all(|s| ledger.complete_word_slot(s)));
        let operation = ledger.slot_operations().last().unwrap();
        assert_eq!(operation.kind, SlotOperationKind::Split);
        assert!(operation.rule_id.starts_with("acoustic_resegmentation/"));
        assert_eq!(operation.sources, vec![parent]);
        assert_eq!(operation.outputs, sources);
        let lineage = sources
            .iter()
            .map(|s| ledger.slot_source_ranges(s))
            .collect::<Vec<_>>();
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(ledger.require_text_recovery(&owner));
        let next = ObservationIdentity::new(ObservationProducer::Whisper, 1002, 1, owner.clone());
        let decision = ledger.admit_word_slots(
            &next,
            &[WordPin::new(3200, 9600, "Iwo").with_decode_window(0, pcm.len() as u64)],
        );
        assert!(
            !decision.grants_mutation(),
            "coarse phrase ancestry cannot retire five complete word ranges: {decision:?}; slots={:?}; text={:?}",
            ledger.slots_of(&owner),
            ledger.text_of(&owner)
        );
        assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
        assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
        assert_eq!(
            sources
                .iter()
                .map(|s| ledger.slot_source_ranges(s))
                .collect::<Vec<_>>(),
            lineage
        );
        assert!(ledger.text_recovery_pending(&owner));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
        assert!(ledger.word_deletions().is_empty());
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    fn forensic_merge_capture_with_parent_tail(
        session: &str,
    ) -> (AcousticLedger, OccurrenceIdentity, Vec<f32>) {
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        let owner = OccurrenceIdentity::new(session, 37, 3_200, 11_200);
        let mut pcm = vec![0.0_f32; 16_000];
        pcm[3_200..11_200].fill(0.2);
        let energy = CaptureEnergyOwner::bind(session, 37);
        let mut writer = CaptureLevelAccumulator::bound_to(&energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let speech = energy.session_active_speech_ranges(session, 37, 16_000);
        assert_eq!(speech.availability().observed_samples(), Some(16_000));
        assert_eq!(
            speech
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect::<Vec<_>>(),
            [(3_200, 11_200)]
        );
        let samples = &pcm[3_200..11_200];
        let integral = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        let calibration = EnergyCalibration::new("forensic-parent-tail-pcm", 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: samples.len() as f64 / 16.0,
                        energy_integral: integral,
                        mean_rms_dbfs: 20.0 * (integral / samples.len() as f64).sqrt().log10(),
                        peak_dbfs: 20.0 * f64::from(0.2_f32).log10(),
                        vad_open_sample: Some(3_200),
                        vad_close_sample: Some(11_200),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        ledger.record_speech_evidence(&speech);
        (ledger, owner, pcm)
    }

    fn forensic_merge_preserve_five_split_children(
        mut ledger: AcousticLedger,
        owner: OccurrenceIdentity,
        pcm: Vec<f32>,
        parent: WordSlot,
        parent_has_tail: bool,
    ) {
        let mut split =
            ObservationIdentity::new(ObservationProducer::Whisper, 1102, 0, owner.clone());
        let pins = (0..5)
            .map(|i| {
                WordPin::new(3200 + i * 1280, 3200 + (i + 1) * 1280, "Iwo")
                    .with_decode_window(0, pcm.len() as u64)
            })
            .collect::<Vec<_>>();
        // Both contexts are slices of the measured capture. A request number
        // alone cannot turn the first frame into another lexical witness.
        let first_frame_end = 14_400;
        assert!(owner.sample_end < first_frame_end && first_frame_end < pcm.len() as u64);
        assert!(
            pcm[first_frame_end as usize..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        let first_pins = pins
            .iter()
            .cloned()
            .map(|pin| pin.with_decode_window(0, first_frame_end))
            .collect::<Vec<_>>();
        let first_decision = ledger.admit_word_slots(&split, &first_pins);
        let split_decision = if let Some(trial) = ledger.next_word_trial(true) {
            split = ledger.next_word_observation(ObservationProducer::Whisper, 1103, &owner);
            ledger.admit_word_trial(&trial, &split, &pins, &pins)
        } else {
            first_decision
        };
        let sources = ledger.slots_of(&owner).unwrap().to_vec();
        println!(
            "parent-tail={parent_has_tail} actual split decision={split_decision:?}; sources={sources:?}; operations={:?}",
            ledger.slot_operations()
        );
        assert_eq!(
            sources.len(),
            5,
            "actual production path must produce five children before testing contraction"
        );
        assert!(
            sources
                .iter()
                .all(|source| ledger.complete_word_slot(source))
        );
        assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
        let operation = ledger.slot_operations().last().unwrap().clone();
        assert_eq!(operation.kind, SlotOperationKind::Split);
        assert!(operation.rule_id.starts_with("acoustic_resegmentation/"));
        assert_eq!(operation.sources, vec![parent.clone()]);
        assert_eq!(operation.outputs, sources);
        if parent_has_tail {
            assert!(
                operation
                    .source_ranges
                    .iter()
                    .any(|range| range.sample_end > 9_600)
            );
            assert_eq!(operation.outputs.last().unwrap().sample_end, 9_600);
        } else {
            assert!(ledger.complete_word_slot(&parent));
        }
        let lineage = sources
            .iter()
            .map(|source| ledger.slot_source_ranges(source))
            .collect::<Vec<_>>();
        let operations_before = ledger.slot_operations().len();
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(ledger.require_text_recovery(&owner));
        let next = ledger.next_word_observation(ObservationProducer::Whisper, 1102, &owner);
        let decision = ledger.admit_word_slots(
            &next,
            &[WordPin::new(3_200, 9_600, "Iwo").with_decode_window(0, pcm.len() as u64)],
        );
        let after = ledger.slots_of(&owner).unwrap().to_vec();
        println!(
            "parent-tail={parent_has_tail} contraction={decision:?}; after={after:?}; text={:?}; recovery={}",
            ledger.text_of(&owner),
            ledger.text_recovery_pending(&owner)
        );
        assert!(
            !decision.grants_mutation(),
            "parent debt/completeness cannot retire five complete Word anchors: {decision:?}; after={after:?}"
        );
        assert_eq!(after, sources);
        assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
        assert_eq!(
            sources
                .iter()
                .map(|source| ledger.slot_source_ranges(source))
                .collect::<Vec<_>>(),
            lineage
        );
        assert_eq!(ledger.slot_operations().len(), operations_before);
        assert!(ledger.text_recovery_pending(&owner));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(
            ledger.len(),
            1,
            "one qualified owner contains five independent word ranges"
        );
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_merge_partial_coarse_parent_tail_cannot_retire_five_complete_words() {
        let (mut ledger, owner, pcm) =
            forensic_merge_capture_with_parent_tail("partial-coarse-parent-five");
        let first = ObservationIdentity::new(ObservationProducer::Apple, 1101, 0, owner.clone());
        assert!(
            ledger
                .admit(&first, "Iwo Iwo Iwo Iwo Iwo")
                .grants_mutation()
        );
        let parent = ledger.slots_of(&owner).unwrap()[0].clone();
        assert!(!ledger.complete_word_slot(&parent));
        assert_eq!((parent.sample_start, parent.sample_end), (3_200, 11_200));
        forensic_merge_preserve_five_split_children(ledger, owner, pcm, parent, true);
    }

    #[test]
    fn forensic_merge_complete_parent_split_cannot_retire_five_later_word_anchors() {
        let (ledger, owner, pcm) = forensic_merge_capture("complete-parent-five", 1);
        let parent = ledger.slots_of(&owner).unwrap()[0].clone();
        assert!(ledger.complete_word_slot(&parent));
        forensic_merge_preserve_five_split_children(ledger, owner, pcm, parent, false);
    }

    #[test]
    fn forensic_merge_complete_wide_pin_cannot_collapse_five_words_from_one_decode() {
        let (mut ledger, owner, pcm) = forensic_merge_capture("merge-five-one-decode", 5);
        let sources = ledger.slots_of(&owner).unwrap().to_vec();
        assert_eq!(sources.len(), 5);
        assert!(
            sources
                .iter()
                .all(|word| word.observation == sources[0].observation)
        );
        assert!(sources.iter().all(|word| ledger.complete_word_slot(word)));
        let exact_ranges = (0..5)
            .map(|index| (3_200 + index * 1_280, 3_200 + (index + 1) * 1_280))
            .collect::<Vec<_>>();
        assert_eq!(
            sources
                .iter()
                .map(|word| (word.sample_start, word.sample_end))
                .collect::<Vec<_>>(),
            exact_ranges
        );
        let lineage = sources
            .iter()
            .map(|word| ledger.slot_source_ranges(word))
            .collect::<Vec<_>>();
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(ledger.require_text_recovery(&owner));
        let next = ledger.next_word_observation(ObservationProducer::Whisper, 707, &owner);
        let decision = ledger.admit_word_slots(
            &next,
            &[WordPin::new(3_200, 9_600, "Iwo").with_decode_window(0, pcm.len() as u64)],
        );
        assert!(
            !decision.grants_mutation(),
            "one earlier observation does not authorize retiring five complete PCM words: {decision:?}; slots={:?}; rendered={:?}",
            ledger.slots_of(&owner),
            ledger.text_of(&owner)
        );
        assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
        assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
        assert_eq!(
            sources
                .iter()
                .map(|word| ledger.slot_source_ranges(word))
                .collect::<Vec<_>>(),
            lineage
        );
        assert!(ledger.text_recovery_pending(&owner));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
        assert!(ledger.word_deletions().is_empty());
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_merge_complete_decode_alone_cannot_retire_complete_word_sources() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, pcm) = forensic_merge_capture("merge-actual-scope", 2);
            let sources = ledger.slots_of(&owner).unwrap().to_vec();
            ledger.schedule_frontier(owner.clone(), [producer]);
            assert!(ledger.require_text_recovery(&owner));
            let next = ledger.next_word_observation(producer, 703, &owner);
            let decision = ledger.admit_word_slots(
                &next,
                &[WordPin::new(3_200, 9_600, "naprawdę").with_decode_window(0, pcm.len() as u64)],
            );
            assert!(!decision.grants_mutation());
            assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
            assert_eq!(ledger.text_of(&owner), Some("na prawdę"));
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .any(|alternative| alternative.observation == next
                        && alternative.candidate == "naprawdę"
                        && alternative.sources == sources)
            );
            assert!(ledger.text_recovery_pending(&owner));
            assert!(ledger.note_frontier_return(&owner, producer));
            assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
            assert!(ledger.word_deletions().is_empty());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_merge_valid_geometry_cannot_override_human_seal_or_formatter_boundary() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for state in ["human", "sealed", "formatter"] {
                let (mut ledger, owner, pcm) = forensic_merge_capture("merge-protected", 2);
                if state == "human" {
                    let human =
                        ledger.next_word_observation(ObservationProducer::ManualHuman, 704, &owner);
                    assert!(
                        ledger
                            .admit_word_slots(
                                &human,
                                &[WordPin::new(6_400, 9_600, "ręcznie")
                                    .with_decode_window(0, pcm.len() as u64)]
                            )
                            .grants_mutation()
                    );
                } else if state == "sealed" {
                    ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
                    assert!(ledger.note_frontier_return(&owner, ObservationProducer::Apple));
                    ledger.seal(&owner).unwrap();
                }
                let sources = ledger.slots_of(&owner).unwrap().to_vec();
                let next = ledger.next_word_observation(
                    if state == "formatter" {
                        ObservationProducer::Formatter
                    } else {
                        producer
                    },
                    705,
                    &owner,
                );
                let decision = ledger.admit_word_slots(
                    &next,
                    &[WordPin::new(3_200, 9_600, "naprawdę")
                        .with_decode_window(0, pcm.len() as u64)],
                );
                assert!(!decision.grants_mutation());
                assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_merge_unbounded_aggregate_cannot_retire_complete_word_sources() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, _) = forensic_merge_capture("merge-missing-frame", 2);
            let sources = ledger.slots_of(&owner).unwrap().to_vec();
            ledger.schedule_frontier(owner.clone(), [producer]);
            assert!(ledger.require_text_recovery(&owner));
            let next = ledger.next_word_observation(producer, 706, &owner);
            let decision =
                ledger.admit_word_slots(&next, &[WordPin::new(3_200, 9_600, "naprawdę")]);
            assert!(!decision.grants_mutation());
            assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
            assert_eq!(ledger.text_of(&owner), Some("na prawdę"));
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .any(|alternative| alternative.observation == next
                        && alternative.candidate == "naprawdę"
                        && alternative.sources == sources)
            );
            assert!(ledger.text_recovery_pending(&owner));
            assert!(ledger.note_frontier_return(&owner, producer));
            assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
            assert!(ledger.word_deletions().is_empty());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    // Root-owned Split controls: measured PCM and explicit source receipts.
    fn forensic_split_empty_capture(
        session: &str,
    ) -> (AcousticLedger, OccurrenceIdentity, Vec<f32>) {
        let (measured, owner, pcm) = forensic_merge_capture(session, 1);
        let serial = measured.serial_of(&owner).unwrap();
        let calibration =
            EnergyCalibration::new(serial.evidence_calibration_version.clone(), 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: serial.duration_ms,
                        energy_integral: serial.energy_integral,
                        mean_rms_dbfs: serial.mean_rms_dbfs,
                        peak_dbfs: serial.peak_dbfs,
                        vad_open_sample: serial.vad_open_sample,
                        vad_close_sample: serial.vad_close_sample,
                        evidence_calibration_version: serial.evidence_calibration_version.clone(),
                    },
                    &calibration,
                )
                .is_qualified()
        );
        ledger.record_speech_evidence(measured.speech_evidence.as_ref().unwrap());
        assert!(ledger.slots_of(&owner).is_none());
        (ledger, owner, pcm)
    }

    #[test]
    fn forensic_split_complete_word_needs_source_accounting_not_lexical_equality() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for proof in ["absent", "partial", "different", "complete"] {
                let (mut ledger, owner, pcm) = forensic_merge_capture("split-source", 1);
                let source = ledger.slots_of(&owner).unwrap()[0].clone();
                assert!(ledger.complete_word_slot(&source));
                ledger.schedule_frontier(owner.clone(), [producer]);
                assert!(ledger.require_text_recovery(&owner));
                let mut next = ledger.next_word_observation(producer, 801, &owner);
                let mut pins = [
                    WordPin::new(3_200, 6_400, "I"),
                    WordPin::new(7_000, 9_600, "wo"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, pin)| match proof {
                    "absent" => pin,
                    "partial" => pin.with_decode_window(0, 7_000),
                    "different" => pin.with_decode_window(index as u64 * 320, pcm.len() as u64),
                    "complete" => pin.with_decode_window(0, pcm.len() as u64),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
                // A partial decode cannot supply a word beyond its end.
                if proof == "partial" {
                    pins.truncate(1);
                }
                let candidate = pins
                    .iter()
                    .map(|pin| pin.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let before = ledger.slot_operations().len();
                let receipt = if proof == "complete" {
                    corroborate_candidate(&mut ledger, &mut next, &pins, (320, pcm.len() as u64))
                } else {
                    ledger.admit_word_slots(&next, &pins)
                };
                if proof == "complete" {
                    assert!(
                        receipt.grants_mutation(),
                        "{producer:?}/{proof}: {receipt:?}"
                    );
                    assert_eq!(ledger.text_of(&owner), Some("I wo"));
                    let operations = &ledger.slot_operations()[before..];
                    let split = operations
                        .iter()
                        .filter(|operation| operation.kind == SlotOperationKind::Split)
                        .collect::<Vec<_>>();
                    assert_eq!(split.len(), 1);
                    assert_eq!(split[0].observation, next);
                    assert_eq!(split[0].sources.as_slice(), std::slice::from_ref(&source));
                    assert!(split[0].source_ranges.contains(&owner));
                    assert_eq!(split[0].outputs, ledger.slots_of(&owner).unwrap());
                    assert_eq!(split[0].outputs.len(), 2);
                    for (output, range) in split[0]
                        .outputs
                        .iter()
                        .zip([(3_200, 6_400), (7_000, 9_600)])
                    {
                        assert_eq!((output.sample_start, output.sample_end), range);
                        assert_eq!(output.observation, next);
                        assert!(ledger.slot_descends_from(output, &source));
                    }
                    assert!(!ledger.text_recovery_pending(&owner));
                    assert!(ledger.note_frontier_return(&owner, producer));
                    ledger.seal(&owner).unwrap();
                } else {
                    assert!(
                        !receipt.grants_mutation(),
                        "{producer:?}/{proof}: {receipt:?}"
                    );
                    assert_eq!(
                        ledger.slots_of(&owner).unwrap(),
                        std::slice::from_ref(&source)
                    );
                    assert_eq!(ledger.slot_operations().len(), before);
                    assert!(ledger.slot_alternatives().iter().any(|alternative| {
                        alternative.observation == next
                            && alternative.candidate == candidate
                            && alternative.sources == [source.clone()]
                    }));
                    assert!(ledger.text_recovery_pending(&owner));
                    assert!(ledger.note_frontier_return(&owner, producer));
                    assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                }
                let slots = ledger.slots_of(&owner).unwrap().to_vec();
                let operations = ledger.slot_operations().len();
                assert!(!ledger.admit_word_slots(&next, &pins).grants_mutation());
                assert_eq!(ledger.slots_of(&owner).unwrap(), slots);
                assert_eq!(ledger.slot_operations().len(), operations);
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_split_repeated_complete_sources_reject_unbounded_compression() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for candidate in ["mamy", "mamy dzisiaj"] {
                let (mut ledger, owner, pcm) = forensic_split_empty_capture("split-no-compression");
                let apple =
                    ObservationIdentity::new(ObservationProducer::Apple, 802, 0, owner.clone());
                let words = [
                    WordPin::new(3_200, 4_800, "mamy"),
                    WordPin::new(4_800, 6_400, "dyżur"),
                    WordPin::new(6_400, 9_600, "mamy"),
                ]
                .map(|pin| pin.with_decode_window(0, pcm.len() as u64));
                assert!(ledger.admit_word_slots(&apple, &words).grants_mutation());
                let sources = ledger.slots_of(&owner).unwrap().to_vec();
                assert_eq!(sources.len(), 3);
                assert!(
                    sources
                        .iter()
                        .all(|source| ledger.complete_word_slot(source))
                );
                ledger.schedule_frontier(owner.clone(), [producer]);
                assert!(ledger.require_text_recovery(&owner));
                let next = ledger.next_word_observation(producer, 803, &owner);
                let operations = ledger.slot_operations().len();
                let result =
                    ledger.admit_word_slots(&next, &[WordPin::new(3_200, 9_600, candidate)]);
                assert!(!result.grants_mutation());
                assert_eq!(ledger.text_of(&owner), Some("mamy dyżur mamy"));
                assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
                assert_eq!(ledger.slot_operations().len(), operations);
                let alternative = ledger
                    .slot_alternatives()
                    .iter()
                    .find(|alternative| {
                        alternative.observation == next && alternative.candidate == candidate
                    })
                    .unwrap();
                assert_eq!(alternative.sources, sources);
                assert_eq!(alternative.reason, "incomplete_source_scope");
                assert!(ledger.text_recovery_pending(&owner));
                assert!(ledger.note_frontier_return(&owner, producer));
                assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_split_declared_decoder_frame_must_contain_every_word() {
        let mut failures = Vec::new();
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for (case, frame, valid) in [
                ("actual-frame", (0, 16_000), true),
                ("past-end", (0, 7_000), false),
                ("before-start", (6_000, 16_000), false),
                ("reversed", (12_000, 1_000), false),
                ("empty", (5_000, 5_000), false),
                ("disjoint", (10_000, 15_000), false),
            ] {
                let (mut ledger, owner, pcm) = forensic_merge_capture("split-frame-integrity", 1);
                let sources = ledger.slots_of(&owner).unwrap().to_vec();
                assert_eq!(sources.len(), 1);
                assert!(ledger.complete_word_slot(&sources[0]));
                ledger.schedule_frontier(owner.clone(), [producer]);
                assert!(ledger.require_text_recovery(&owner));
                let mut next = ledger.next_word_observation(producer, 804, &owner);
                let pin = WordPin::new(4_000, 9_000, "mamy").with_decode_window(frame.0, frame.1);
                let before = ledger.slot_operations().len();
                let result = if valid {
                    corroborate_candidate(&mut ledger, &mut next, &[pin], (320, pcm.len() as u64))
                } else {
                    ledger.admit_word_slots(&next, &[pin])
                };
                if valid {
                    assert!(result.grants_mutation(), "{producer:?}/{case}: {result:?}");
                    assert_eq!(ledger.text_of(&owner), Some("mamy"));
                    let output = &ledger.slots_of(&owner).unwrap()[0];
                    assert_eq!((output.sample_start, output.sample_end), (4_000, 9_000));
                    assert!(ledger.slot_descends_from(output, &sources[0]));
                    let operation = ledger.slot_operations().last().unwrap();
                    assert_eq!(operation.kind, SlotOperationKind::Correct);
                    assert_eq!(operation.sources, sources);
                    assert_eq!(operation.outputs.as_slice(), std::slice::from_ref(output));
                    assert!(!ledger.text_recovery_pending(&owner));
                } else if result.grants_mutation()
                    || ledger.slots_of(&owner).unwrap() != sources
                    || ledger.slot_operations().len() != before
                    || !ledger.text_recovery_pending(&owner)
                {
                    failures.push(format!(
                        "{producer:?}/{case} frame={frame:?}: {result:?}; text={:?}; pending={}",
                        ledger.text_of(&owner),
                        ledger.text_recovery_pending(&owner)
                    ));
                }
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn forensic_split_malformed_word_cannot_block_valid_partial_speech() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, _) = forensic_split_empty_capture("split-mixed-frame");
            let apple = ObservationIdentity::new(ObservationProducer::Apple, 805, 0, owner.clone());
            assert!(ledger.admit(&apple, "czekam na całość").grants_mutation());
            let source = ledger.slots_of(&owner).unwrap()[0].clone();
            ledger.schedule_frontier(owner.clone(), [producer]);
            assert!(ledger.require_text_recovery(&owner));
            let next = ledger.next_word_observation(producer, 806, &owner);
            let pins = [
                WordPin::new(4_000, 6_000, "mamy").with_decode_window(0, 7_000),
                WordPin::new(8_000, 9_000, "zakłócenie").with_decode_window(0, 7_000),
            ];
            let receipt = ledger.admit_word_slots(&next, &pins);
            assert!(receipt.grants_mutation(), "{producer:?}: {receipt:?}");
            assert_eq!(ledger.text_of(&owner), Some("mamy"));
            let output = &ledger.slots_of(&owner).unwrap()[0];
            assert_eq!((output.sample_start, output.sample_end), (4_000, 6_000));
            assert_eq!(output.observation, next);
            assert!(ledger.slot_descends_from(output, &source));
            // Child refusals belong to their exact PCM occurrence. The
            // owner-linked alternative preserves their parent/source relation.
            let rejected_pin =
                OccurrenceIdentity::new(&owner.session, owner.capture_epoch, 8_000, 9_000);
            assert!(ledger.layer_trail_for(&rejected_pin).any(|entry| {
                entry.observation.producer == producer
                    && entry.candidate_label == "zakłócenie"
                    && !entry.decision.grants_mutation()
            }));
            assert!(ledger.slot_alternatives().iter().any(|alternative| {
                alternative.observation == next
                    && alternative.candidate == "zakłócenie"
                    && alternative.sources == [source.clone()]
            }));
            assert!(ledger.text_recovery_pending(&owner));
            assert!(ledger.note_frontier_return(&owner, producer));
            assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
            assert!(ledger.word_deletions().is_empty());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    // Root-only independent Relay controls for the div0 admission boundary.
    #[test]
    fn forensic_div0_complete_decode_can_replace_a_coarse_apple_hypothesis() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for pinned in [false, true] {
                let (mut ledger, owner, pcm) = forensic_split_empty_capture("coarse-pencil");
                let first =
                    ObservationIdentity::new(ObservationProducer::Apple, 930, 0, owner.clone());
                let receipt = if pinned {
                    ledger.admit_word_slots(
                        &first,
                        &[WordPin::new(
                            owner.sample_start,
                            owner.sample_end,
                            "czy plan weryfikowałeś",
                        )],
                    )
                } else {
                    ledger.admit(&first, "czy plan weryfikowałeś")
                };
                assert!(receipt.grants_mutation());
                let source = ledger.slots_of(&owner).unwrap()[0].clone();
                assert!(!ledger.complete_word_slot(&source));
                ledger.schedule_frontier(owner.clone(), [producer]);
                assert!(ledger.require_text_recovery(&owner));
                let next = ledger.next_word_observation(producer, 931, &owner);
                let receipt = ledger.admit_word_slots(
                    &next,
                    &[WordPin::new(owner.sample_start, owner.sample_end, "tak")
                        .with_decode_window(0, pcm.len() as u64)],
                );
                assert!(
                    receipt.grants_mutation(),
                    "{producer:?}/pinned={pinned}: {receipt:?}"
                );
                assert_eq!(
                    ledger.text_of(&owner),
                    Some("tak"),
                    "a coarse Apple pencil is not a lexical floor"
                );
                let output = &ledger.slots_of(&owner).unwrap()[0];
                assert_eq!(output.producer, producer);
                assert_eq!(
                    (output.sample_start, output.sample_end),
                    (owner.sample_start, owner.sample_end)
                );
                assert!(ledger.slot_descends_from(output, &source));
                assert!(ledger.complete_word_slot(output));
                assert!(!ledger.text_recovery_pending(&owner));
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_div0_unbounded_label_cannot_append_to_a_complete_word_source() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for pending in [false, true] {
                let (mut ledger, owner, _) = forensic_merge_capture("word-authority", 1);
                let before = ledger.slots_of(&owner).unwrap().to_vec();
                assert!(ledger.complete_word_slot(&before[0]));
                assert_eq!(ledger.text_of(&owner), Some("Iwo"));
                if pending {
                    assert!(ledger.require_text_recovery(&owner));
                }
                let candidate = ledger.next_word_observation(producer, 941, &owner);
                let operations = ledger.slot_operations().len();
                let receipt = ledger.admit(&candidate, "Iwo plan");
                assert!(
                    !receipt.grants_mutation(),
                    "a label without Word/decode targets has no authority over completed pins: {producer:?}/pending={pending}: {receipt:?}"
                );
                assert_eq!(ledger.text_of(&owner), Some("Iwo"));
                assert_eq!(ledger.slots_of(&owner).unwrap(), before);
                assert_eq!(ledger.slot_operations().len(), operations);
                assert_eq!(ledger.text_recovery_pending(&owner), pending);
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    fn owner() -> OccurrenceIdentity {
        OccurrenceIdentity::new("slot-test", 1, 0, 16_000)
    }

    fn observation(producer: ObservationProducer, generation: u64) -> ObservationIdentity {
        ObservationIdentity::new(producer, 1, generation, owner())
    }

    fn pinned(words: &[WordPin]) -> AcousticLedger {
        let mut ledger = AcousticLedger::new();
        ledger.admit_word_slots(&observation(ObservationProducer::Apple, 0), words);
        ledger
    }

    fn measured_pinned(words: &[WordPin]) -> AcousticLedger {
        let mut pcm = vec![0.0_f32; 24_000];
        for word in words {
            pcm[word.sample_start as usize..word.sample_end as usize].fill(0.2);
        }
        let mut ledger = super::super::word_adjudication_tests::measured_ledger(&owner(), &pcm);
        assert!(
            ledger
                .admit_word_slots(&observation(ObservationProducer::Apple, 0), words)
                .grants_mutation()
        );
        ledger
    }

    #[test]
    fn audit_f02_group_omission_cannot_hide_behind_extra_tokens() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for has_pin in [false, true] {
                for via_label in [false, true] {
                    for candidate in [
                        "czy weryfikowałeś dokładnie",
                        "czy weryfikowałeś yyy [śmiech]",
                    ] {
                        let mut ledger = if has_pin {
                            pinned(&[WordPin::new(0, 16_000, "czy plan weryfikowałeś")])
                        } else {
                            let mut ledger = AcousticLedger::new();
                            ledger.admit(
                                &observation(ObservationProducer::Apple, 0),
                                "czy plan weryfikowałeś",
                            );
                            ledger
                        };
                        let sources = ledger.slots_of(&owner()).unwrap().to_vec();
                        let operations_before = ledger.slot_operations().len();
                        let next = observation(producer, 1);
                        let receipt = if via_label {
                            ledger.admit_pinned_label(&next, candidate, &[])
                        } else {
                            ledger.admit_word_slots(&next, &[WordPin::new(0, 16_000, candidate)])
                        };
                        assert_eq!(
                            ledger.text_of(&owner()),
                            Some("czy plan weryfikowałeś"),
                            "{producer:?}, pinned={has_pin}, label={via_label}"
                        );
                        assert!(!receipt.grants_mutation());
                        assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
                        assert!(ledger.word_deletions().is_empty());
                        let alternative = ledger.slot_alternatives().last().unwrap();
                        assert_eq!(alternative.candidate, candidate);
                        assert_eq!(alternative.sources, sources);
                        assert_eq!(alternative.observation, next);
                        for diagnostic in &ledger.slot_operations()[operations_before..] {
                            assert_eq!(diagnostic.sources, sources);
                            assert!(diagnostic.outputs.is_empty() || diagnostic.outputs == sources);
                            assert_eq!(diagnostic.source_ranges, vec![owner()]);
                        }
                        for source in ledger.slots_of(&owner()).unwrap() {
                            assert_eq!(ledger.slot_source_ranges(source), vec![owner()]);
                        }
                        assert_eq!(ledger.conservation().residue(), 0);
                        let slots = ledger.slots_of(&owner()).unwrap().to_vec();
                        let operations = ledger.slot_operations().len();
                        assert!(matches!(
                            ledger.admit_word_slots(&next, &[WordPin::new(0, 16_000, candidate)]),
                            MutationReceipt::Refuse {
                                reason: RefuseReason::BatchDuplicate,
                                ..
                            }
                        ));
                        assert_eq!(ledger.slots_of(&owner()).unwrap(), slots);
                        assert_eq!(ledger.slot_operations().len(), operations);
                        assert_eq!(ledger.conservation().residue(), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn group_alignment_ambiguity_retains_text_and_candidate() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for has_pin in [false, true] {
                let mut ledger = if has_pin {
                    pinned(&[WordPin::new(0, 16_000, "czy plan jutro weryfikowałeś")])
                } else {
                    let mut ledger = AcousticLedger::new();
                    ledger.admit(
                        &observation(ObservationProducer::Apple, 0),
                        "czy plan jutro weryfikowałeś",
                    );
                    ledger
                };
                let sources = ledger.slots_of(&owner()).unwrap().to_vec();
                let next = observation(producer, 1);
                ledger.admit_word_slots(
                    &next,
                    &[WordPin::new(0, 16_000, "czy kot teraz weryfikowałeś")],
                );
                assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
                let alternative = ledger.slot_alternatives().last().unwrap();
                assert_eq!(alternative.candidate, "czy kot teraz weryfikowałeś");
                assert_eq!(alternative.reason, "group_alignment_ambiguous");
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn group_alignment_keeps_forms_insertions_events_and_repetitions() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for (held, candidate, expected, retained) in [
                (
                    "czy plan weryfikowałeś",
                    "Czy PLAN, zweryfikowałeś.",
                    "Czy PLAN, zweryfikowałeś.",
                    false,
                ),
                (
                    "czy plan weryfikowałeś",
                    "czy klan weryfikowałeś",
                    "czy klan weryfikowałeś",
                    false,
                ),
                (
                    "plan weryfikowałeś",
                    "zweryfikowałeś yyy [śmiech]",
                    "plan weryfikowałeś",
                    true,
                ),
                (
                    "czy yyy [śmiech głośny] plan",
                    "Czy plan.",
                    "czy yyy [śmiech głośny] plan",
                    true,
                ),
                (
                    "Iwo Iwo Iwo Iwo Iwo",
                    "Iwo Iwo",
                    "Iwo Iwo Iwo Iwo Iwo",
                    true,
                ),
            ] {
                for has_pin in [false, true] {
                    for via_label in [false, true] {
                        let mut ledger = if has_pin {
                            pinned(&[WordPin::new(0, 16_000, held)])
                        } else {
                            let mut ledger = AcousticLedger::new();
                            ledger.admit(&observation(ObservationProducer::Apple, 0), held);
                            ledger
                        };
                        let next = observation(producer, 1);
                        let source_slots = ledger.slots_of(&owner()).unwrap().to_vec();
                        assert_eq!(
                            preserve_group_content(held, candidate),
                            Ok((expected.to_owned(), retained))
                        );
                        if via_label {
                            ledger.admit_pinned_label(&next, candidate, &[]);
                        } else {
                            ledger.admit_word_slots(&next, &[WordPin::new(0, 16_000, candidate)]);
                        }
                        // A label without word targets cannot revise pinned sources.
                        // Lexical alignment does not supply mutation authority.
                        if has_pin && via_label {
                            assert_eq!(ledger.text_of(&owner()), Some(held));
                            assert_eq!(ledger.slots_of(&owner()).unwrap(), source_slots);
                            assert!(
                                ledger
                                    .slot_alternatives()
                                    .iter()
                                    .any(|alternative| alternative.candidate == candidate
                                        && alternative.reason == "whole_label_has_no_word_targets")
                            );
                            assert!(ledger.word_deletions().is_empty());
                            assert_eq!(ledger.conservation().residue(), 0);
                            continue;
                        }
                        assert_eq!(
                            ledger.text_of(&owner()),
                            Some(expected),
                            "{held} → {candidate}"
                        );
                        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 1);
                        assert_eq!(
                            ledger
                                .slot_alternatives()
                                .iter()
                                .any(|alternative| alternative.candidate == candidate
                                    && alternative.reason == "held_token_retained"),
                            retained
                        );
                        assert_eq!(ledger.conservation().residue(), 0);
                    }
                }
            }
        }
    }

    fn group_speech(ranges: &[(u64, u64)]) -> AcousticSpeechEvidence {
        use crate::audio::capture_receipt::{CAPTURE_ENERGY_PRODUCER, CaptureEvidenceIdentity};
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(owner().session, owner().capture_epoch),
            CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 16_000,
            },
            ranges
                .iter()
                .map(|&(start, end)| TailSampleRange {
                    session: owner().session,
                    capture_epoch: owner().capture_epoch,
                    sample_start: start,
                    sample_end: end,
                })
                .collect(),
        )
    }

    #[test]
    fn measured_group_pins_replace_unpaired_tokens_only_with_complete_speech_coverage() {
        let pins = [
            WordPin::new(0, 4_000, "czy"),
            WordPin::new(6_000, 10_000, "weryfikowałeś"),
            WordPin::new(12_000, 16_000, "dokładnie"),
        ];
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for outer_pin in [false, true] {
                let mut ledger = if outer_pin {
                    pinned(&[WordPin::new(0, 16_000, "czy plan weryfikowałeś")])
                } else {
                    let mut ledger = AcousticLedger::new();
                    ledger.admit(
                        &observation(ObservationProducer::Apple, 0),
                        "czy plan weryfikowałeś",
                    );
                    ledger
                };
                let source = ledger.slots_of(&owner()).unwrap().to_vec();
                let speech = group_speech(&[(0, 4_000), (6_000, 10_000), (12_000, 16_000)]);
                ledger.record_speech_evidence(&speech);
                let next = observation(producer, 1);
                assert!(ledger.admit_word_slots(&next, &pins).grants_mutation());
                assert_eq!(
                    ledger.text_of(&owner()),
                    Some("czy weryfikowałeś dokładnie")
                );
                let receipt = ledger
                    .group_speech_coverages()
                    .last()
                    .expect("coverage proof");
                assert_eq!(receipt.operation.observation, next);
                assert_eq!(receipt.operation.sources, source);
                assert_eq!(
                    receipt.operation.outputs,
                    ledger.slots_of(&owner()).unwrap()
                );
                assert_eq!(receipt.speech, speech);
                assert_eq!(receipt.coverage.len(), 3);
                assert!(receipt.coverage.iter().all(|part| part.pin_owner == owner()
                    && part.speech_range.sample_start >= part.pin_range.sample_start
                    && part.speech_range.sample_end <= part.pin_range.sample_end));
                let operations = ledger.slot_operations().len();
                assert!(matches!(
                    ledger.admit_word_slots(&next, &pins),
                    MutationReceipt::Refuse {
                        reason: RefuseReason::BatchDuplicate,
                        ..
                    }
                ));
                assert_eq!(ledger.slot_operations().len(), operations);
                assert_eq!(ledger.group_speech_coverages().len(), 1);
                assert!(ledger.word_deletions().is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn measured_group_pins_keep_group_when_speech_is_uncovered_or_unavailable() {
        // The multiword second pin is deliberately incomplete word-grain input;
        // neither its label nor a producer return can complete the coarse source.
        let pins = [
            WordPin::new(0, 4_000, "czy"),
            WordPin::new(6_000, 16_000, "weryfikowałeś dokładnie"),
        ];
        for has_speech in [false, true] {
            for outer_pin in [false, true] {
                let (mut ledger, measured) = forensic_coarse_fixture(outer_pin);
                let source = ledger.slots_of(&owner()).unwrap().to_vec();
                if has_speech {
                    ledger.record_speech_evidence(&measured);
                }
                ledger.schedule_frontier(owner(), [ObservationProducer::Whisper]);
                assert!(ledger.require_text_recovery(&owner()));
                let next = observation(ObservationProducer::Whisper, 1);
                ledger.admit_word_slots(&next, &pins);
                if has_speech {
                    // Measured individual words may improve a coarse hypothesis;
                    // the still-unaccounted PCM stays debt, not an Apple word floor.
                    assert_eq!(
                        ledger.text_of(&owner()),
                        Some("czy weryfikowałeś dokładnie")
                    );
                    let operation = ledger.slot_operations().last().unwrap();
                    assert_eq!(operation.sources, source);
                    assert!(operation.source_ranges.contains(&owner()));
                    assert_eq!(
                        operation.rule_id,
                        "acoustic_resegmentation/partial-speech/v1"
                    );
                } else {
                    assert_eq!(ledger.slots_of(&owner()).unwrap(), source);
                }
                assert!(!ledger.slot_alternatives().is_empty());
                assert!(ledger.group_speech_coverages().is_empty());
                assert!(ledger.text_recovery_pending(&owner()));
                assert!(ledger.note_frontier_return(&owner(), ObservationProducer::Whisper));
                assert_eq!(ledger.seal(&owner()), Err(SealRefusal::TextRecoveryPending));
                assert_eq!(
                    ledger.seal_terminal("slot-test", 1),
                    Err(SealRefusal::TextRecoveryPending)
                );
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn group_speech_coverage_refuses_foreign_partial_invalid_and_empty_evidence() {
        use crate::audio::capture_receipt::{CAPTURE_ENERGY_PRODUCER, CaptureEvidenceIdentity};
        let valid = group_speech(&[(0, 4_000), (6_000, 16_000)]);
        let mut evidence = vec![
            group_speech(&[]),
            group_speech(&[(0, 4_001), (6_000, 16_000)]),
        ];
        for availability in [
            AcousticAvailability::NotObserved,
            AcousticAvailability::Discontinuous {
                observed_samples: 16_000,
            },
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 16_000,
            },
            AcousticAvailability::Observed {
                observed_samples: 15_999,
            },
        ] {
            evidence.push(AcousticSpeechEvidence::measured(
                valid.identity().clone(),
                CAPTURE_ENERGY_PRODUCER,
                availability,
                valid.ranges().to_vec(),
            ));
        }
        evidence.push(AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(owner().session, 2),
            CAPTURE_ENERGY_PRODUCER,
            valid.availability(),
            valid.ranges().to_vec(),
        ));
        evidence.push(AcousticSpeechEvidence::measured(
            valid.identity().clone(),
            "text_producer",
            valid.availability(),
            valid.ranges().to_vec(),
        ));
        for (start, end, epoch) in [(7_000, 6_000, 1), (6_000, 16_001, 1), (6_000, 16_000, 2)] {
            let mut ranges = valid.ranges().to_vec();
            ranges[1].sample_start = start;
            ranges[1].sample_end = end;
            ranges[1].capture_epoch = epoch;
            evidence.push(AcousticSpeechEvidence::measured(
                valid.identity().clone(),
                CAPTURE_ENERGY_PRODUCER,
                valid.availability(),
                ranges,
            ));
        }
        let pins = [
            WordPin::new(0, 4_000, "czy"),
            WordPin::new(6_000, 16_000, "weryfikowałeś"),
        ];
        for (variant, speech) in evidence.into_iter().enumerate() {
            let (mut ledger, _) = forensic_coarse_fixture(false);
            let source = ledger.slots_of(&owner()).unwrap().to_vec();
            ledger.record_speech_evidence(&valid);
            ledger.record_speech_evidence(&speech);
            ledger.schedule_frontier(owner(), [ObservationProducer::Whisper]);
            assert!(ledger.require_text_recovery(&owner()));
            ledger.admit_word_slots(&observation(ObservationProducer::Whisper, 1), &pins);
            if variant == 1 {
                // A one-sample uncovered speech gap prevents group completion,
                // while individually supported words may replace the weak label.
                assert_eq!(ledger.text_of(&owner()), Some("czy weryfikowałeś"));
                let operation = ledger.slot_operations().last().unwrap();
                assert_eq!(operation.sources, source);
                assert!(operation.source_ranges.contains(&owner()));
                assert_eq!(
                    operation.rule_id,
                    "acoustic_resegmentation/partial-speech/v1"
                );
            } else {
                assert_eq!(ledger.slots_of(&owner()).unwrap(), source, "{speech:?}");
            }
            assert!(ledger.group_speech_coverages().is_empty());
            assert!(!ledger.slot_alternatives().is_empty());
            assert!(ledger.text_recovery_pending(&owner()));
            assert!(ledger.note_frontier_return(&owner(), ObservationProducer::Whisper));
            assert_eq!(ledger.seal(&owner()), Err(SealRefusal::TextRecoveryPending));
            assert_eq!(
                ledger.seal_terminal("slot-test", 1),
                Err(SealRefusal::TextRecoveryPending)
            );
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn adjacent_assigned_pin_covers_speech_without_lending_its_word() {
        let neighbour = OccurrenceIdentity::new(owner().session, 1, 16_000, 32_000);
        let next = observation(ObservationProducer::Whisper, 1);
        let neighbouring_pin = OccurrenceIdentity::new(owner().session, 1, 14_000, 20_000);
        let pins = [
            WordPin::new(2_000, 5_000, "czy"),
            WordPin::new(5_000, 8_000, "weryfikowałeś"),
        ];
        for neighbour_kind in ["qualified", "label", "word"] {
            for variant in 0..4 {
                let matching_request = variant == 0;
                let (mut ledger, measured_owner, measured_neighbour, _pcm) =
                    forensic_neighbour_capture("slot-test", 1, false);
                assert_eq!(measured_owner, owner());
                assert_eq!(measured_neighbour, neighbour);
                if neighbour_kind == "label" {
                    ledger.admit(
                        &ObservationIdentity::new(
                            ObservationProducer::Apple,
                            0,
                            0,
                            neighbour.clone(),
                        ),
                        "sąsiad",
                    );
                } else if neighbour_kind == "word" {
                    ledger.admit_word_slots(
                        &ObservationIdentity::new(
                            ObservationProducer::Apple,
                            0,
                            0,
                            neighbour.clone(),
                        ),
                        &[WordPin::new(14_000, 20_000, "sąsiad").with_decode_window(0, 32_000)],
                    );
                }
                assert!(ledger.is_qualified(&neighbour));
                let mut assignment_observation = next.clone();
                match variant {
                    1 => assignment_observation.request += 1,
                    2 => assignment_observation.generation += 1,
                    3 => assignment_observation.producer = ObservationProducer::CloudLive,
                    _ => (),
                }
                ledger.record_assigned_word_pins(
                    &assignment_observation,
                    &[(neighbour.clone(), neighbouring_pin.clone())],
                );
                ledger.schedule_frontier(owner(), [ObservationProducer::Whisper]);
                ledger.admit_word_slots(&next, &pins);
                // Accepted words may refine the coarse Apple hypothesis.
                // Only an actual matching neighbouring word settles the rest.
                assert_eq!(ledger.text_of(&owner()), Some("czy weryfikowałeś"));
                let covered = matching_request && neighbour_kind == "word";
                if covered {
                    let coverage = ledger.group_speech_coverages().last().unwrap();
                    assert_eq!(coverage.coverage.last().unwrap().pin_owner, neighbour);
                    assert_eq!(
                        coverage.coverage.last().unwrap().pin_range,
                        neighbouring_pin
                    );
                    assert_eq!(coverage.operation.outputs.len(), 2);
                } else {
                    assert!(ledger.group_speech_coverages().is_empty());
                }
                assert_eq!(
                    ledger.text_of(&neighbour),
                    (neighbour_kind != "qualified").then_some("sąsiad")
                );
                ledger.note_frontier_return(&owner(), ObservationProducer::Whisper);
                assert_eq!(ledger.text_recovery_pending(&owner()), !covered);
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn adjacent_assigned_pin_cannot_cover_speech_if_its_owner_pin_is_deleted_as_no_speech() {
        use super::word_verdict::adjudicate_word_pcm;
        use crate::audio::capture_receipt::CaptureEvidenceIdentity;
        use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

        let neighbour = OccurrenceIdentity::new(owner().session, 1, 16_000, 32_000);
        let quiet_pin = OccurrenceIdentity::new(owner().session, 1, 16_000, 20_000);
        let (mut ledger, measured_owner, measured_neighbour, pcm) =
            forensic_neighbour_capture("slot-test", 1, true);
        assert_eq!(measured_owner, owner());
        assert_eq!(measured_neighbour, neighbour);
        ledger.admit_word_slots(
            &ObservationIdentity::new(ObservationProducer::Apple, 0, 0, neighbour.clone()),
            &[WordPin::new(16_000, 20_000, "sąsiad")],
        );
        let target = SlotTarget::from(&ledger.slots_of(&neighbour).unwrap()[0]);
        let silero = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(owner().session, 1),
            SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 32_000,
            },
            [(2_000, 8_000), (14_000, 16_000)]
                .into_iter()
                .map(|(sample_start, sample_end)| TailSampleRange {
                    session: owner().session,
                    capture_epoch: owner().capture_epoch,
                    sample_start,
                    sample_end,
                })
                .collect(),
        );
        let verdict = adjudicate_word_pcm(&quiet_pin, &quiet_pin, &pcm[16_000..20_000], &silero);
        ledger
            .remove_word_with_verdict(
                &ObservationIdentity::new(ObservationProducer::Whisper, 0, 0, neighbour.clone()),
                &target,
                &verdict,
            )
            .unwrap();
        let next = observation(ObservationProducer::Whisper, 1);
        ledger.record_assigned_word_pins(
            &next,
            &[(
                neighbour.clone(),
                OccurrenceIdentity::new(owner().session, 1, 14_000, 20_000),
            )],
        );
        ledger.schedule_frontier(owner(), [ObservationProducer::Whisper]);
        ledger.admit_word_slots(
            &next,
            &[
                WordPin::new(2_000, 5_000, "czy"),
                WordPin::new(5_000, 8_000, "weryfikowałeś"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("czy weryfikowałeś"));
        ledger.note_frontier_return(&owner(), ObservationProducer::Whisper);
        assert!(ledger.text_recovery_pending(&owner()));
        assert_eq!(ledger.seal(&owner()), Err(SealRefusal::TextRecoveryPending));
        assert!(ledger.group_speech_coverages().is_empty());
        assert!(!ledger.slot_alternatives().is_empty());
        assert_eq!(ledger.word_deletions().len(), 1);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn complete_speech_coverage_does_not_authorize_whole_group_pins_or_protected_sources() {
        for producer in [ObservationProducer::Apple, ObservationProducer::ManualHuman] {
            for sealed in [false, true] {
                let mut ledger = AcousticLedger::new();
                let initial = observation(producer, 0);
                ledger.admit(&initial, "czy plan weryfikowałeś");
                let calibration = EnergyCalibration {
                    version: "a2-protected-group".into(),
                    min_energy_integral: 1.0,
                    min_valley_samples: 1,
                };
                ledger.qualify(
                    &AcousticEvidence {
                        occurrence: owner(),
                        duration_ms: 1_000.0,
                        energy_integral: 10.0,
                        mean_rms_dbfs: -12.0,
                        peak_dbfs: -3.0,
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(16_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration,
                );
                if sealed {
                    ledger.schedule_frontier(owner(), vec![producer]);
                    ledger.note_frontier_return(&owner(), producer);
                    ledger.seal(&owner()).unwrap();
                }
                let source = ledger.slots_of(&owner()).unwrap().to_vec();
                ledger.record_speech_evidence(&group_speech(&[(0, 16_000)]));
                let pins = if producer == ObservationProducer::Apple && !sealed {
                    vec![WordPin::new(0, 16_000, "czy weryfikowałeś dokładnie")]
                } else {
                    vec![
                        WordPin::new(0, 8_000, "czy"),
                        WordPin::new(8_000, 16_000, "weryfikowałeś"),
                    ]
                };
                ledger.admit_word_slots(&observation(ObservationProducer::Whisper, 1), &pins);
                assert_eq!(ledger.slots_of(&owner()).unwrap(), source);
                assert!(ledger.group_speech_coverages().is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn grouped_omission_cannot_be_hidden_by_new_child_boundaries() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(
            &observation(ObservationProducer::Apple, 0),
            "czy plan weryfikowałeś",
        );
        let sources = ledger.slots_of(&owner()).unwrap().to_vec();
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[
                WordPin::new(0, 4_000, "czy"),
                WordPin::new(6_000, 10_000, "weryfikowałeś"),
                WordPin::new(12_000, 16_000, "dokładnie"),
            ],
        );
        assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
        assert!(!ledger.slot_alternatives().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn automatic_group_slot_revisions_share_content_preservation() {
        for producer in [ObservationProducer::Apple, ObservationProducer::Lexicon] {
            for via_label in [false, true] {
                let mut ledger = AcousticLedger::new();
                ledger.admit(
                    &observation(ObservationProducer::Apple, 0),
                    "czy plan weryfikowałeś",
                );
                if via_label {
                    ledger.admit_pinned_label(
                        &observation(producer, 1),
                        "czy weryfikowałeś dokładnie",
                        &[],
                    );
                } else {
                    ledger.admit_word_slots(
                        &observation(producer, 1),
                        &[WordPin::new(0, 16_000, "czy weryfikowałeś dokładnie")],
                    );
                }
                assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
                assert!(
                    ledger
                        .slot_operations()
                        .last()
                        .unwrap()
                        .rule_id
                        .contains("held_token_retained")
                );
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn group_alignment_does_not_choose_between_repeated_anchors() {
        let mut ledger = pinned(&[WordPin::new(0, 16_000, "Iwo plan Iwo")]);
        let sources = ledger.slots_of(&owner()).unwrap().to_vec();
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[WordPin::new(0, 16_000, "Iwo klan")],
        );
        assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
        assert_eq!(
            ledger.slot_alternatives().last().unwrap().reason,
            "group_alignment_ambiguous"
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn crossing_spelling_matches_remain_alternative() {
        let mut ledger = pinned(&[WordPin::new(0, 16_000, "czy plan weryfikowałeś")]);
        let sources = ledger.slots_of(&owner()).unwrap().to_vec();
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[WordPin::new(0, 16_000, "czy zweryfikowałeś klan")],
        );
        assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
        assert_eq!(
            ledger.slot_alternatives().last().unwrap().reason,
            "group_alignment_ambiguous"
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn whole_group_formatter_pin_has_no_slot_authority() {
        for via_label in [false, true] {
            let mut ledger = AcousticLedger::new();
            ledger.admit(
                &observation(ObservationProducer::Apple, 0),
                "czy plan weryfikowałeś",
            );
            let sources = ledger.slots_of(&owner()).unwrap().to_vec();
            let operations = ledger.slot_operations().len();
            let formatter = observation(ObservationProducer::Formatter, 1);
            let candidate = "czy weryfikowałeś dokładnie";
            let receipt = if via_label {
                ledger.admit_pinned_label(&formatter, candidate, &[])
            } else {
                ledger.admit_word_slots(&formatter, &[WordPin::new(0, 16_000, candidate)])
            };
            // Both entry paths deny Raw authority. A word batch records its
            // protected-source refusal before the shared label decision.
            let expected_reason = if via_label {
                super::super::RefuseReason::AuthorityConflict
            } else {
                super::super::RefuseReason::SlotAdmissionRejected
            };
            assert!(matches!(
                receipt,
                MutationReceipt::Refuse {
                    reason,
                    ..
                } if reason == expected_reason
            ));
            assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
            assert_eq!(ledger.slot_operations().len(), operations);
            assert!(
                ledger
                    .layer_trail_for(&owner())
                    .any(|entry| entry.producer() == ObservationProducer::Formatter
                        && entry.candidate_label == candidate
                        && matches!(
                            entry.decision,
                            MutationReceipt::Refuse {
                                reason,
                                ..
                            } if reason == expected_reason
                        ))
            );
            if !via_label {
                let alternative = ledger.slot_alternatives().last().unwrap();
                assert_eq!(alternative.candidate, candidate);
                assert_eq!(alternative.sources, sources);
                assert_eq!(alternative.reason, "protected_source");
            }
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn group_label_revision_keeps_accuracy_and_noop_lexicon_keeps_authority() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(
            &observation(ObservationProducer::Apple, 0),
            "czy plan weryfikowałeś",
        );
        ledger.admit_pinned_label(
            &observation(ObservationProducer::Whisper, 1),
            "Czy plan weryfikowałeś",
            &[],
        );
        assert!(ledger.committed_word_pin_ranges(&owner()).is_empty());
        let source = ledger.slots_of(&owner()).unwrap()[0].clone();
        ledger.admit_pinned_label(
            &observation(ObservationProducer::Lexicon, 1),
            "Czy plan weryfikowałeś",
            &[],
        );
        assert_eq!(
            ledger.slots_of(&owner()).unwrap(),
            std::slice::from_ref(&source)
        );
        assert_eq!(source.producer, ObservationProducer::Whisper);
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 2),
            &[
                WordPin::new(0, 4_000, "czy"),
                WordPin::new(6_000, 10_000, "plan"),
                WordPin::new(12_000, 16_000, "zweryfikowałeś"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("czy plan zweryfikowałeś"));
        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 3);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn manual_human_can_change_a_group_and_blocks_late_acoustic_labels() {
        let mut ledger = pinned(&[WordPin::new(0, 16_000, "czy plan weryfikowałeś")]);
        ledger.admit_word_slots(
            &observation(ObservationProducer::ManualHuman, 1),
            &[WordPin::new(0, 16_000, "czy weryfikowałeś")],
        );
        let sources = ledger.slots_of(&owner()).unwrap().to_vec();
        assert_eq!(ledger.text_of(&owner()), Some("czy weryfikowałeś"));
        for producer in [
            ObservationProducer::Whisper,
            ObservationProducer::CloudLive,
            ObservationProducer::Apple,
        ] {
            ledger.admit_word_slots(
                &observation(producer, 2),
                &[WordPin::new(0, 16_000, "czy plan weryfikowałeś")],
            );
            assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
        }
        assert_eq!(ledger.conservation().residue(), 0);
    }

    fn bounded_group_words(text: &str) -> Vec<WordPin> {
        let words = text.split_whitespace().collect::<Vec<_>>();
        assert!(!words.is_empty());
        words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                WordPin::new(
                    500 + (15_000 * index / words.len()) as u64,
                    500 + (15_000 * (index + 1) / words.len()) as u64,
                    *word,
                )
                .with_decode_window(0, 16_000)
            })
            .collect()
    }

    #[test]
    fn acoustic_whole_group_corrections_keep_exact_source() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for (apple, acoustic) in [
                ("weryfikowałeś", "zweryfikowałeś"),
                ("weryfikowałeś", "zweryfikowałeś yyy [śmiech]"),
                ("czy weryfikowałeś", "czy plan weryfikowałeś"),
            ] {
                // Exercise both occurrence labels and already pinned groups.
                for has_pin in [false, true] {
                    let (mut ledger, speech) = forensic_group_fixture(has_pin, apple);
                    ledger.record_speech_evidence(&speech);
                    ledger.require_text_recovery(&owner());
                    let source = ledger.slots_of(&owner()).unwrap()[0].clone();
                    if has_pin {
                        let operations = ledger.slot_operations().len();
                        let label_only =
                            ledger.admit_pinned_label(&observation(producer, 0), acoustic, &[]);
                        assert!(!label_only.grants_mutation());
                        assert_eq!(ledger.text_of(&owner()), Some(apple));
                        assert_eq!(
                            ledger.slots_of(&owner()).unwrap(),
                            std::slice::from_ref(&source)
                        );
                        assert_eq!(ledger.slot_operations().len(), operations);
                        assert!(ledger.text_recovery_pending(&owner()));
                    }
                    let pins = bounded_group_words(acoustic);
                    let mut next = observation(producer, 1);
                    let receipt = corroborate_candidate(&mut ledger, &mut next, &pins, (0, 20_000));
                    assert!(receipt.is_correct(), "{producer:?}: {apple} → {acoustic}");
                    assert_eq!(ledger.text_of(&owner()), Some(acoustic));
                    assert_eq!(ledger.slots_of(&owner()).unwrap().len(), pins.len());
                    let operation = ledger.slot_operations().last().unwrap();
                    let expected_kind = if has_pin && pins.len() == 1 {
                        SlotOperationKind::Correct
                    } else {
                        SlotOperationKind::Split
                    };
                    assert_eq!(operation.kind, expected_kind);
                    assert_eq!(operation.sources, vec![source.clone()]);
                    let mut expected_ranges = vec![owner()];
                    expected_ranges.extend(pins.iter().map(|pin| {
                        OccurrenceIdentity::new(
                            owner().session,
                            owner().capture_epoch,
                            pin.sample_start,
                            pin.sample_end,
                        )
                    }));
                    assert_eq!(operation.source_ranges, expected_ranges);
                    assert_eq!(operation.outputs, ledger.slots_of(&owner()).unwrap());
                    for output in &operation.outputs {
                        let expected_playback = if expected_kind == SlotOperationKind::Correct {
                            expected_ranges.clone()
                        } else {
                            vec![OccurrenceIdentity::new(
                                owner().session,
                                owner().capture_epoch,
                                output.sample_start,
                                output.sample_end,
                            )]
                        };
                        assert_eq!(ledger.slot_source_ranges(output), expected_playback);
                    }
                    assert!(!ledger.text_recovery_pending(&owner()));
                    let operations = ledger.slot_operations().len();
                    assert!(matches!(
                        ledger.admit_word_slots(&next, &pins),
                        MutationReceipt::Refuse {
                            reason: RefuseReason::BatchDuplicate,
                            ..
                        }
                    ));
                    assert_eq!(ledger.slot_operations().len(), operations);
                    assert_eq!(ledger.conservation().residue(), 0);
                }
            }
        }
    }

    #[test]
    fn acoustic_group_omission_retains_committed_plan() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, speech) = forensic_group_fixture(true, "czy weryfikowałeś");
            ledger.record_speech_evidence(&speech);
            assert!(
                ledger
                    .admit_word_slots(
                        &observation(producer, 1),
                        &bounded_group_words("czy plan weryfikowałeś")
                    )
                    .is_correct()
            );
            assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
            assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 3);
            let sources = ledger.slots_of(&owner()).unwrap().to_vec();
            for late in [
                ObservationProducer::Whisper,
                ObservationProducer::CloudLive,
                ObservationProducer::Apple,
                ObservationProducer::Lexicon,
                ObservationProducer::Formatter,
            ] {
                let operations = ledger.slot_operations().len();
                let refused =
                    ledger.admit_pinned_label(&observation(late, 2), "czy weryfikowałeś", &[]);
                assert!(!refused.grants_mutation());
                assert_eq!(ledger.slot_operations().len(), operations);
                assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
                assert_eq!(ledger.slots_of(&owner()).unwrap(), sources.as_slice());
                let alternative = ledger.slot_alternatives().last().unwrap();
                assert_eq!(alternative.candidate, "czy weryfikowałeś");
                assert_eq!(alternative.sources, sources.clone());
            }
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    // A group has acoustic coordinates but no child word boundaries. More
    // tokens are not evidence that every source word was accounted for.
    fn assert_group_omission_is_retained(candidate: &str, word_batch: bool) {
        let original = "czy plan weryfikowałeś";
        let mut failures = Vec::new();
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for pinned_group in [false, true] {
                let mut ledger = if pinned_group {
                    pinned(&[WordPin::new(0, 16_000, original)])
                } else {
                    let mut ledger = AcousticLedger::new();
                    ledger.admit(&observation(ObservationProducer::Apple, 0), original);
                    ledger
                };
                let before = ledger.slots_of(&owner()).unwrap().to_vec();
                let next = observation(producer, 1);
                if word_batch {
                    ledger.admit_word_slots(&next, &[WordPin::new(0, 16_000, candidate)]);
                } else {
                    ledger.admit_pinned_label(&next, candidate, &[]);
                }
                let retained = ledger.text_of(&owner()) == Some(original)
                    && ledger.slots_of(&owner()).unwrap() == before
                    && ledger.word_deletions().is_empty();
                let alternative = ledger.slot_alternatives().iter().any(|alternative| {
                    alternative.candidate == candidate
                        && alternative.sources == before
                        && alternative.observation == next
                });
                if !retained || !alternative {
                    failures.push(format!(
                        "{producer:?}, pinned_group={pinned_group}, word_batch={word_batch}: \
                         committed={:?}, source_retained={retained}, alternative={alternative}",
                        ledger.text_of(&owner())
                    ));
                }
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn relay_acceptance_equal_length_label_cannot_mask_group_omission() {
        assert_group_omission_is_retained("czy weryfikowałeś dokładnie", false);
    }

    #[test]
    fn relay_acceptance_longer_label_cannot_mask_group_omission() {
        assert_group_omission_is_retained("czy weryfikowałeś yyy [śmiech]", false);
    }

    #[test]
    fn relay_acceptance_equal_length_word_batch_cannot_mask_group_omission() {
        assert_group_omission_is_retained("czy weryfikowałeś dokładnie", true);
    }

    #[test]
    fn relay_acceptance_longer_word_batch_cannot_mask_group_omission() {
        assert_group_omission_is_retained("czy weryfikowałeś yyy [śmiech]", true);
    }

    #[test]
    fn whole_acoustic_label_cannot_merge_pinned_words() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let mut ledger = pinned(&[
                WordPin::new(0, 4_000, "czy"),
                WordPin::new(6_000, 10_000, "plan"),
                WordPin::new(12_000, 16_000, "weryfikowałeś"),
            ]);
            let sources = ledger.slots_of(&owner()).unwrap().to_vec();
            // Extra words cannot mask the missing pinned source.
            ledger.admit_pinned_label(
                &observation(producer, 1),
                "czy weryfikowałeś yyy [śmiech]",
                &[],
            );
            assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
            assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
            assert_eq!(
                ledger.slot_alternatives().last().unwrap().reason,
                "whole_label_has_no_word_targets"
            );
            let mut partial = pinned(&[WordPin::new(6_000, 10_000, "plan")]);
            let sources = partial.slots_of(&owner()).unwrap().to_vec();
            partial.admit_pinned_label(&observation(producer, 1), "nowy plan", &[]);
            assert_eq!(partial.slots_of(&owner()).unwrap(), sources);
            assert_eq!(partial.text_of(&owner()), Some("plan"));
        }
    }

    #[test]
    fn coarse_group_refinement_conserves_positions() {
        let mut ledger = AcousticLedger::new();
        ledger.admit(&observation(ObservationProducer::Apple, 0), "dwa słowa");
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[WordPin::new(0, 1_000, "jedno")],
        );
        assert_eq!(ledger.text_of(&owner()), Some("dwa słowa"));
        assert!(!ledger.slot_alternatives().is_empty());
        // The earlier proposal returned no complete decoded source scope.
        // The final batch now supplies the same measured capture and bounds.
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        let mut pcm = vec![0.0_f32; 16_000];
        pcm[320..1_000].fill(0.2);
        pcm[2_000..3_000].fill(0.2);
        let energy = CaptureEnergyOwner::bind("slot-test", 1);
        let mut writer = CaptureLevelAccumulator::bound_to(&energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let speech = energy.session_active_speech_ranges("slot-test", 1, 16_000);
        assert_eq!(
            speech.availability().observed_samples(),
            Some(pcm.len() as u64)
        );
        let integral = pcm
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        let peak = pcm
            .iter()
            .map(|sample| f64::from(sample.abs()))
            .fold(0.0_f64, f64::max);
        let calibration = EnergyCalibration::new("forensic-coarse-actual-pcm", 1.0, 1);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner(),
                        duration_ms: pcm.len() as f64 / 16.0,
                        energy_integral: integral,
                        mean_rms_dbfs: 20.0 * (integral / pcm.len() as f64).sqrt().log10(),
                        peak_dbfs: 20.0 * peak.log10(),
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(16_000),
                        evidence_calibration_version: calibration.version.clone()
                    },
                    &calibration
                )
                .is_qualified()
        );
        ledger.record_speech_evidence(&speech);
        ledger.schedule_frontier(owner(), [ObservationProducer::Whisper]);
        assert!(ledger.require_text_recovery(&owner()));
        let original = ledger.slots_of(&owner()).unwrap().to_vec();
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 3),
            &[
                WordPin::new(320, 1_000, "dwa").with_decode_window(0, pcm.len() as u64),
                WordPin::new(2_000, 3_000, "wyrazy").with_decode_window(0, pcm.len() as u64),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("dwa wyrazy"));
        let operation = ledger.slot_operations().last().unwrap();
        assert_eq!(operation.kind, SlotOperationKind::Split);
        assert_eq!(operation.sources[0].text, "dwa słowa");
        assert_eq!(
            ledger.slot_source_ranges(&operation.outputs[0])[0].sample_end,
            1_000
        );
        assert_eq!(operation.sources, original);
        assert!(operation.source_ranges.contains(&owner()));
        assert_eq!(operation.outputs.len(), 2);
        assert_eq!(
            (
                operation.outputs[0].sample_start,
                operation.outputs[0].sample_end
            ),
            (320, 1_000)
        );
        assert_eq!(
            (
                operation.outputs[1].sample_start,
                operation.outputs[1].sample_end
            ),
            (2_000, 3_000)
        );
        assert!(!ledger.text_recovery_pending(&owner()));
        assert!(ledger.note_frontier_return(&owner(), ObservationProducer::Whisper));
        ledger.seal(&owner()).unwrap();
        let coverage = ledger.assess_seal_coverage("slot-test", 1, &speech, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert!(ledger.record_seal_coverage(coverage));
        ledger.seal_terminal("slot-test", 1).unwrap();
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn dictionary_rewrites_preserve_pins_and_cannot_insert_an_unheard_word() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "doker"),
            WordPin::new(2_000, 3_000, "plan"),
        ]);
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let rule = DictionarySlotRule {
            id: "dictionary/doker/Docker/v1".into(),
            input: vec!["doker".into()],
            canonical: "Docker".into(),
        };
        ledger
            .rewrite_dictionary_slots(
                &observation(ObservationProducer::Lexicon, 1),
                &[(target, rule)],
            )
            .unwrap();
        assert_eq!(ledger.text_of(&owner()), Some("Docker plan"));
        assert_eq!(ledger.slots_of(&owner()).unwrap()[0].sample_end, 1_000);
        assert_eq!(
            ledger.slot_operations().last().unwrap().rule_id,
            "dictionary/doker/Docker/v1"
        );
        ledger.admit_word_slots(
            &observation(ObservationProducer::Lexicon, 2),
            &[WordPin::new(5_000, 6_000, "nowe")],
        );
        assert_eq!(ledger.text_of(&owner()), Some("Docker plan"));
    }

    #[test]
    fn five_physical_iwo_survive_a_whole_label_formatter() {
        let words = (0..5)
            .map(|i| WordPin::new(i * 2_000, i * 2_000 + 1_000, "Iwo"))
            .collect::<Vec<_>>();
        let mut ledger = pinned(&words);
        ledger.admit(&observation(ObservationProducer::Formatter, 1), "Iwo");
        assert_eq!(ledger.text_of(&owner()), Some("Iwo Iwo Iwo Iwo Iwo"));
        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 5);
    }

    #[test]
    fn omission_in_a_pinned_apple_revision_keeps_plan() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "czy"),
            WordPin::new(2_000, 3_000, "plan"),
            WordPin::new(4_000, 5_000, "zostaje"),
        ]);
        ledger.admit_pinned_label(
            &observation(ObservationProducer::Apple, 1),
            "czy zostaje",
            &[
                WordPin::new(0, 1_000, "czy"),
                WordPin::new(4_000, 5_000, "zostaje"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("czy plan zostaje"));
    }

    #[test]
    fn whisper_omission_keeps_plan_and_corrects_the_other_word() {
        let mut ledger = measured_pinned(&[
            WordPin::new(0, 1_000, "plan"),
            WordPin::new(2_000, 4_000, "weryfikowałeś"),
        ]);
        let original = ledger.slots_of(&owner()).unwrap().to_vec();
        let mut correction_observation = observation(ObservationProducer::Whisper, 1);
        corroborate_candidate(
            &mut ledger,
            &mut correction_observation,
            &[
                WordPin::new(2_050, 4_050, "zweryfikowałeś").with_decode_window(0, 16_000),
                WordPin::new(5_000, 6_000, "kod").with_decode_window(0, 16_000),
            ],
            (0, 20_000),
        );
        assert_eq!(ledger.text_of(&owner()), Some("plan zweryfikowałeś kod"));
        let slots = ledger.slots_of(&owner()).unwrap();
        assert_eq!(
            slots[0], original[0],
            "an omitted physical word remains held"
        );
        assert_eq!((slots[1].sample_start, slots[1].sample_end), (2_050, 4_050));
        let correction = ledger
            .slot_operations()
            .iter()
            .find(|operation| {
                operation.kind == SlotOperationKind::Correct
                    && operation.outputs.contains(&slots[1])
            })
            .expect("the stronger word keeps an explicit correction lineage");
        assert_eq!(correction.sources, vec![original[1].clone()]);
        assert!(
            correction
                .source_ranges
                .iter()
                .any(|range| range.sample_start == 2_000 && range.sample_end == 4_000)
        );
        assert_eq!(slots[1].producer, ObservationProducer::Whisper);
        let insertion = ledger
            .slot_operations()
            .iter()
            .find(|operation| {
                operation.kind == SlotOperationKind::Insert
                    && operation.outputs == [slots[2].clone()]
            })
            .expect("the new word has its own insertion receipt");
        assert!(insertion.sources.is_empty());
        assert_eq!(slots[2].producer, ObservationProducer::Whisper);
        assert_eq!((slots[2].sample_start, slots[2].sample_end), (5_000, 6_000));
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn sentence_compression_and_ambiguous_repetition_keep_sources_and_candidates() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "Iwo"),
            WordPin::new(1_100, 2_100, "Iwo"),
            WordPin::new(2_200, 3_200, "plan"),
        ]);
        let before = ledger.slots_of(&owner()).unwrap().to_vec();
        let candidate = observation(ObservationProducer::Whisper, 1);
        ledger.admit_word_slots(&candidate, &[WordPin::new(0, 3_200, "Iwo")]);
        assert_eq!(ledger.slots_of(&owner()).unwrap(), before);
        let alternative = ledger.slot_alternatives().last().unwrap();
        assert_eq!(alternative.sources, before);
        assert_eq!(alternative.observation, candidate);
        // Resegmentation now names why the compression was refused: the held
        // "Iwo Iwo plan" is not accounted for by one "Iwo".
        assert_eq!(alternative.reason, "incomplete_source_scope");
    }

    #[test]
    fn two_children_in_an_ordinary_batch_do_not_implicitly_split_one_slot() {
        let mut ledger = pinned(&[WordPin::new(0, 2_000, "naprawdę")]);
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[
                WordPin::new(0, 1_000, "na"),
                WordPin::new(1_000, 2_000, "prawdę"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("naprawdę"));
        // The refused partition is kept whole, as one candidate for the picker.
        assert_eq!(ledger.slot_alternatives().len(), 1);
        assert_eq!(ledger.slot_alternatives()[0].candidate, "na prawdę");
    }

    #[test]
    fn explicit_dictionary_merge_keeps_exact_source_ranges_and_rejects_stale_targets() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "na"),
            WordPin::new(2_000, 3_000, "prawdę"),
        ]);
        let targets = ledger
            .slots_of(&owner())
            .unwrap()
            .iter()
            .map(SlotTarget::from)
            .collect::<Vec<_>>();
        let rule = DictionarySlotRule {
            id: "dictionary/na-prawde/v1".into(),
            input: vec!["na".into(), "prawdę".into()],
            canonical: "naprawdę".into(),
        };
        let receipt = ledger
            .merge_word_slots(
                &observation(ObservationProducer::Lexicon, 1),
                &targets,
                &rule,
            )
            .unwrap();
        assert_eq!(ledger.text_of(&owner()), Some("naprawdę"));
        assert_eq!(
            receipt.source_ranges,
            vec![
                OccurrenceIdentity::new("slot-test", 1, 0, 1_000),
                OccurrenceIdentity::new("slot-test", 1, 2_000, 3_000),
            ]
        );
        assert_eq!(
            ledger.slot_source_ranges(&ledger.slots_of(&owner()).unwrap()[0]),
            receipt.source_ranges
        );
        assert_eq!(
            ledger.merge_word_slots(
                &observation(ObservationProducer::Lexicon, 2),
                &targets,
                &rule
            ),
            Err(SlotOperationRefusal::StaleTarget)
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn dictionary_merge_cannot_hide_a_word_outside_the_rule() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "na"),
            WordPin::new(2_000, 3_000, "prawdę"),
            WordPin::new(4_000, 5_000, "plan"),
        ]);
        let targets = ledger
            .slots_of(&owner())
            .unwrap()
            .iter()
            .map(SlotTarget::from)
            .collect::<Vec<_>>();
        let rule = DictionarySlotRule {
            id: "dictionary/na-prawde/v1".into(),
            input: vec!["na".into(), "prawdę".into()],
            canonical: "naprawdę".into(),
        };
        assert_eq!(
            ledger.merge_word_slots(
                &observation(ObservationProducer::Lexicon, 1),
                &targets,
                &rule
            ),
            Err(SlotOperationRefusal::InvalidEvidence)
        );
        assert_eq!(ledger.text_of(&owner()), Some("na prawdę plan"));
    }

    #[test]
    fn explicit_split_needs_child_boundaries_otherwise_text_has_group_accuracy() {
        let mut ledger = measured_pinned(&[WordPin::new(0, 2_000, "naprawdę")]);
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let next = observation(ObservationProducer::Whisper, 1);
        assert_eq!(
            ledger.split_word_slot(&next, &target, &[]),
            Err(SlotOperationRefusal::InvalidEvidence)
        );
        let children = [
            WordPin::new(0, 1_000, "na").with_decode_window(0, 16_000),
            WordPin::new(1_000, 2_000, "prawdę").with_decode_window(0, 16_000),
        ];
        let next = observation(ObservationProducer::Whisper, 2);
        assert_eq!(
            ledger.split_word_slot(&next, &target, &children),
            Err(SlotOperationRefusal::ProducerNotAuthorized)
        );
        let trial = ledger
            .next_word_trial(true)
            .expect("split requires a lexical resolution");
        let confirmed = observation(ObservationProducer::Whisper, 3);
        let witness = children
            .clone()
            .map(|pin| pin.with_decode_window(0, 20_000));
        ledger.stage_word_evidence(&confirmed, &witness, None, "unknown");
        let mut input = ledger.word_evidence_input(&confirmed).unwrap().clone();
        input.trial = Some(trial.clone());
        ledger.restore_word_evidence(input);
        let receipt = ledger
            .split_word_slot(&confirmed, &target, &witness)
            .unwrap();
        ledger.close_word_trial(&trial, "resolved");
        assert_eq!(receipt.sources.len(), 1);
        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 2);
        assert_eq!(ledger.text_of(&owner()), Some("na prawdę"));
        assert_eq!(
            ledger.slot_source_ranges(&ledger.slots_of(&owner()).unwrap()[0]),
            vec![OccurrenceIdentity::new("slot-test", 1, 0, 1_000)]
        );

        let mut group = measured_pinned(&[WordPin::new(0, 2_000, "naprawdę")]);
        let mut next = next;
        corroborate_candidate(
            &mut group,
            &mut next,
            &[WordPin::new(0, 2_000, "na prawdę").with_decode_window(0, 16_000)],
            (0, 20_000),
        );
        assert_eq!(group.slots_of(&owner()).unwrap().len(), 1);
        assert_eq!(group.text_of(&owner()), Some("na prawdę"));
    }

    #[test]
    fn overlap_windows_and_boundary_jitter_do_not_duplicate_five_iwo() {
        let words = (0..5)
            .map(|i| WordPin::new(i * 2_000 + 100, i * 2_000 + 1_100, "Iwo"))
            .collect::<Vec<_>>();
        let mut ledger = pinned(&words);
        for generation in 1..6 {
            let window = words[1..4]
                .iter()
                .map(|pin| WordPin::new(pin.sample_start + 30, pin.sample_end + 30, "Iwo"))
                .collect::<Vec<_>>();
            ledger.admit_word_slots(
                &observation(ObservationProducer::Whisper, generation),
                &window,
            );
        }
        assert_eq!(ledger.text_of(&owner()), Some("Iwo Iwo Iwo Iwo Iwo"));
        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 5);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn targeted_manual_human_survives_all_late_producers() {
        for end in [1_000, 16_000] {
            let mut ledger = pinned(&[WordPin::new(0, end, "plan")]);
            ledger.admit_word_slots(
                &observation(ObservationProducer::ManualHuman, 1),
                &[WordPin::new(0, end, "Plan")],
            );
            for producer in [
                ObservationProducer::Apple,
                ObservationProducer::CloudLive,
                ObservationProducer::Whisper,
                ObservationProducer::Lexicon,
                ObservationProducer::Formatter,
            ] {
                ledger.admit_word_slots(&observation(producer, 2), &[WordPin::new(0, end, "inna")]);
                ledger.admit_pinned_label(&observation(producer, 3), "whole sentence", &[]);
            }
            assert_eq!(ledger.text_of(&owner()), Some("Plan"));
            assert_eq!(
                ledger.slots_of(&owner()).unwrap()[0].producer,
                ObservationProducer::ManualHuman
            );
        }
    }

    use super::word_verdict::{WordVerdict, adjudicate_word_pcm};
    use crate::audio::capture_receipt::CaptureEvidenceIdentity;
    use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

    fn silero(ranges: &[(u64, u64)]) -> AcousticSpeechEvidence {
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("slot-test", 1),
            SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 16_000,
            },
            ranges
                .iter()
                .map(|&(sample_start, sample_end)| TailSampleRange {
                    session: "slot-test".into(),
                    capture_epoch: 1,
                    sample_start,
                    sample_end,
                })
                .collect(),
        )
    }

    #[test]
    fn hallucination_on_complete_no_speech_pcm_is_deleted_with_receipt() {
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "halucynacja"),
            WordPin::new(2_000, 3_000, "plan"),
        ]);
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let pcm_range = OccurrenceIdentity::new("slot-test", 1, 0, 1_000);
        let verdict = adjudicate_word_pcm(&pcm_range, &pcm_range, &[0.0; 1_000], &silero(&[]));
        assert_eq!(verdict.verdict(), WordVerdict::ConfirmedNoSpeech);
        let deletion = ledger
            .remove_word_with_verdict(
                &observation(ObservationProducer::Whisper, 1),
                &target,
                &verdict,
            )
            .unwrap();
        assert_eq!(deletion.verdict.energy_integral(), Some(0.0));
        assert!(!deletion.verdict.rule_version().is_empty());
        assert_eq!(ledger.text_of(&owner()), Some("plan"));
        assert_eq!(ledger.word_deletions().len(), 1);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn quiet_speech_and_speech_with_laughter_are_preserved() {
        let pcm_range = OccurrenceIdentity::new("slot-test", 1, 0, 1_000);
        let quiet = adjudicate_word_pcm(&pcm_range, &pcm_range, &[0.000_001; 1_000], &silero(&[]));
        assert_eq!(quiet.verdict(), WordVerdict::SoundUndetermined);
        let overlapping = adjudicate_word_pcm(
            &pcm_range,
            &pcm_range,
            &[0.3; 1_000],
            &silero(&[(0, 1_000)]),
        );
        assert_eq!(overlapping.verdict(), WordVerdict::SpeechEvidence);
        for verdict in [quiet, overlapping] {
            let mut ledger = pinned(&[WordPin::new(0, 1_000, "plan")]);
            let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
            assert_eq!(
                ledger.remove_word_with_verdict(
                    &observation(ObservationProducer::Whisper, 1),
                    &target,
                    &verdict
                ),
                Err(SlotOperationRefusal::InvalidEvidence)
            );
            assert_eq!(ledger.text_of(&owner()), Some("plan"));
        }
    }

    #[test]
    fn holes_wrong_epoch_invalid_pcm_and_sentence_energy_are_undetermined() {
        let range = OccurrenceIdentity::new("slot-test", 1, 0, 1_000);
        let foreign = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("slot-test", 2),
            SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 16_000,
            },
            vec![],
        );
        let hole = AcousticSpeechEvidence::unavailable(
            CaptureEvidenceIdentity::new("slot-test", 1),
            SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Discontinuous {
                observed_samples: 16_000,
            },
        );
        assert_eq!(
            adjudicate_word_pcm(&range, &range, &[0.0; 999], &silero(&[])).verdict(),
            WordVerdict::Undetermined
        );
        assert_eq!(
            adjudicate_word_pcm(&range, &range, &[0.0; 1_000], &foreign).verdict(),
            WordVerdict::Undetermined
        );
        assert_eq!(
            adjudicate_word_pcm(&range, &range, &[0.0; 1_000], &hole).verdict(),
            WordVerdict::Undetermined
        );
        assert_eq!(
            adjudicate_word_pcm(&range, &range, &[f32::NAN; 1_000], &silero(&[])).verdict(),
            WordVerdict::Undetermined
        );
        assert_eq!(
            adjudicate_word_pcm(&range, &owner(), &[0.0; 16_000], &silero(&[])).verdict(),
            WordVerdict::Undetermined
        );
    }

    #[test]
    fn no_speech_receipt_never_deletes_manual_human_or_a_merge_bounding_box() {
        let mut ledger = pinned(&[WordPin::new(0, 1_000, "plan")]);
        ledger.admit_word_slots(
            &observation(ObservationProducer::ManualHuman, 1),
            &[WordPin::new(0, 1_000, "Plan")],
        );
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let range = OccurrenceIdentity::new("slot-test", 1, 0, 1_000);
        let verdict = adjudicate_word_pcm(&range, &range, &[0.0; 1_000], &silero(&[]));
        assert_eq!(
            ledger.remove_word_with_verdict(
                &observation(ObservationProducer::ManualHuman, 2),
                &target,
                &verdict
            ),
            Err(SlotOperationRefusal::ProtectedHuman)
        );
        assert_eq!(ledger.text_of(&owner()), Some("Plan"));

        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "na"),
            WordPin::new(2_000, 3_000, "prawdę"),
        ]);
        let targets = ledger
            .slots_of(&owner())
            .unwrap()
            .iter()
            .map(SlotTarget::from)
            .collect::<Vec<_>>();
        let rule = DictionarySlotRule {
            id: "dictionary/na-prawde/v1".into(),
            input: vec!["na".into(), "prawdę".into()],
            canonical: "naprawdę".into(),
        };
        ledger
            .merge_word_slots(
                &observation(ObservationProducer::Lexicon, 1),
                &targets,
                &rule,
            )
            .unwrap();
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let range = OccurrenceIdentity::new("slot-test", 1, 0, 3_000);
        let verdict = adjudicate_word_pcm(&range, &range, &[0.0; 3_000], &silero(&[]));
        assert_eq!(
            ledger.remove_word_with_verdict(
                &observation(ObservationProducer::Whisper, 2),
                &target,
                &verdict
            ),
            Err(SlotOperationRefusal::InvalidEvidence)
        );
        assert_eq!(ledger.text_of(&owner()), Some("naprawdę"));
    }

    #[test]
    fn no_speech_receipt_blocks_reinsertion_after_pcm_is_no_longer_available() {
        let mut ledger = pinned(&[WordPin::new(0, 1_000, "phantom")]);
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let range = OccurrenceIdentity::new("slot-test", 1, 0, 1_000);
        let verdict = adjudicate_word_pcm(&range, &range, &[0.0; 1_000], &silero(&[]));
        ledger
            .remove_word_with_verdict(
                &observation(ObservationProducer::Whisper, 1),
                &target,
                &verdict,
            )
            .unwrap();
        // No PCM or VAD reader is consulted on the late admission.
        let receipt = ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 2),
            &[WordPin::new(0, 1_000, "other")],
        );
        assert!(matches!(
            receipt,
            MutationReceipt::Refuse {
                reason: RefuseReason::ConfirmedNoSpeech,
                ..
            }
        ));
        assert_eq!(ledger.text_of(&owner()), Some(""));
        ledger.admit(
            &observation(ObservationProducer::Whisper, 3),
            "unlocated late text",
        );
        assert_eq!(ledger.text_of(&owner()), Some(""));
        assert_eq!(
            ledger.slot_alternatives().last().unwrap().reason,
            "word_targets_required_after_no_speech"
        );
        ledger.admit_word_slots(
            &observation(ObservationProducer::ManualHuman, 3),
            &[WordPin::new(0, 1_000, "human")],
        );
        assert_eq!(ledger.text_of(&owner()), Some("human"));
        assert_eq!(ledger.conservation().residue(), 0);
    }
    // Root-only debt controls from the authentic archived PCM boundary lead.
    #[test]
    fn forensic_edge_debt_complete_word_survives_late_decode_cut_without_false_debt() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, pcm) = forensic_split_empty_capture("full-edge-source");
            ledger.schedule_frontier(owner.clone(), [producer]);
            let first = ledger.next_word_observation(producer, 995, &owner);
            let receipt = ledger.admit_word_slots(
                &first,
                &[
                    WordPin::new(owner.sample_start, owner.sample_end + 400, "Iwo")
                        .with_decode_window(0, pcm.len() as u64),
                ],
            );
            assert!(receipt.grants_mutation(), "{receipt:?}");
            let source = ledger.slots_of(&owner).unwrap().to_vec();
            assert!(ledger.complete_word_slot(&source[0]));
            assert!(ledger.returned_word_scope_accounted(&first));
            assert!(ledger.require_text_recovery(&owner));
            let late = ledger.next_word_observation(producer, 996, &owner);
            let receipt = ledger.admit_word_slots(
                &late,
                &[WordPin::new(owner.sample_start, owner.sample_end, "I")
                    .with_decode_window(0, owner.sample_end)],
            );
            assert!(
                !receipt.grants_mutation(),
                "an edge stub cannot overwrite the complete word"
            );
            assert_eq!(ledger.slots_of(&owner).unwrap(), source);
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .any(
                        |alternative| alternative.reason == "incomplete_source_scope"
                            && alternative.sources == source
                    )
            );
            assert!(ledger.returned_word_scope_accounted(&first));
            assert!(ledger.note_frontier_return(&owner, producer));
            assert!(
                !ledger.text_recovery_pending(&owner),
                "a refused edge stub does not invalidate the retained complete decoded source"
            );
            assert!(ledger.seal(&owner).is_ok());
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_edge_debt_incomplete_word_and_frontier_return_cannot_claim_settlement() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, _) = forensic_split_empty_capture("incomplete-edge-source");
            ledger.schedule_frontier(owner.clone(), [producer]);
            let first = ledger.next_word_observation(producer, 997, &owner);
            let receipt = ledger.admit_word_slots(
                &first,
                &[WordPin::new(owner.sample_start, owner.sample_end, "I")
                    .with_decode_window(0, owner.sample_end)],
            );
            assert!(receipt.grants_mutation(), "{receipt:?}");
            let source = ledger.slots_of(&owner).unwrap().to_vec();
            assert!(!ledger.complete_word_slot(&source[0]));
            assert!(!ledger.returned_word_scope_accounted(&first));
            assert!(ledger.require_text_recovery(&owner));
            assert!(ledger.note_frontier_return(&owner, producer));
            assert!(ledger.text_recovery_pending(&owner));
            assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
            assert_eq!(ledger.slots_of(&owner).unwrap(), source);
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }
    #[test]
    fn forensic_merge_intersection_only_pin_cannot_retire_two_complete_words() {
        use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let session = format!("B45-{producer:?}");
            let owner = OccurrenceIdentity::new(&session, 91, 0, 32_000);
            let pcm = vec![0.2_f32; 32_000];
            let energy = CaptureEnergyOwner::bind(&session, 91);
            let mut writer = CaptureLevelAccumulator::bound_to(&energy);
            for samples in pcm.chunks(320) {
                writer.push_samples(samples);
            }
            let speech = energy.session_active_speech_ranges(&session, 91, 16_000);
            assert_eq!(speech.availability().observed_samples(), Some(32_000));
            let calibration = EnergyCalibration::new("B45-measured-pcm", 1.0, 1);
            let mut ledger = AcousticLedger::new();
            ledger.bind_capture_rate(16_000);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: owner.clone(),
                            duration_ms: 2_000.0,
                            energy_integral: pcm.iter().map(|x| f64::from(*x).powi(2)).sum(),
                            mean_rms_dbfs: 20.0 * f64::from(0.2_f32).log10(),
                            peak_dbfs: 20.0 * f64::from(0.2_f32).log10(),
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(32_000),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration
                    )
                    .is_qualified()
            );
            ledger.record_speech_evidence(&speech);
            let first = ledger.next_word_observation(producer, 1, &owner);
            assert!(
                ledger
                    .admit_word_slots(
                        &first,
                        &[WordPin::new(4_000, 14_000, "Iwo").with_decode_window(0, 32_000)]
                    )
                    .grants_mutation()
            );
            let second = ledger.next_word_observation(producer, 2, &owner);
            assert!(
                ledger
                    .admit_word_slots(
                        &second,
                        &[WordPin::new(13_000, 20_000, "Iwo").with_decode_window(0, 32_000)]
                    )
                    .grants_mutation()
            );
            assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo"));
            let sources = ledger
                .slots_of(&owner)
                .expect("two complete sources")
                .to_vec();
            assert_eq!(sources.len(), 2);
            assert!(sources.iter().all(|word| ledger.complete_word_slot(word)));
            assert!(sources.iter().all(|word| {
                ledger
                    .complete_decoded_words
                    .get(&word.observation)
                    .is_some_and(|ranges| ranges.contains(&(word.sample_start, word.sample_end)))
            }));
            let next = ledger.next_word_observation(producer, 3, &owner);
            let pin = WordPin::new(13_000, 14_000, "Iwo").with_decode_window(0, 32_000);
            assert!(
                sources.iter().all(|word| {
                    let midpoint = word.sample_start + (word.sample_end - word.sample_start) / 2;
                    midpoint < pin.sample_start || midpoint >= pin.sample_end
                }),
                "the new pin contains zero previous centres, but intersects both physical sources"
            );
            let decision = ledger.admit_word_slots(&next, &[pin]);
            assert!(
                !decision.grants_mutation(),
                "an intersection-only pin cannot merge distinct complete Words: {decision:?}"
            );
            assert_eq!(
                ledger
                    .slots_of(&owner)
                    .expect("conserved complete source words"),
                sources.as_slice(),
                "an intersection-only pin is not a disposition for either complete physical source"
            );
            assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo"));
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_merge_dictionary_rule_merges_the_exact_same_source_words() {
        let (mut ledger, owner, _) = forensic_merge_capture("B45-dictionary-source", 2);
        let sources = ledger
            .slots_of(&owner)
            .expect("complete source words")
            .to_vec();
        let targets = sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
        let expected_ranges = sources
            .iter()
            .flat_map(|slot| ledger.slot_source_ranges(slot))
            .collect::<Vec<_>>();
        let next = ledger.next_word_observation(ObservationProducer::Lexicon, 712, &owner);
        let rule = DictionarySlotRule {
            id: "dictionary/B45-na-prawde/v1".into(),
            input: vec!["na".into(), "prawdę".into()],
            canonical: "naprawdę".into(),
        };
        let receipt = ledger
            .merge_word_slots(&next, &targets, &rule)
            .expect("authorized exact Dictionary merge");
        assert_eq!(ledger.text_of(&owner), Some("naprawdę"));
        assert_eq!(receipt.sources, sources);
        assert_eq!(receipt.source_ranges, expected_ranges);
        assert_eq!(receipt.kind, SlotOperationKind::Merge);
        assert_eq!(receipt.rule_id, rule.id);
        let output = &ledger.slots_of(&owner).expect("merged output")[0];
        for source in &receipt.sources {
            assert!(ledger.slot_descends_from(output, source));
        }
        assert_eq!(ledger.slot_source_ranges(output), receipt.source_ranges);
        let replay = ledger.next_word_observation(ObservationProducer::Lexicon, 713, &owner);
        assert_eq!(
            ledger.merge_word_slots(&replay, &targets, &rule),
            Err(SlotOperationRefusal::StaleTarget)
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_merge_coarse_apple_group_is_still_refinable_by_whisper() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut ledger, owner, pcm) = forensic_split_empty_capture("B45-coarse-source");
            let apple = ledger.next_word_observation(ObservationProducer::Apple, 714, &owner);
            assert!(
                ledger
                    .admit_pinned_label(
                        &apple,
                        "na prawdę",
                        &[WordPin::new(3_200, 9_600, "na prawdę")]
                    )
                    .grants_mutation()
            );
            let sources = ledger
                .slots_of(&owner)
                .expect("coarse source group")
                .to_vec();
            assert_eq!(sources.len(), 1);
            let next = ledger.next_word_observation(producer, 715, &owner);
            let decision = ledger.admit_word_slots(
                &next,
                &[WordPin::new(3_200, 9_600, "naprawdę").with_decode_window(0, pcm.len() as u64)],
            );
            assert!(
                decision.grants_mutation(),
                "a measured Word may refine a coarse Apple group: {decision:?}"
            );
            assert_eq!(ledger.text_of(&owner), Some("naprawdę"));
            let output = &ledger.slots_of(&owner).expect("measured output")[0];
            assert_eq!(output.observation, next);
            assert!(ledger.slot_descends_from(output, &sources[0]));
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }
    #[test]
    fn forensic_timed_words_correct_coarse_hypothesis_with_lineage() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for replacement in ["zweryfikowałeś.", "weryfikowałeś"] {
                let (mut ledger, owner, pcm) =
                    forensic_split_empty_capture("forensic-qualified-group");
                assert_eq!(pcm.len(), 16_000);
                let source_obs =
                    ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
                assert!(
                    ledger
                        .admit_word_slots(
                            &source_obs,
                            &[WordPin::new(3_200, 9_600, "czy plan weryfikowałeś")]
                        )
                        .grants_mutation()
                );
                let source = ledger.slots_of(&owner).unwrap()[0].clone();
                let next = ledger.next_word_observation(producer, 2, &owner);
                let middle = if replacement == "weryfikowałeś" {
                    "klan"
                } else {
                    "PLAN,"
                };
                let pins = [
                    WordPin::new(3_200, 5_200, "Czy"),
                    WordPin::new(5_200, 7_200, middle),
                    WordPin::new(7_200, 9_600, replacement),
                ]
                .map(|p| p.with_decode_window(0, 16_000));
                let receipt = ledger.admit_word_slots(&next, &pins);
                assert!(receipt.grants_mutation(), "{receipt:?}");
                assert_eq!(
                    ledger.text_of(&owner),
                    Some(format!("Czy {middle} {replacement}").as_str())
                );
                let slots = ledger.slots_of(&owner).unwrap();
                assert_eq!(slots.len(), 3);
                for (s, p) in slots.iter().zip(&pins) {
                    assert_eq!(
                        (s.sample_start, s.sample_end),
                        (p.sample_start, p.sample_end)
                    );
                    assert_eq!(s.producer, producer);
                }
                assert_eq!(
                    ledger
                        .slot_operations()
                        .iter()
                        .filter(|op| op.sources.contains(&source))
                        .count(),
                    1
                );
                assert!(ledger.slot_alternatives().iter().any(
                    |a| a.candidate == source.text && a.reason == "resegmentation_source_label"
                ));
                assert!(ledger.word_deletions().is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }
}
