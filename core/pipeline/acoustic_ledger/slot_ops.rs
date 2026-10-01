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

#[derive(Debug, Clone, PartialEq, Eq)]
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

impl AcousticLedger {
    /// Snapshot the existing capture observer; unavailable evidence replaces
    /// the previous snapshot so stale measurements cannot authorize a cut.
    pub fn record_speech_evidence(&mut self, speech: &AcousticSpeechEvidence) {
        self.speech_evidence = Some(speech.clone());
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
        use crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER;
        use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

        let owner = &observation.occurrence;
        let speech = self.speech_evidence.as_ref()?;
        let observed = speech.availability().observed_samples()?;
        if !speech
            .identity()
            .matches(&owner.session, owner.capture_epoch)
            || !matches!(
                speech.producer(),
                CAPTURE_ENERGY_PRODUCER | SILERO_BOUNDARIES_PRODUCER
            )
            || observed < source.sample_end
            || speech.ranges().iter().any(|range| {
                let range = OccurrenceIdentity::from(range);
                !range.same_capture(owner) || !range.is_anchored() || range.sample_end > observed
            })
        {
            return None;
        }
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
                // A candidate cannot lend itself additional geometry from the
                // routing snapshot. Only a distinct, adjacent owner may do so.
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
                    && (self.committed.contains_key(assigned_owner)
                        || self.is_qualified(assigned_owner))
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
                    pins.push((pin.clone(), assigned_owner.clone()));
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
                        if receipt.kind == SlotOperationKind::Split {
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
        self.answered.push(observation.clone());
        self.word_pin_observations.insert(observation.clone());
        self.record_layer_decision(observation, &label, &decision);
        self.slot_operations.push(receipt);
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
        let sources = self.resolve_slot_targets(observation, std::slice::from_ref(target), true)?;
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
            return Err(SlotOperationRefusal::InvalidEvidence);
        }
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
            .collect();
        let receipt = SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Split,
            source_ranges: self.slot_source_ranges(source),
            sources,
            outputs,
            rule_id: "producer_child_pcm_boundaries/v1".to_string(),
        };
        let trace = super::super::trail::SlotTrace::split(self, observation, target, children);
        self.commit_slot_operation(receipt.clone());
        trace.finish(Ok(receipt), self)
    }
}

#[cfg(test)]
mod slot_ops_tests {
    use super::*;

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
                        let operation = ledger.slot_operations().last().unwrap();
                        assert!(operation.rule_id.contains("held_token_retained"));
                        assert_eq!(operation.sources[0].text, "czy plan weryfikowałeś");
                        assert_eq!(operation.source_ranges, vec![owner()]);
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
                        if via_label {
                            ledger.admit_pinned_label(&next, candidate, &[]);
                        } else {
                            ledger.admit_word_slots(&next, &[WordPin::new(0, 16_000, candidate)]);
                        }
                        assert_eq!(
                            ledger.text_of(&owner()),
                            Some(expected),
                            "{held} → {candidate}"
                        );
                        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 1);
                        assert_eq!(
                            ledger
                                .slot_operations()
                                .last()
                                .unwrap()
                                .rule_id
                                .contains("held_token_retained"),
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
        let pins = [
            WordPin::new(0, 4_000, "czy"),
            WordPin::new(6_000, 16_000, "weryfikowałeś dokładnie"),
        ];
        for speech in [None, Some(group_speech(&[(0, 16_000)]))] {
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
                if let Some(speech) = &speech {
                    ledger.record_speech_evidence(speech);
                }
                ledger.admit_word_slots(&observation(ObservationProducer::Whisper, 1), &pins);
                assert_eq!(ledger.slots_of(&owner()).unwrap(), source);
                assert!(!ledger.slot_alternatives().is_empty());
                assert!(ledger.group_speech_coverages().is_empty());
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
        for speech in evidence {
            let mut ledger = AcousticLedger::new();
            ledger.admit(
                &observation(ObservationProducer::Apple, 0),
                "czy plan weryfikowałeś",
            );
            let source = ledger.slots_of(&owner()).unwrap().to_vec();
            ledger.record_speech_evidence(&valid);
            ledger.record_speech_evidence(&speech);
            ledger.admit_word_slots(&observation(ObservationProducer::Whisper, 1), &pins);
            assert_eq!(ledger.slots_of(&owner()).unwrap(), source, "{speech:?}");
            assert!(ledger.group_speech_coverages().is_empty());
            assert!(!ledger.slot_alternatives().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn adjacent_assigned_pin_covers_speech_without_lending_its_word() {
        let neighbour = OccurrenceIdentity::new(owner().session, 1, 16_000, 32_000);
        let next = observation(ObservationProducer::Whisper, 1);
        let neighbouring_pin = OccurrenceIdentity::new(owner().session, 1, 14_000, 20_000);
        let pins = [
            WordPin::new(0, 4_000, "czy"),
            WordPin::new(6_000, 10_000, "weryfikowałeś"),
        ];
        let mut speech = group_speech(&[(0, 4_000), (6_000, 10_000), (14_000, 16_000)]);
        // The original neighbouring pin is authenticated through its end.
        speech = AcousticSpeechEvidence::measured(
            speech.identity().clone(),
            speech.producer(),
            AcousticAvailability::Observed {
                observed_samples: 32_000,
            },
            speech.ranges().to_vec(),
        );
        for labelled_neighbour in [false, true] {
            for variant in 0..4 {
                let matching_request = variant == 0;
                let mut ledger = AcousticLedger::new();
                ledger.admit(
                    &observation(ObservationProducer::Apple, 0),
                    "czy plan weryfikowałeś",
                );
                if labelled_neighbour {
                    ledger.admit(
                        &ObservationIdentity::new(
                            ObservationProducer::Apple,
                            0,
                            0,
                            neighbour.clone(),
                        ),
                        "sąsiad",
                    );
                } else {
                    let calibration = EnergyCalibration {
                        version: "a2-neighbour".into(),
                        min_energy_integral: 1.0,
                        min_valley_samples: 1,
                    };
                    ledger.qualify(
                        &AcousticEvidence {
                            occurrence: neighbour.clone(),
                            duration_ms: 1_000.0,
                            energy_integral: 10.0,
                            mean_rms_dbfs: -12.0,
                            peak_dbfs: -3.0,
                            vad_open_sample: Some(16_000),
                            vad_close_sample: Some(32_000),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration,
                    );
                    assert!(ledger.is_qualified(&neighbour));
                }
                ledger.record_speech_evidence(&speech);
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
                ledger.admit_word_slots(&next, &pins);
                if matching_request {
                    assert_eq!(ledger.text_of(&owner()), Some("czy weryfikowałeś"));
                    let coverage = ledger.group_speech_coverages().last().unwrap();
                    assert_eq!(coverage.coverage.last().unwrap().pin_owner, neighbour);
                    assert_eq!(
                        coverage.coverage.last().unwrap().pin_range,
                        neighbouring_pin
                    );
                    assert_eq!(coverage.operation.outputs.len(), 2);
                } else {
                    assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
                    assert!(ledger.group_speech_coverages().is_empty());
                }
                assert_eq!(
                    ledger.text_of(&neighbour),
                    labelled_neighbour.then_some("sąsiad")
                );
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
        let mut ledger = AcousticLedger::new();
        ledger.admit(
            &observation(ObservationProducer::Apple, 0),
            "czy plan weryfikowałeś",
        );
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
            Vec::new(),
        );
        let verdict = adjudicate_word_pcm(&quiet_pin, &quiet_pin, &[0.0; 4_000], &silero);
        ledger
            .remove_word_with_verdict(
                &ObservationIdentity::new(ObservationProducer::Whisper, 0, 0, neighbour.clone()),
                &target,
                &verdict,
            )
            .unwrap();
        let speech = group_speech(&[(0, 4_000), (6_000, 10_000), (14_000, 16_000)]);
        let speech = AcousticSpeechEvidence::measured(
            speech.identity().clone(),
            speech.producer(),
            AcousticAvailability::Observed {
                observed_samples: 32_000,
            },
            speech.ranges().to_vec(),
        );
        ledger.record_speech_evidence(&speech);
        let next = observation(ObservationProducer::Whisper, 1);
        ledger.record_assigned_word_pins(
            &next,
            &[(
                neighbour.clone(),
                OccurrenceIdentity::new(owner().session, 1, 14_000, 20_000),
            )],
        );
        ledger.admit_word_slots(
            &next,
            &[
                WordPin::new(0, 4_000, "czy"),
                WordPin::new(6_000, 10_000, "weryfikowałeś"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
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
            if via_label {
                ledger.admit_pinned_label(
                    &observation(ObservationProducer::Formatter, 1),
                    "czy weryfikowałeś dokładnie",
                    &[],
                );
            } else {
                ledger.admit_word_slots(
                    &observation(ObservationProducer::Formatter, 1),
                    &[WordPin::new(0, 16_000, "czy weryfikowałeś dokładnie")],
                );
            }
            assert_eq!(ledger.slots_of(&owner()).unwrap(), sources);
            assert_eq!(
                ledger.slot_alternatives().last().unwrap().reason,
                "protected_source"
            );
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
                    let mut ledger = if has_pin {
                        pinned(&[WordPin::new(0, 16_000, apple)])
                    } else {
                        let mut ledger = AcousticLedger::new();
                        ledger.admit(&observation(ObservationProducer::Apple, 0), apple);
                        ledger
                    };
                    ledger.require_text_recovery(&owner());
                    let source = ledger.slots_of(&owner()).unwrap()[0].clone();
                    let next = observation(producer, 1);
                    let receipt = ledger.admit_pinned_label(&next, acoustic, &[]);
                    assert!(receipt.is_correct(), "{producer:?}: {apple} → {acoustic}");
                    assert_eq!(ledger.text_of(&owner()), Some(acoustic));
                    assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 1);
                    let operation = ledger.slot_operations().last().unwrap();
                    assert_eq!(operation.kind, SlotOperationKind::Correct);
                    assert_eq!(operation.sources, vec![source]);
                    assert_eq!(operation.source_ranges, vec![owner()]);
                    assert_eq!(
                        ledger.slot_source_ranges(&operation.outputs[0]),
                        vec![owner()]
                    );
                    assert!(!ledger.text_recovery_pending(&owner()));
                    let operations = ledger.slot_operations().len();
                    assert!(matches!(
                        ledger.admit_pinned_label(&next, acoustic, &[]),
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
            let mut ledger = pinned(&[WordPin::new(0, 16_000, "czy weryfikowałeś")]);
            ledger.admit_pinned_label(&observation(producer, 1), "czy plan weryfikowałeś", &[]);
            let source = ledger.slots_of(&owner()).unwrap()[0].clone();
            for late in [
                ObservationProducer::Whisper,
                ObservationProducer::CloudLive,
                ObservationProducer::Apple,
                ObservationProducer::Lexicon,
                ObservationProducer::Formatter,
            ] {
                ledger.admit_pinned_label(&observation(late, 2), "czy weryfikowałeś", &[]);
                assert_eq!(ledger.text_of(&owner()), Some("czy plan weryfikowałeś"));
                assert_eq!(
                    ledger.slots_of(&owner()).unwrap(),
                    std::slice::from_ref(&source)
                );
                let alternative = ledger.slot_alternatives().last().unwrap();
                assert_eq!(alternative.candidate, "czy weryfikowałeś");
                assert_eq!(alternative.sources, vec![source.clone()]);
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
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 3),
            &[
                WordPin::new(0, 1_000, "dwa"),
                WordPin::new(2_000, 3_000, "wyrazy"),
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
        let mut ledger = pinned(&[
            WordPin::new(0, 1_000, "plan"),
            WordPin::new(2_000, 4_000, "weryfikowałeś"),
        ]);
        ledger.admit_word_slots(
            &observation(ObservationProducer::Whisper, 1),
            &[
                WordPin::new(2_050, 4_050, "zweryfikowałeś"),
                WordPin::new(5_000, 6_000, "kod"),
            ],
        );
        assert_eq!(ledger.text_of(&owner()), Some("plan zweryfikowałeś kod"));
        let slots = ledger.slots_of(&owner()).unwrap();
        assert_eq!((slots[1].sample_start, slots[1].sample_end), (2_000, 4_000));
        assert_eq!(slots[1].producer, ObservationProducer::Whisper);
        assert_eq!(
            ledger.slot_operations().last().unwrap().kind,
            SlotOperationKind::Insert
        );
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
        assert_eq!(alternative.reason, "ambiguous_pcm_target");
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
        assert_eq!(ledger.slot_alternatives().len(), 2);
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
        let mut ledger = pinned(&[WordPin::new(0, 2_000, "naprawdę")]);
        let target = SlotTarget::from(&ledger.slots_of(&owner()).unwrap()[0]);
        let next = observation(ObservationProducer::Whisper, 1);
        assert_eq!(
            ledger.split_word_slot(&next, &target, &[]),
            Err(SlotOperationRefusal::InvalidEvidence)
        );
        let receipt = ledger
            .split_word_slot(
                &next,
                &target,
                &[
                    WordPin::new(0, 1_000, "na"),
                    WordPin::new(1_000, 2_000, "prawdę"),
                ],
            )
            .unwrap();
        assert_eq!(receipt.sources.len(), 1);
        assert_eq!(ledger.slots_of(&owner()).unwrap().len(), 2);
        assert_eq!(ledger.text_of(&owner()), Some("na prawdę"));
        assert_eq!(
            ledger.slot_source_ranges(&ledger.slots_of(&owner()).unwrap()[0]),
            vec![OccurrenceIdentity::new("slot-test", 1, 0, 1_000)]
        );

        let mut group = pinned(&[WordPin::new(0, 2_000, "naprawdę")]);
        group.admit_word_slots(&next, &[WordPin::new(0, 2_000, "na prawdę")]);
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
}
