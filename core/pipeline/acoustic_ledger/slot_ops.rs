//! Explicit operations address a source observation and exact PCM pins.
//! Dictionary rules authorize a merge; timed child pins authorize a split.

use super::*;

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
        Ok(decision)
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
        self.commit_slot_operation(receipt.clone());
        Ok(receipt)
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
        self.commit_slot_operation(receipt.clone());
        Ok(receipt)
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
