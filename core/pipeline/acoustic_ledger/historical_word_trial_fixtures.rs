//! Constructors for historical word-trial receipts used by adversarial fixtures.
//! Native capture has no trial scheduler. These helpers exist only in the test
//! binary so old recorded states can still falsify geometry and replay integrity.
use super::*;

impl AcousticLedger {
    pub(crate) fn next_word_trial(&mut self, stopping: bool) -> Option<WordTrial> {
        self.next_word_trial_in(stopping, None, None)
    }

    pub(crate) fn next_word_trial_in(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        // Required padding and retained capture bounds. With coverage, the
        // returned frame must supply this context. Without coverage, exclude
        // already observed planned frames before spending a live trial.
        context: Option<(u64, std::ops::Range<u64>)>,
    ) -> Option<WordTrial> {
        self.select_word_trial(stopping, coverage, context, u64::MAX, false)
    }

    pub(crate) fn next_word_trial_before_horizon(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        context: Option<(u64, std::ops::Range<u64>)>,
        horizon: u64,
    ) -> Option<WordTrial> {
        self.select_word_trial(stopping, coverage, context, horizon, true)
    }

    pub(crate) fn word_trial_has_decode(
        &self,
        trial: &WordTrial,
        decode: &OccurrenceIdentity,
    ) -> bool {
        self.word_adjudication
            .components
            .iter()
            .filter(|component| component.trial.as_ref() == Some(trial))
            .any(|component| {
                component.has_decode(decode, Some((decode.sample_start, decode.sample_end)))
            })
    }

    pub(crate) fn admit_word_trial(
        &mut self,
        trial: &WordTrial,
        observation: &ObservationIdentity,
        words: &[WordPin],
        original: &[WordPin],
    ) -> MutationReceipt {
        // A reserved coarse group can receive its measured child pins. It
        // cannot pass the one-word matcher before ordinary admission has
        // proved that partition. Keep every child inside the reserved group;
        // context still cannot create targets or rewrite unreserved neighbors.
        let current = self.slots_of(&trial.owner).unwrap_or(&[]);
        let targets_pin = |word: &&WordPin| {
            let range = OccurrenceIdentity::new(
                &trial.owner.session,
                trial.owner.capture_epoch,
                word.sample_start,
                word.sample_end,
            );
            current.iter().any(|slot| {
                trial.targets.contains(&SlotTarget::from(slot))
                    && (self.word_slot_targets_pin(slot, &range)
                        || (self.coarse_word_source(slot)
                            && range.is_anchored()
                            && range.same_capture(&slot.observation.occurrence)
                            && slot.sample_start <= range.sample_start
                            && range.sample_end <= slot.sample_end))
            })
        };
        let pins = words
            .iter()
            .filter(targets_pin)
            .cloned()
            .collect::<Vec<_>>();
        let raw = original
            .iter()
            .filter(targets_pin)
            .cloned()
            .collect::<Vec<_>>();
        self.stage_word_evidence(observation, &raw, None, "unknown");
        self.word_adjudication
            .input
            .as_mut()
            .expect("staged trial")
            .trial = Some(trial.clone());
        let refusal = self.word_trial_input_refusal(observation, &pins);
        let receipt = self.admit_word_slots(observation, &pins);
        let resolved = self
            .word_adjudication
            .choices
            .iter()
            .rev()
            .find(|choice| choice.observation == *observation)
            .is_some_and(|choice| choice.reason == "trial_confirmed");
        self.close_word_trial(
            trial,
            refusal.unwrap_or(if resolved { "resolved" } else { "unresolved" }),
        );
        receipt
    }

    fn select_word_trial(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        context: Option<(u64, std::ops::Range<u64>)>,
        horizon: u64,
        fresh_decode: bool,
    ) -> Option<WordTrial> {
        let (index, sources) = self
            .word_adjudication
            .components
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                if !c.conflict || c.owner.sample_end > horizon || self.is_sealed(&c.owner) {
                    return None;
                }
                let sources = self.word_trial_sources(index, fresh_decode)?;
                let targets = sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
                let grouped;
                let evidence = if targets == c.targets {
                    c
                } else {
                    grouped = self.component_for_sources(&c.owner, &sources);
                    &grouped
                };
                if !stopping {
                    let support = evidence.support();
                    let complete_frames = support
                        .iter()
                        .filter(|h| h.family() == ObservationProducer::Whisper && h.complete)
                        .filter_map(|h| h.decode)
                        .collect::<BTreeSet<_>>()
                        .len();
                    // An early owner can have only one covering grid frame.
                    // An incomplete Apple label must not make its second real
                    // witness impossible to request. This spends the existing
                    // trial budget; admission still requires fresh agreement.
                    // A changed negation neighbour is itself a lexical dispute,
                    // not a reason to forbid collecting its resolving evidence.
                    // Only eligibility changes here: this witness alone still
                    // cannot revise the held label or its physical partition.
                    let first_banded_witness = evidence.apple.last().is_some_and(|apple| {
                        !(apple.complete && apple.acoustic_boundaries_complete)
                            && apple.original_text.is_some()
                            && support.iter().any(|h| {
                                h.family() == ObservationProducer::Whisper
                                    && h.complete
                                    && h.acoustic_boundaries_complete
                                    && h.decode.is_some()
                                    && h.q >= BAND_RIGHTS_FLOOR
                                    && h.original_text.is_some()
                            })
                    });
                    if complete_frames < 2 && !first_banded_witness {
                        return None;
                    }
                }
                if coverage.is_none() && context.is_none() {
                    return Some((index, sources));
                }
                let ranges = self.word_source_ranges(&sources);
                if let Some(coverage) = coverage
                    && (evidence
                        .has_decode(coverage, Some((coverage.sample_start, coverage.sample_end)))
                        || !ranges.iter().all(|range| {
                            range.same_capture(coverage)
                                && self.decode_word_onset_complete(
                                    range,
                                    coverage.sample_start,
                                    coverage.sample_end,
                                    range.sample_start,
                                )
                                && range.sample_end <= coverage.sample_end
                                && !self.decode_word_fence_incomplete(
                                    range,
                                    coverage.sample_start,
                                    coverage.sample_end,
                                    range.sample_start,
                                    range.sample_end,
                                )
                        }))
                {
                    return None;
                }
                let context_complete = context.as_ref().is_none_or(|(padding, retained)| {
                    let start = ranges
                        .iter()
                        .map(|range| range.sample_start)
                        .min()
                        .expect("nonempty source ranges");
                    let end = ranges
                        .iter()
                        .map(|range| range.sample_end)
                        .max()
                        .expect("nonempty source ranges");
                    let decode = (
                        start.saturating_sub(*padding).max(retained.start),
                        end.saturating_add(*padding).min(retained.end),
                    );
                    start >= retained.start
                        && end <= retained.end
                        && coverage.map_or_else(
                            || !evidence.has_decode(&c.owner, Some(decode)),
                            |range| range.sample_start <= decode.0 && range.sample_end >= decode.1,
                        )
                });
                context_complete.then_some((index, sources))
            })
            .max_by_key(|(index, _)| {
                self.word_adjudication.components[*index]
                    .whisper
                    .iter()
                    .map(|h| h.q)
                    .max()
                    .unwrap_or(0)
            })?;
        let index = self.group_word_trial_component(index, &sources);
        self.open_word_trial(index, fresh_decode)
    }
}
