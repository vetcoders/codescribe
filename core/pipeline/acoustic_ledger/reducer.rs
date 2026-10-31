//! Hypothesis reduction: word-slot admission, observation decisions,
//! overlap classification and refusals.
//!
//! Apple and Whisper hypotheses meet here. Every observation receives exactly
//! one [`MutationReceipt`]; the reducer fuses, replaces or refuses word slots
//! by PCM ownership, never by string equality.

use super::*;

impl AcousticLedger {
    /// Candidates retain their exact source slots and source observations.
    pub fn slot_alternatives(&self) -> &[SlotAlternative] {
        &self.slot_alternatives
    }

    pub(crate) fn retain_slot_alternative(
        &mut self,
        observation: &ObservationIdentity,
        candidate: &str,
        sources: Vec<WordSlot>,
        reason: &'static str,
    ) {
        let alternative = SlotAlternative {
            observation: observation.clone(),
            candidate: candidate.to_string(),
            sources,
            reason,
        };
        if !self.slot_alternatives.contains(&alternative) {
            self.slot_alternatives.push(alternative);
        }
    }

    /// A refused word retains its own PCM, label, producer and generation.
    /// The owner-wide receipt must never stand in for this recovery evidence.
    pub(super) fn refuse_word_slot(
        &mut self,
        observation: &ObservationIdentity,
        word: &WordSlot,
        sources: Vec<WordSlot>,
        reason: &'static str,
    ) {
        self.retain_slot_alternative(observation, &word.text, sources, reason);
        self.record_word_slot_refusal(observation, word, true);
    }

    /// Retain the newest rejected evidence per producer and exact physical pin.
    pub(super) fn retain_rejected_word_pin(
        &mut self,
        observation: &ObservationIdentity,
        word: &WordSlot,
    ) {
        let rejected_pins = self
            .rejected_word_pins
            .entry(observation.occurrence.clone())
            .or_default();
        if let Some(held) = rejected_pins.iter_mut().find(|pin| {
            pin.sample_start == word.sample_start
                && pin.sample_end == word.sample_end
                && pin.producer == word.producer
                && pin.observation.occurrence == word.observation.occurrence
        }) {
            if word.observation.generation > held.observation.generation {
                *held = word.clone();
            }
        } else {
            rejected_pins.push(word.clone());
        }
    }

    /// Carry a decoder-edge candidate into the existing rejected-pin accounting
    /// before live admission or at terminal drain. Its window is provenance,
    /// not success, and the original pin need not have a midpoint owner.
    pub(crate) fn retain_rejected_decode_word(
        &mut self,
        observation: &ObservationIdentity,
        pin: &WordPin,
    ) {
        let owner = &observation.occurrence;
        let (Some(start), Some(end)) = (pin.decode_sample_start, pin.decode_sample_end) else {
            return;
        };
        if observation.producer != ObservationProducer::Whisper
            || !self.is_qualified(owner)
            || self.is_sealed(owner)
            || start >= end
            || pin.sample_start < start
            || pin.sample_start >= pin.sample_end
            || pin.sample_end > end
        {
            return;
        }
        let word = WordSlot {
            sample_start: pin.sample_start,
            sample_end: pin.sample_end,
            text: pin.text.clone(),
            producer: observation.producer,
            observation: observation.clone(),
            witness: SlotWitness::Unwitnessed,
            confidence: pin.confidence,
            surface_rewritten: pin.surface_rewritten,
        };
        self.decoded_word_windows
            .insert(observation.clone(), (start, end));
        self.complete_decoded_words
            .insert(observation.clone(), Vec::new());
        self.refuse_word_slot(
            observation,
            &word,
            vec![word.clone()],
            "decode_window_clipped",
        );
    }

    /// Child receipts account for rejected PCM without replacing the offered
    /// batch identity on the recoverable candidate.
    pub(super) fn record_word_slot_refusal(
        &mut self,
        observation: &ObservationIdentity,
        word: &WordSlot,
        unresolved_speech: bool,
    ) {
        let owner = &observation.occurrence;
        // A handed-off layer can leave provenance, but cannot reopen speech
        // already held by a stronger source. Account for exact source ranges,
        // including a merge's gaps; an unrelated protected neighbour is not
        // authority over the entire offered pin.
        let holds_speech = |source: &WordSlot| {
            source.producer == ObservationProducer::ManualHuman
                || self.complete_word_source(source).is_some_and(|proof| {
                    proof.producer.authority_rank() > observation.producer.authority_rank()
                        || (proof.producer == observation.producer
                            && proof.observation.generation >= observation.generation)
                })
        };
        let mut protected = self
            .slots_of(owner)
            .unwrap_or(&[])
            .iter()
            .filter(|source| holds_speech(source))
            .flat_map(|source| self.slot_source_ranges(source))
            .filter(|range| range.same_capture(owner))
            .collect::<Vec<_>>();
        protected.sort_by_key(|range| (range.sample_start, range.sample_end));
        let mut cursor = word.sample_start;
        for range in protected {
            if range.sample_start <= cursor && range.sample_end > cursor {
                cursor = range.sample_end;
            }
        }
        let physical_target_held = self
            .slots_of(owner)
            .unwrap_or(&[])
            .iter()
            .filter(|source| holds_speech(source))
            .any(|source| {
                self.word_pin_observations.contains(&source.observation)
                    && source.text.split_whitespace().count() == 1
                    && word.text.split_whitespace().count() == 1
                    && self.pin_targets_source(source, word)
            });
        if unresolved_speech
            && !physical_target_held
            && cursor < word.sample_end
            && matches!(
                observation.producer,
                ObservationProducer::Apple
                    | ObservationProducer::Whisper
                    | ObservationProducer::CloudLive
                    | ObservationProducer::ManualHuman
            )
        {
            self.retain_rejected_word_pin(observation, word);
            self.require_text_recovery(owner);
        }
        let pin = OccurrenceIdentity::new(
            &owner.session,
            owner.capture_epoch,
            word.sample_start,
            word.sample_end,
        );
        let mut rejected = self.next_word_observation(word.producer, observation.request, &pin);
        // A whole-owner word can have exactly the batch's identity. Its child
        // receipt must not answer the parent before the batch is committed.
        if rejected == *observation {
            rejected.generation = rejected
                .generation
                .checked_add(1)
                .expect("word generation exhausted");
        }
        // D3, Maciej 2026-10-21: "zbierzmy całość hipotez" — a word the batch
        // could not place is still one engine's hypothesis on that PCM range.
        // Keep it, keyed by its own range, beside the words held there. It
        // changes no text and settles or blocks no debt. A batch that already
        // kept its reading (whole, as one candidate for the picker) has
        // collected this word with it; it is not collected twice.
        let already_kept = self
            .slot_alternatives
            .iter()
            .any(|alternative| alternative.observation == *observation);
        if !already_kept && !word.text.trim().is_empty() {
            let beside = self
                .slots_of(owner)
                .unwrap_or(&[])
                .iter()
                .filter(|held| {
                    held.sample_start < word.sample_end && word.sample_start < held.sample_end
                })
                .cloned()
                .collect::<Vec<_>>();
            self.retain_slot_alternative(&rejected, &word.text, beside, COLLECTED_HYPOTHESIS);
        }
        self.refuse_replacement(&rejected, &word.text, RefuseReason::SlotAdmissionRejected);
    }

    pub(in crate::pipeline) fn assigned_word_pin_ranges(
        &self,
        observation: &ObservationIdentity,
    ) -> Vec<(OccurrenceIdentity, OccurrenceIdentity)> {
        self.assigned_word_pins
            .as_ref()
            .filter(|batch| batch.observation == *observation)
            .map(|batch| batch.assignments.clone())
            .unwrap_or_default()
    }

    /// Read-only word evidence for one occurrence.
    pub fn slots_of(&self, occurrence: &OccurrenceIdentity) -> Option<&[WordSlot]> {
        self.committed
            .get(occurrence)
            .map(|held| held.slots.as_slice())
    }

    /// Committed geometry supplied as word pins, excluding whole-label ranges.
    /// This is evidence for presentation coverage, never an admission rule.
    pub fn committed_word_pin_ranges(&self, occurrence: &OccurrenceIdentity) -> Vec<(u64, u64)> {
        self.slots_of(occurrence)
            .unwrap_or(&[])
            .iter()
            .filter(|slot| self.word_pin_observations.contains(&slot.observation))
            .map(|slot| (slot.sample_start, slot.sample_end))
            .collect()
    }

    /// Admit an acoustic label with its exact word ranges in the same decision.
    /// Pins must compose the offered label before they can authorize overlap.
    /// Invalid Apple timing uses ordinary whole-label admission.
    pub(crate) fn admit_pinned_label(
        &mut self,
        observation: &ObservationIdentity,
        label: &str,
        words: &[WordPin],
    ) -> MutationReceipt {
        let occurrence = &observation.occurrence;
        // A label without word targets follows label admission. It cannot
        // mint measured word geometry or complete a source-wide recovery.
        if observation.producer != ObservationProducer::Apple
            || self.is_sealed(occurrence)
            || words.is_empty()
        {
            return self.admit(observation, label);
        }
        let mut slots = Vec::with_capacity(words.len());
        for pin in words {
            let midpoint = pin
                .sample_start
                .saturating_add(pin.sample_end.saturating_sub(pin.sample_start) / 2);
            if pin.sample_end <= pin.sample_start
                || midpoint < occurrence.sample_start
                || midpoint >= occurrence.sample_end
            {
                return self.admit(observation, label);
            }
            slots.push(WordSlot {
                sample_start: pin.sample_start.max(occurrence.sample_start),
                sample_end: pin.sample_end.min(occurrence.sample_end),
                text: pin.text.clone(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                confidence: pin.confidence,
                surface_rewritten: pin.surface_rewritten,
            });
        }
        slots.sort_by(|a, b| {
            (a.sample_start, a.sample_end, &a.text).cmp(&(b.sample_start, b.sample_end, &b.text))
        });
        if slots
            .windows(2)
            .any(|pair| pair[0].sample_end > pair[1].sample_start)
            || compose_label(&slots) != label
        {
            return self.admit(observation, label);
        }
        if self.committed.contains_key(occurrence) {
            return self.admit_word_slots(observation, words);
        }
        self.admit_word_slots(observation, words)
    }

    /// Allocate a new generation of this occurrence's producer, independently
    /// of provider completion order. The producer's first request stays stable.
    pub(crate) fn next_word_observation(
        &self,
        producer: ObservationProducer,
        request: u64,
        occurrence: &OccurrenceIdentity,
    ) -> ObservationIdentity {
        let history = self
            .owner_history
            .get(occurrence)
            .and_then(|owner| owner.producers.get(&producer));
        let prior = history.and_then(|history| history.latest_answered.as_ref());
        // Successful non-mutating work reserves its generation without
        // becoming an answered lexical observation or changing its request.
        let generation = history
            .and_then(|history| history.highest_generation)
            .map_or(0, |generation| {
                generation
                    .checked_add(1)
                    .expect("word generation exhausted")
            });
        ObservationIdentity::new(
            producer,
            prior.map_or(request, |(_, request)| *request),
            generation,
            occurrence.clone(),
        )
    }

    /// Membership is unique; every offered answer still counts, including
    /// duplicate refusals. Equal-generation arrivals retain the last request.
    pub(super) fn note_answered(&mut self, observation: &ObservationIdentity) {
        self.answered.insert(observation.clone());
        let owner = self
            .owner_history
            .entry(observation.occurrence.clone())
            .or_default();
        owner.answered += 1;
        let history = owner.producers.entry(observation.producer).or_default();
        if history
            .latest_answered
            .as_ref()
            .is_none_or(|(generation, _)| *generation <= observation.generation)
        {
            history.latest_answered = Some((observation.generation, observation.request));
        }
        Self::reserve_generation(history, observation.generation);
    }

    pub(super) fn reserve_generation(history: &mut ProducerHistory, generation: u64) {
        history.highest_generation = Some(
            history
                .highest_generation
                .map_or(generation, |held| held.max(generation)),
        );
    }

    /// A replay requires one PCM slot and equal normalized text. An occurrence
    /// label is not evidence that every pin in its range was heard.
    ///
    /// `whisper_only` stays Whisper. A cloud-live slot is heard evidence, but
    /// counting it here would classify a later Whisper pin as an already-heard
    /// replay and drop the replacement. Whisper must still replace CloudLive.
    pub(crate) fn matching_word_slot(
        &self,
        owner: &OccurrenceIdentity,
        pin: &OccurrenceIdentity,
        text: &str,
        whisper_only: bool,
    ) -> bool {
        self.slots_of(owner).is_some_and(|slots| {
            slots.iter().any(|slot| {
                (!whisper_only || slot.producer == ObservationProducer::Whisper)
                    && !normalize_word_token(text).is_empty()
                    && normalize_word_token(text) == normalize_word_token(&slot.text)
                    && self.word_slot_targets_pin(slot, pin)
            })
        })
    }

    /// Cross-crate test door to the real word-slot admission, for the app
    /// crate's presentation tests. Production callers stay inside this crate.
    #[cfg(any(test, feature = "test-isolation"))]
    pub fn admit_word_slots_for_tests(
        &mut self,
        observation: &ObservationIdentity,
        words: &[WordPin],
    ) -> MutationReceipt {
        self.admit_word_slots(observation, words)
    }

    /// Merge word evidence on its owner. Heard span is the union of pin ranges,
    /// never a coverage gate. Apple and Whisper choices use accumulated
    /// local evidence; other producer permissions retain their own rules. Cloud-live replaces Apple
    /// and lexicon and fills only ranges Whisper does not hold; Apple fills
    /// only ranges neither cloud-live nor Whisper holds. Finality fences stay
    /// in the same admission path used by whole-label producer generations.
    pub(crate) fn admit_word_slots(
        &mut self,
        observation: &ObservationIdentity,
        words: &[WordPin],
    ) -> MutationReceipt {
        self.prepare_word_evidence(observation, words);
        let measured_anchors = self.measured_word_anchors(&observation.occurrence);
        let anchors = measured_anchors.clone().unwrap_or_default();
        let original_words = words;
        let trace = crate::pipeline::trail::SlotTrace::words(
            self,
            observation,
            words,
            self.capture_rate_hz,
        );
        if observation.producer == ObservationProducer::AppleDictation
            && words.is_empty()
            && let Some(receipt) = self.record_witness_absence(observation)
        {
            return trace.finish(receipt, self);
        }
        let owner = &observation.occurrence;
        if self.is_sealed(owner) && observation.producer != ObservationProducer::ManualHuman {
            let reason = match observation.producer {
                ObservationProducer::Whisper => NoAuthorityReason::LateWhisperWordSealedOwner,
                ObservationProducer::CloudLive => NoAuthorityReason::LateCloudLiveWordSealedOwner,
                _ => NoAuthorityReason::LateAppleWordSealedOwner,
            };
            // The reducer keys evidence by occurrence. Carry forward the last
            // K5 receipt so another late window cannot erase earlier evidence.
            let prior = self.trail.iter().rev().find_map(|entry| {
                if &entry.observation.occurrence != owner {
                    return None;
                }
                match &entry.decision {
                    MutationReceipt::KeepVisibleUnanchored {
                        label,
                        reason:
                            NoAuthorityReason::LateWhisperWordSealedOwner
                            | NoAuthorityReason::LateCloudLiveWordSealedOwner
                            | NoAuthorityReason::LateAppleWordSealedOwner,
                        ..
                    } => Some(label.clone()),
                    _ => None,
                }
            });
            let mut labels = prior.into_iter().collect::<Vec<_>>();
            labels.extend(words.iter().map(|pin| pin.text.clone()));
            return trace.finish(
                self.keep_visible_unanchored(observation, &labels.join(" "), reason),
                self,
            );
        }
        // Replay is fenced before alignment can emit a retention operation.
        if self.answered.contains(observation) {
            let candidate = words
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return trace.finish(
                self.refuse_replacement(observation, &candidate, RefuseReason::BatchDuplicate),
                self,
            );
        }
        let closed_words = self.defer_edge_cut_words(observation, words, &anchors);
        let offered_tokens = words
            .iter()
            .map(|pin| pin.text.split_whitespace().count())
            .sum::<usize>();
        let closed_tokens = closed_words
            .iter()
            .map(|pin| pin.text.split_whitespace().count())
            .sum::<usize>();
        tracing::info!(
            target: "codescribe::acoustic_identity",
            producer = ?observation.producer,
            session = %observation.occurrence.session,
            capture_epoch = observation.occurrence.capture_epoch,
            request = observation.request,
            offered_tokens,
            edge_deferred_tokens = offered_tokens.saturating_sub(closed_tokens),
            measured_anchors = anchors.len(),
            "acoustic_identity_input"
        );
        let words = closed_words.as_slice();
        if offered_tokens > 0 && closed_tokens == 0 {
            // Preserve the full candidate in deferred evidence, not in a
            // delivery receipt that could repaint an edge-cut word as text.
            let candidate = original_words
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return trace.finish(
                self.refuse_replacement(
                    observation,
                    &candidate,
                    RefuseReason::SlotAdmissionRejected,
                ),
                self,
            );
        }
        if self.word_trial_input_refusal(observation, words).is_some() {
            return trace.finish(
                self.refuse_replacement(observation, "", RefuseReason::SlotAdmissionRejected),
                self,
            );
        }
        let admission_trail_start = self.trail.len();
        let offered_words = words;
        // Declared decoder fences must contain the original PCM. Owner
        // clipping cannot turn a contradictory declaration into evidence.
        let mut admissible_words = Vec::new();
        let mut declared_bounds_refused = false;
        for pin in offered_words {
            if observation.producer != ObservationProducer::ManualHuman
                && let (Some(start), Some(end)) = (pin.decode_sample_start, pin.decode_sample_end)
                && (pin.sample_start >= pin.sample_end
                    || start >= end
                    || start > pin.sample_start
                    || pin.sample_end > end)
            {
                declared_bounds_refused = true;
                let sources = self
                    .slots_of(owner)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|source| {
                        pin.sample_start < pin.sample_end
                            && self.slot_source_ranges(source).iter().any(|range| {
                                range.same_capture(owner)
                                    && range.sample_start < pin.sample_end
                                    && pin.sample_start < range.sample_end
                            })
                    })
                    .cloned()
                    .collect();
                self.retain_slot_alternative(
                    observation,
                    &pin.text,
                    sources,
                    "contradictory_decode_bounds",
                );
                let target = OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    pin.sample_start,
                    pin.sample_end,
                );
                let mut rejected =
                    self.next_word_observation(observation.producer, observation.request, &target);
                // Even a whole-owner refusal is a child, not the batch answer.
                // Use the supplied coordinates only for diagnostics; an empty
                // or reversed pin cannot enter measured-word geometry or debt.
                if rejected == *observation {
                    rejected.generation = rejected
                        .generation
                        .checked_add(1)
                        .expect("word generation exhausted");
                }
                self.refuse_replacement(&rejected, &pin.text, RefuseReason::SlotAdmissionRejected);
                continue;
            }
            admissible_words.push(pin.clone());
        }
        let words = admissible_words.as_slice();
        // Routing may have proposed this owner's pins before admission. Keep
        // only admissible declarations; committed neighbours retain their own
        // source authentication in the coverage owner.
        if declared_bounds_refused
            && let Some(batch) = self.assigned_word_pins.as_mut()
            && batch.observation == *observation
        {
            batch.assignments.retain(|(pin_owner, pin)| {
                words.iter().any(|word| {
                    pin.same_capture(owner)
                        && pin.sample_start == word.sample_start
                        && pin.sample_end == word.sample_end
                }) || (pin_owner != owner
                    && !offered_words.iter().any(|word| {
                        pin.same_capture(owner)
                            && pin.sample_start == word.sample_start
                            && pin.sample_end == word.sample_end
                    }))
            });
        }
        if declared_bounds_refused && words.is_empty() {
            let candidate = offered_words
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return trace.finish(
                self.refuse_replacement(
                    observation,
                    &candidate,
                    RefuseReason::SlotAdmissionRejected,
                ),
                self,
            );
        }
        let declared_slots = || {
            words
                .iter()
                .filter(|pin| !pin.text.trim().is_empty())
                .map(|pin| {
                    (
                        WordSlot {
                            sample_start: pin.sample_start.max(owner.sample_start),
                            sample_end: pin.sample_end.min(owner.sample_end),
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
                .collect::<Vec<_>>()
        };
        // Cloud words carry publication-band rights in their original decode
        // coordinates. Borrowing their geometry would manufacture context for
        // a later local hypothesis before the band gate can assess it.
        let cloud_context = observation.producer == ObservationProducer::CloudLive
            || self.slots_of(owner).is_some_and(|slots| {
                slots
                    .iter()
                    .any(|slot| slot.producer == ObservationProducer::CloudLive)
            });
        let mut sequence_binding = None;
        let mut incoming = if observation.producer == ObservationProducer::ManualHuman
            || cloud_context
        {
            declared_slots()
        } else if measured_anchors.is_none()
            || words
                .iter()
                .any(|pin| pin.text.split_whitespace().count() > 1)
        {
            // No speech measurement covers this capture, or the producer
            // declared phrase grain: a phrase is one coarse source that
            // measured word pins refine later, never split by token here.
            // Declared ranges are the only word geometry until then;
            // nothing is withheld for lack of a measurement.
            tracing::info!(
                target: "codescribe::acoustic_identity",
                producer = ?observation.producer,
                session = %owner.session,
                capture_epoch = owner.capture_epoch,
                request = observation.request,
                declared_tokens = closed_tokens,
                "acoustic_identity_declared_geometry"
            );
            if let Some(projected) = self.declared_sequence_geometry(observation, words) {
                sequence_binding = Some((
                    self.slots_of(owner).unwrap_or(&[]).to_vec(),
                    projected
                        .iter()
                        .map(|(slot, _)| slot.clone())
                        .collect::<Vec<_>>(),
                ));
                projected
            } else if let Some(aligned) = self.declared_sequence_alignment(observation, words) {
                aligned
            } else {
                declared_slots()
            }
        } else if let Some(incoming) = self.normalize_anchor_words(observation, words, &anchors) {
            tracing::info!(
                target: "codescribe::acoustic_identity",
                producer = ?observation.producer,
                session = %owner.session,
                capture_epoch = owner.capture_epoch,
                request = observation.request,
                normalized_tokens = incoming.len(),
                unanchored_tokens = closed_tokens.saturating_sub(incoming.len()),
                "acoustic_identity_normalization"
            );
            incoming
        } else {
            tracing::info!(
                target: "codescribe::acoustic_identity",
                producer = ?observation.producer,
                session = %owner.session,
                capture_epoch = owner.capture_epoch,
                request = observation.request,
                normalized_tokens = 0_usize,
                unanchored_tokens = closed_tokens,
                "acoustic_identity_normalization"
            );
            let candidate = original_words
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let sources = self.slots_of(owner).unwrap_or(&[]).to_vec();
            self.retain_slot_alternative(
                observation,
                &candidate,
                sources,
                "no_measured_speech_under_declaration",
            );
            return trace.finish(
                self.keep_visible_unanchored(observation, &candidate, NoAuthorityReason::NoRange),
                self,
            );
        };
        // Reconcile while live, before resegmentation can treat a drifted
        // copy as a new word. The same rule applies to every word of a
        // re-emitted clause, not merely adjacent equal strings.
        let known = self
            .committed
            .values()
            .flat_map(|held| held.slots.iter().cloned())
            .collect::<Vec<_>>();
        let mut voice_replays = Vec::new();
        incoming.retain(|(word, _)| {
            if let Some(source) = self.acoustic_replay_source(word, &known) {
                voice_replays.push((word.clone(), source));
                false
            } else {
                true
            }
        });
        for (word, source) in &voice_replays {
            // The predecessor is a committed source from a different
            // overlapping decode, never a provisional within-batch token.
            if self
                .slots_of(&source.observation.occurrence)
                .is_some_and(|slots| slots.contains(source))
            {
                self.record_slot_operation(SlotOperationReceipt {
                    observation: observation.clone(),
                    kind: SlotOperationKind::Correct,
                    sources: vec![source.clone()],
                    outputs: vec![source.clone()],
                    source_ranges: self.slot_source_ranges(source),
                    rule_id: "overlapping_decode_same_voiced_burst/v1".into(),
                });
            }
            if !incoming.is_empty() {
                let pin = OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    word.sample_start,
                    word.sample_end,
                );
                let mut replay =
                    self.next_word_observation(observation.producer, observation.request, &pin);
                if replay == *observation {
                    replay.generation = replay.generation.saturating_add(1);
                }
                self.refuse_replayed_range(&replay, &word.text);
            }
        }
        if !voice_replays.is_empty() && incoming.is_empty() {
            // A window that only repeats held words still heard its whole
            // decode range, so its return pays the text debt it covered
            // (decision of Maciej, 2026-10-21) like any complete return. A
            // second delivery of the same observation stopped at `answered`.
            self.record_word_decode_bounds(observation, words);
            let candidate = original_words
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return trace.finish(self.refuse_replayed_range(observation, &candidate), self);
        }
        let mut no_speech_refused = Vec::new();
        incoming.retain(|(word, pin)| {
            if observation.producer != ObservationProducer::ManualHuman
                && self.word_deletions.iter().any(|deletion| {
                    let range = deletion.verdict.target();
                    range.same_capture(owner)
                        && word.sample_start >= range.sample_start
                        && word.sample_end <= range.sample_end
                })
            {
                no_speech_refused.push((*pin).clone());
                return false;
            }
            true
        });
        if incoming.is_empty() && !no_speech_refused.is_empty() {
            let candidate = no_speech_refused
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return trace.finish(
                self.refuse_replacement(observation, &candidate, RefuseReason::ConfirmedNoSpeech),
                self,
            );
        }
        let assignments = words
            .iter()
            .filter(|pin| pin.sample_start < pin.sample_end && !pin.text.trim().is_empty())
            .map(|pin| {
                (
                    owner.clone(),
                    OccurrenceIdentity::new(
                        &owner.session,
                        owner.capture_epoch,
                        pin.sample_start,
                        pin.sample_end,
                    ),
                )
            })
            .collect::<Vec<_>>();
        if let Some(batch) = self.assigned_word_pins.as_mut()
            && batch.observation == *observation
        {
            for assignment in assignments {
                if !batch.assignments.contains(&assignment) {
                    batch.assignments.push(assignment);
                }
            }
        } else {
            self.record_assigned_word_pins(observation, &assignments);
        }
        self.record_word_decode_bounds(observation, words);
        self.project_anchor_decode_bounds(observation, &incoming);
        let recovery_pending = self.text_recovery_pending(owner);
        let mut slots = self.slots_of(owner).unwrap_or(&[]).to_vec();
        // Source completeness is checked at the resolved group/word boundary
        // below, so a rejected partition retains one complete alternative.
        // ClockLie constrains replacements, never new words over empty audio.
        // Filter before geometric operations so blocked sources stay held.
        if observation.producer != ObservationProducer::ManualHuman {
            incoming.retain(|(word, _)| {
                let sources = slots
                    .iter()
                    .filter(|source| {
                        source.sample_start < word.sample_end
                            && word.sample_start < source.sample_end
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let human_sources = sources
                    .iter()
                    .filter(|source| source.producer == ObservationProducer::ManualHuman)
                    .cloned()
                    .collect::<Vec<_>>();
                if !human_sources.is_empty() {
                    self.refuse_word_slot(observation, word, human_sources, "protected_source");
                    return false;
                }
                let Some(blocker) = self.replace_neighbour(owner, &sources) else {
                    return true;
                };
                let mut rejected = observation.clone();
                rejected.occurrence = OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    word.sample_start,
                    word.sample_end,
                );
                self.retain_slot_alternative(observation, &word.text, sources, "clock_lie");
                self.retain_rejected_word_pin(observation, word);
                self.offered_observations += 1;
                self.note_answered(&rejected);
                let decision = MutationReceipt::Refuse {
                    occurrence: rejected.occurrence.clone(),
                    reason: RefuseReason::ClockLie,
                };
                self.record_layer_decision(&rejected, &word.text, &decision, Some(blocker));
                false
            });
        }
        // Resegmentation runs before ordinary per-word correction and may consume a
        // one-to-one window refinement. Enforce the CloudLive -> Whisper
        // context boundary here so an incomplete exact source cannot bypass
        // the direct-correction guard by entering that earlier path. Keep each
        // original pin paired with its projection: owner clipping cannot
        // improve the decoder's measured context quality.
        if observation.producer == ObservationProducer::Whisper {
            incoming.retain(|(word, pin)| {
                let direct_sources = slots
                    .iter()
                    .filter(|source| {
                        source.producer == ObservationProducer::CloudLive
                            && !self.coarse_word_source(source)
                            && ((source.sample_start == word.sample_start
                                && source.sample_end == word.sample_end)
                                || self.pin_targets_source(source, word))
                    })
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>();
                let [source] = direct_sources.as_slice() else {
                    return true;
                };
                if word_adjudication::label_equal(&source.text, &word.text)
                    || self.whisper_has_correction_context(
                        observation,
                        pin.sample_start,
                        pin.sample_end,
                    )
                {
                    return true;
                }
                self.retain_slot_alternative(
                    observation,
                    &word.text,
                    vec![source.clone()],
                    "outside_publication_band",
                );
                self.record_word_slot_refusal(observation, word, false);
                false
            });
        }
        let (mut incoming, original_pins): (Vec<_>, Vec<_>) = incoming.into_iter().unzip();
        let (mut resegments, proposed_coverages) =
            self.resegment_word_slots(observation, &mut slots, &mut incoming, &original_pins);
        let mut removed = resegments
            .iter()
            .flat_map(|op| op.sources.clone())
            .collect::<Vec<_>>();
        let mut group_rules = Vec::new();
        // Match the complete batch before editing. Equal tokens can require
        // a repetition picker; distinct pins require a measured partition.
        let prior = slots.clone();
        let mut replayed_words = 0_usize;
        let mut replayed_pins = Vec::new();
        for mut word in incoming.iter().cloned() {
            let ordinary_target = self.ordinary_word_target(&prior, &incoming, &word);
            let conflicts = prior
                .iter()
                .enumerate()
                .filter_map(|(index, held)| self.pin_targets_source(held, &word).then_some(index))
                .collect::<Vec<_>>();
            if let [index] = conflicts.as_slice()
                && (self.same_anchor_assignment(&prior[*index], &word)
                    || (word.producer == ObservationProducer::Apple
                        && self.unmeasured_pair(&prior[*index], &word)
                        && prior[*index].sample_start == word.sample_start
                        && prior[*index].sample_end == word.sample_end))
                && prior[*index].producer == word.producer
                && normalize_word_token(&prior[*index].text) == normalize_word_token(&word.text)
                // Agreement may still upgrade an incomplete phrase witness to
                // a complete measured word. Keeping the old Apple source here
                // would strand finality despite the successful Whisper return.
                && (!self.complete_word_slot(&word) || self.complete_word_slot(&prior[*index]))
            {
                let source = &prior[*index];
                self.adjudicate_word_sources(
                    observation,
                    std::slice::from_ref(source),
                    std::slice::from_ref(&word),
                );
                let mut source_ranges = self.slot_source_ranges(source);
                if let Some(original) = self.original_anchor_word_range(&word).cloned()
                    && !source_ranges.contains(&original)
                {
                    source_ranges.push(original);
                }
                self.record_slot_operation(SlotOperationReceipt {
                    observation: observation.clone(),
                    kind: SlotOperationKind::Correct,
                    sources: vec![source.clone()],
                    outputs: vec![source.clone()],
                    source_ranges,
                    rule_id: "speech_anchor_agreement/v2".into(),
                });
                replayed_words += 1;
                replayed_pins.push(word.clone());
                continue;
            }
            let intersects = prior
                .iter()
                .filter(|held| {
                    held.sample_start < word.sample_end && word.sample_start < held.sample_end
                })
                .collect::<Vec<_>>();
            let ambiguous = self.repetition_target_ambiguous(
                &intersects
                    .iter()
                    .map(|source| (*source).clone())
                    .collect::<Vec<_>>(),
                &incoming
                    .iter()
                    .filter(|pin| {
                        normalize_word_token(&pin.text) == normalize_word_token(&word.text)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            let partition = word.producer != ObservationProducer::ManualHuman
                && ordinary_target.is_none()
                && ((intersects.len() > 1 && conflicts.len() != 1)
                    || conflicts.iter().any(|&index| {
                        incoming
                            .iter()
                            .filter(|other| self.same_anchored_word(&prior[index], other))
                            .count()
                            > 1
                    })
                    || conflicts.iter().any(|&index| {
                        let source = &prior[index];
                        (source.sample_start != word.sample_start
                            || source.sample_end != word.sample_end)
                            && ordinary_target != Some(index)
                    })
                    || prior.iter().enumerate().any(|(index, source)| {
                        !conflicts.contains(&index)
                            && (self.same_anchored_word(source, &word)
                                || (self.unmeasured_pair(source, &word)
                                    && declared_range_contains_midpoint(
                                        word.sample_start,
                                        word.sample_end,
                                        source.sample_start,
                                        source.sample_end,
                                    )))
                    }));
            if !conflicts.is_empty()
                && matches!(
                    word.producer,
                    ObservationProducer::Apple
                        | ObservationProducer::Whisper
                        | ObservationProducer::CloudLive
                        | ObservationProducer::AppleDictation
                )
            {
                let sources = conflicts
                    .iter()
                    .map(|&index| prior[index].clone())
                    .collect::<Vec<_>>();
                if !self.asr_source_scope_complete(observation, &sources) {
                    if self
                        .adjudicate_word_sources(observation, &sources, std::slice::from_ref(&word))
                        .is_none()
                    {
                        self.record_word_choice(
                            observation,
                            &sources,
                            std::slice::from_ref(&word),
                            "incomplete_source_scope",
                            false,
                        );
                    }
                    self.retain_slot_alternative(
                        observation,
                        &word.text,
                        sources,
                        "incomplete_source_scope",
                    );
                    self.record_word_slot_refusal(observation, &word, false);
                    continue;
                }
            }
            let local_choice = if !ambiguous && !partition && !conflicts.is_empty() {
                let sources = conflicts
                    .iter()
                    .map(|&index| prior[index].clone())
                    .collect::<Vec<_>>();
                self.adjudicate_word_sources(observation, &sources, std::slice::from_ref(&word))
            } else {
                None
            };
            if local_choice == Some(true) {
                let sources = conflicts
                    .iter()
                    .map(|&index| prior[index].clone())
                    .collect::<Vec<_>>();
                self.stamp_word_witness(observation, &sources, std::slice::from_mut(&mut word));
            }
            if local_choice == Some(false) {
                self.retain_slot_alternative(
                    observation,
                    &word.text,
                    conflicts
                        .iter()
                        .map(|&index| prior[index].clone())
                        .collect(),
                    "word_adjudication_held",
                );
                self.record_word_slot_refusal(observation, &word, false);
                continue;
            }
            let blocked = (word.producer != ObservationProducer::ManualHuman
                && intersects
                    .iter()
                    .any(|source| source.producer == ObservationProducer::ManualHuman))
                || word.producer == ObservationProducer::Formatter
                || (conflicts.is_empty()
                    && !matches!(
                        word.producer,
                        ObservationProducer::Apple
                            | ObservationProducer::Whisper
                            | ObservationProducer::CloudLive
                            | ObservationProducer::AppleDictation
                            | ObservationProducer::ManualHuman
                    ))
                || conflicts.iter().any(|&index| {
                    let held = &prior[index];
                    held.producer == ObservationProducer::ManualHuman
                        && word.producer != ObservationProducer::ManualHuman
                        || (local_choice.is_none()
                            && (held.producer.authority_rank() > word.producer.authority_rank()
                                || (held.producer == word.producer
                                    && held.observation.generation >= word.observation.generation
                                    && held.observation != word.observation)))
                });
            if ambiguous || blocked || partition {
                self.refuse_word_slot(
                    observation,
                    &word,
                    intersects.into_iter().cloned().collect(),
                    if ambiguous {
                        "ambiguous_pcm_target"
                    } else if blocked {
                        "protected_source"
                    } else {
                        "word_partition_requires_coverage"
                    },
                );
                continue;
            }
            if let Some(&index) = conflicts.first() {
                let source = &prior[index];
                if word.producer != ObservationProducer::ManualHuman
                    && ordinary_target != Some(index)
                {
                    let candidate = word.text.clone();
                    let alignment = slot_ops::preserve_group_content(&source.text, &candidate);
                    let (label, retained) = match alignment {
                        Ok(alignment) => alignment,
                        Err(reason) => {
                            self.refuse_word_slot(observation, &word, vec![source.clone()], reason);
                            // The unchanged source is an explicit retention decision.
                            self.record_slot_operation(SlotOperationReceipt {
                                observation: observation.clone(),
                                kind: SlotOperationKind::Correct,
                                sources: vec![source.clone()],
                                outputs: vec![source.clone()],
                                source_ranges: self.slot_source_ranges(source),
                                rule_id: "group_alignment_ambiguous/held_token_retained/v1".into(),
                            });
                            continue;
                        }
                    };
                    if retained {
                        self.refuse_word_slot(
                            observation,
                            &word,
                            vec![source.clone()],
                            "held_token_retained",
                        );
                        self.record_slot_operation(SlotOperationReceipt {
                            observation: observation.clone(),
                            kind: SlotOperationKind::Correct,
                            sources: vec![source.clone()],
                            outputs: vec![source.clone()],
                            source_ranges: self.slot_source_ranges(source),
                            rule_id: "group_alignment/held_token_retained/v1".into(),
                        });
                        continue;
                    }
                    word.text = label;
                    group_rules.push((
                        source.sample_start,
                        source.sample_end,
                        "group_alignment/v1",
                    ));
                }
                // Keep the newly measured clock. The explicit correction retires
                // its predecessor while retaining that predecessor's exact ranges.
                let mut source_ranges = self.slot_source_ranges(source);
                let current_source = OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    source.sample_start,
                    source.sample_end,
                );
                if !source_ranges.contains(&current_source) {
                    source_ranges.push(current_source);
                }
                for (pin_owner, pin) in self.assigned_word_pin_ranges(observation) {
                    if pin_owner == *owner
                        && pin.same_capture(owner)
                        && pin.sample_start.max(owner.sample_start) == word.sample_start
                        && pin.sample_end.min(owner.sample_end) == word.sample_end
                        && !source_ranges.contains(&pin)
                    {
                        source_ranges.push(pin);
                    }
                }
                resegments.push(SlotOperationReceipt {
                    observation: observation.clone(),
                    kind: SlotOperationKind::Correct,
                    sources: vec![source.clone()],
                    outputs: vec![word.clone()],
                    source_ranges,
                    rule_id: "measured_word_target/v1".into(),
                });
                let position = slots
                    .iter()
                    .position(|slot| slot == source)
                    .expect("unique source retained until its correction");
                removed.push(slots[position].clone());
                slots[position] = word;
                slots.sort_by_key(|slot| (slot.sample_start, slot.sample_end));
            } else if incoming
                .iter()
                .filter(|other| self.same_anchored_word(other, &word))
                .count()
                > 1
            {
                self.refuse_word_slot(observation, &word, Vec::new(), "overlapping_new_pins");
            } else {
                // Missing a physical target does not prove another word.
                // A late Apple pin may intersect a complete Whisper Word
                // while its shifted clock misses the same-slot threshold.
                // Preserve that uncertainty as evidence and recovery debt;
                // neither text equality nor weaker overlap grants identity.
                if word.producer == ObservationProducer::Apple {
                    let sources = prior
                        .iter()
                        .filter(|source| {
                            source.producer == ObservationProducer::Whisper
                                && source.sample_start < word.sample_end
                                && word.sample_start < source.sample_end
                                && source.text.split_whitespace().count() == 1
                                && self.complete_word_slot(source)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if !sources.is_empty() {
                        self.record_word_choice(
                            observation,
                            &sources,
                            std::slice::from_ref(&word),
                            "late_apple_overlap_requires_partition",
                            false,
                        );
                        self.refuse_word_slot(
                            observation,
                            &word,
                            sources,
                            "late_apple_overlap_requires_partition",
                        );
                        continue;
                    }
                }
                // Decode completeness remains required even when no measured
                // anchor map was available to defer the original pin.
                if word.producer == ObservationProducer::Whisper
                    && self.new_word_decode_fence_incomplete(observation, &word)
                {
                    self.refuse_word_slot(
                        observation,
                        &word,
                        Vec::new(),
                        "incomplete_decode_fence_new_word",
                    );
                    continue;
                }
                // A new pin is not a new word if the same PCM edge already
                // carries that token, including a group or a neighbouring owner.
                let normalized = normalize_word_token(&word.text);
                let boundary_tolerance = self
                    .capture_rate_hz
                    .map_or(8_000, |rate| u64::from(rate) / 2);
                let duplicate_sources = slots
                    .iter()
                    .filter(|held| self.word_pin_observations.contains(&held.observation))
                    .filter(|held| edge_token_duplicate(self, held, &word, &normalized))
                    .cloned()
                    .chain(
                        self.committed
                            .iter()
                            .filter(|(other, _)| {
                                *other != owner
                                    && other.same_capture(owner)
                                    && ((other.sample_end <= owner.sample_start
                                        && owner.sample_start.saturating_sub(other.sample_end)
                                            <= boundary_tolerance)
                                        || (owner.sample_end <= other.sample_start
                                            && other.sample_start.saturating_sub(owner.sample_end)
                                                <= boundary_tolerance))
                            })
                            .flat_map(|(_, held)| held.slots.iter())
                            .filter(|held| self.word_pin_observations.contains(&held.observation))
                            .filter(|held| edge_token_duplicate(self, held, &word, &normalized))
                            .cloned(),
                    )
                    .collect::<Vec<_>>();
                if word.producer != ObservationProducer::ManualHuman
                    && !duplicate_sources.is_empty()
                {
                    self.retain_slot_alternative(
                        observation,
                        &word.text,
                        duplicate_sources,
                        "duplicate_pcm_edge_token",
                    );
                    self.record_word_slot_refusal(observation, &word, false);
                    continue;
                }
                let position = slots
                    .iter()
                    .position(|held| held.sample_start > word.sample_start)
                    .unwrap_or(slots.len());
                slots.insert(position, word);
            }
        }
        // A mixed window still owes a receipt for each replayed word. The
        // batch's mutation accounts for new words, not those rejected copies.
        if replayed_words < incoming.len() {
            for word in &replayed_pins {
                let target = OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    word.sample_start,
                    word.sample_end,
                );
                let mut replay =
                    self.next_word_observation(observation.producer, observation.request, &target);
                if replay == *observation {
                    replay.generation = replay
                        .generation
                        .checked_add(1)
                        .expect("word generation exhausted");
                }
                self.refuse_replayed_range(&replay, &word.text);
            }
        }
        slots.sort_by_key(|slot| (slot.sample_start, slot.sample_end));
        if removed.is_empty() && slots.as_slice() == self.slots_of(owner).unwrap_or(&[]) {
            if replayed_words == incoming.len() && replayed_words > 0 {
                // Refusing another document occurrence does not erase a
                // successful complete decode of the same physical words.
                // Its recorded PCM window can close coverage, while retained
                // cut/rejected sources still prevent debt settlement below.
                if self
                    .complete_decoded_words
                    .get(observation)
                    .is_some_and(|complete| {
                        incoming
                            .iter()
                            .all(|word| complete.contains(&(word.sample_start, word.sample_end)))
                    })
                {
                    self.word_pin_observations.insert(observation.clone());
                    self.reconcile_returned_word_debt(owner);
                }
                tracing::info!(
                    target: "codescribe::acoustic_identity",
                    producer = ?observation.producer,
                    session = %owner.session,
                    capture_epoch = owner.capture_epoch,
                    request = observation.request,
                    replayed_range_identity = replayed_words,
                    "acoustic_identity_replay"
                );
                let candidate = original_words
                    .iter()
                    .map(|pin| pin.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                return trace.finish(self.refuse_replayed_range(observation, &candidate), self);
            }
            if self.trail[admission_trail_start..]
                .iter()
                .any(|entry| matches!(entry.decision, MutationReceipt::Refuse { .. }))
            {
                let candidate = offered_words
                    .iter()
                    .map(|pin| pin.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let receipt = self.refuse_replacement(
                    observation,
                    &candidate,
                    RefuseReason::SlotAdmissionRejected,
                );
                return trace.finish(receipt, self);
            }
            let label = self.text_of(owner).unwrap_or("").to_string();
            let recovery_pending = self.text_recovery_pending(owner);
            let receipt = self.admit(observation, &label);
            // Echoing our retained label is not accepted recovery evidence.
            if recovery_pending {
                self.pending_text_recovery.insert(owner.clone());
            }
            for pin in no_speech_refused {
                let rejected =
                    self.next_word_observation(observation.producer, observation.request, owner);
                self.refuse_replacement(&rejected, &pin.text, RefuseReason::ConfirmedNoSpeech);
            }
            self.record_sequence_binding(observation, sequence_binding.as_ref(), original_words);
            return trace.finish(receipt, self);
        }
        let label = compose_label(&slots);
        let operation_start = self.slot_operations.len();
        let receipt = self.admit_slot_batch(observation, &label, Some(slots), true, &resegments);
        for operation in &mut self.slot_operations[operation_start..] {
            if let Some((_, _, rule_id)) = group_rules.iter().find(|(start, end, _)| {
                operation
                    .outputs
                    .iter()
                    .any(|output| output.sample_start == *start && output.sample_end == *end)
            }) {
                operation.rule_id = (*rule_id).into();
            }
        }
        if receipt.grants_mutation() || matches!(receipt, MutationReceipt::Preserve { .. }) {
            self.record_sequence_binding(observation, sequence_binding.as_ref(), original_words);
            self.record_anchor_normalizations(observation);
            self.settle_closed_edge_words(owner);
            let duplicate_anchor_words = self
                .slots_of(owner)
                .unwrap_or(&[])
                .windows(2)
                .filter(|pair| {
                    normalize_word_token(&pair[0].text) == normalize_word_token(&pair[1].text)
                        && self.same_anchored_word(&pair[0], &pair[1])
                })
                .count();
            tracing::info!(
                target: "codescribe::acoustic_identity",
                producer = ?observation.producer,
                session = %owner.session,
                capture_epoch = owner.capture_epoch,
                request = observation.request,
                duplicate_anchor_words,
                replayed_range_identity = replayed_words,
                "acoustic_identity_admission"
            );
            // Coverage describes committed operations only. A proposal that
            // loses owner admission cannot publish a successful partition.
            self.group_speech_coverages.extend(proposed_coverages);
            // A proposed partition cannot create owner debt before its exact
            // source retirement is committed. Only admitted partial operations
            // carry coarse-source obligations into the producer's final return.
            for operation in &resegments {
                if operation.rule_id == "acoustic_resegmentation/partial-speech/v1" {
                    self.require_text_recovery(owner);
                    for source in &operation.sources {
                        self.retain_rejected_word_pin(observation, source);
                    }
                }
            }
            self.reconcile_returned_word_debt(owner);
            let reason = match observation.producer {
                ObservationProducer::Whisper => Some(RefuseReason::ReplacedByWhisper),
                ObservationProducer::CloudLive => Some(RefuseReason::ReplacedByCloudLive),
                _ => None,
            };
            if let Some(reason) = reason {
                for slot in removed {
                    let loser = if slot.observation == *observation {
                        self.next_word_observation(slot.producer, observation.request, owner)
                    } else {
                        slot.observation
                    };
                    self.refuse_replacement(&loser, &slot.text, reason);
                }
            }
        }
        for pin in no_speech_refused {
            let rejected =
                self.next_word_observation(observation.producer, observation.request, owner);
            self.refuse_replacement(&rejected, &pin.text, RefuseReason::ConfirmedNoSpeech);
        }
        // A partial revision may improve the visible text without answering
        // the whole occurrence's recovery. Retained source labels and refused
        // pins cannot be promoted into a successful recovery by the batch.
        let recovery_unresolved = self.rejected_word_pins.contains_key(owner)
            || self.slots_of(owner).is_some_and(|slots| {
                slots.iter().any(|slot| {
                    slot.producer != ObservationProducer::ManualHuman
                        && slot.producer.authority_rank() < observation.producer.authority_rank()
                })
            });
        if recovery_unresolved
            && (recovery_pending || self.text_recovery_pending(owner))
            && !self.complete_decode_covers(owner)
        {
            self.pending_text_recovery.insert(owner.clone());
        }
        trace.finish(receipt, self)
    }

    #[cfg(test)]
    pub(crate) fn assert_slot_labels(&self) {
        for held in self.committed.values() {
            assert_eq!(held.label, compose_label(&held.slots));
        }
    }

    /// Mark an acoustically qualified occurrence whose provisional label does
    /// not account for its speech. The label stays visible, but cannot certify
    /// coverage or finality until an authorized recovery observation lands.
    /// A range that complete decode windows already heard owes nothing.
    pub fn require_text_recovery(&mut self, occurrence: &OccurrenceIdentity) -> bool {
        if !self.evidence.contains_key(occurrence)
            || self.seals.contains_key(occurrence)
            || self.complete_decode_covers(occurrence)
        {
            return false;
        }
        let newly_pending = self.pending_text_recovery.insert(occurrence.clone());
        if newly_pending && crate::pipeline::trail::is_enabled(occurrence) {
            crate::pipeline::trail::record(
                occurrence,
                crate::pipeline::trail::TrailEvent::RecoveryRequired {
                    recovery: self.recovery_diagnostic(occurrence),
                },
            );
        }
        true
    }

    /// Debt is open until complete decode windows cover its whole range.
    pub fn text_recovery_pending(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.publication_debt.get(occurrence).is_some_and(|ranges| {
            ranges.iter().any(|&(start, end)| {
                !self
                    .publication_range_debt(occurrence, start, end)
                    .is_empty()
            })
        }) || (self.pending_text_recovery.contains(occurrence)
            && !self.complete_decode_covers(occurrence))
    }

    pub fn pending_text_recoveries(
        &self,
        session: &str,
        capture_epoch: u64,
    ) -> Vec<OccurrenceIdentity> {
        self.pending_text_recovery
            .iter()
            .chain(self.publication_debt.keys())
            .filter(|occurrence| {
                occurrence.session == session
                    && occurrence.capture_epoch == capture_epoch
                    && self.text_recovery_pending(occurrence)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Offer one observation and receive exactly one receipt.
    ///
    /// `text` is what the producer heard. It is recorded, compared for
    /// equality, and never used to establish identity.
    ///
    /// Every call also appends exactly one [`LayerDecisionReceipt`], so the
    /// per-layer history can never fall behind the decisions it describes.
    pub fn admit(&mut self, observation: &ObservationIdentity, text: &str) -> MutationReceipt {
        self.admit_with_slots(observation, text, None, false)
    }

    pub(in crate::pipeline) fn admit_with_slots(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        slots: Option<Vec<WordSlot>>,
        slot_revision: bool,
    ) -> MutationReceipt {
        self.admit_slot_batch(observation, text, slots, slot_revision, &[])
    }

    /// All slot batches commit here, including explicit geometric operations.
    /// Where the instruments hear speech, a producer of lower authority does
    /// not retire a word a higher one holds, not even by retiming it: its
    /// proposal leaves the batch and stays an alternative on the held word,
    /// while the rest of the batch is still admitted. A held word with no
    /// speech range under it is not protected here; the lower producer may
    /// still falsify it through the acoustic coverage check. Returns the kept
    /// slots and operations, and the kept label when anything was withheld.
    pub(super) fn withhold_lower_authority_over_speech(
        &mut self,
        observation: &ObservationIdentity,
        slots: Option<Vec<WordSlot>>,
        operations: &[SlotOperationReceipt],
    ) -> (
        Option<Vec<WordSlot>>,
        Vec<SlotOperationReceipt>,
        Option<String>,
    ) {
        let owner = &observation.occurrence;
        let held = self.slots_of(owner).unwrap_or(&[]).to_vec();
        let protected = |source: &WordSlot| {
            observation.producer != ObservationProducer::ManualHuman
                && !operations.iter().any(|operation| {
                    operation.sources.contains(source)
                        && self
                            .independent_dt_partition_debt(
                                observation,
                                &operation.sources,
                                &operation.outputs,
                            )
                            .is_some()
                })
                && source.producer.authority_rank() > observation.producer.authority_rank()
                && !self.speech_evidence.as_ref().is_some_and(|speech| {
                    speech
                        .identity()
                        .matches(&owner.session, owner.capture_epoch)
                        && speech
                            .availability()
                            .observed_samples()
                            .is_some_and(|end| end >= source.sample_end)
                        && speech.ranges().iter().all(|range| {
                            range.sample_end <= source.sample_start
                                || range.sample_start >= source.sample_end
                        })
                })
        };
        let (withheld_operations, kept_operations): (Vec<_>, Vec<_>) = operations
            .iter()
            .cloned()
            .partition(|operation| operation.sources.iter().any(&protected));
        let mut withheld_sources = withheld_operations
            .iter()
            .flat_map(|operation| operation.sources.iter().cloned())
            .collect::<Vec<_>>();
        let mut withheld_outputs = withheld_operations
            .iter()
            .flat_map(|operation| operation.outputs.iter().cloned())
            .collect::<Vec<_>>();
        let Some(proposed) = slots else {
            return (None, kept_operations, None);
        };
        let mut kept = Vec::with_capacity(proposed.len());
        for output in proposed {
            if withheld_outputs.contains(&output) {
                continue;
            }
            if kept_operations
                .iter()
                .any(|operation| operation.outputs.contains(&output))
            {
                kept.push(output);
                continue;
            }
            match held
                .iter()
                .find(|source| **source != output && self.same_anchored_word(source, &output))
            {
                Some(source) if protected(source) => {
                    withheld_sources.push(source.clone());
                    withheld_outputs.push(output);
                }
                _ => kept.push(output),
            }
        }
        if withheld_outputs.is_empty() {
            return (Some(kept), kept_operations, None);
        }
        withheld_sources.sort_by_key(|word| (word.sample_start, word.sample_end));
        withheld_sources.dedup();
        withheld_outputs.sort_by_key(|word| (word.sample_start, word.sample_end));
        self.retain_slot_alternative(
            observation,
            &compose_label(&withheld_outputs),
            withheld_sources,
            "lower_authority_over_speech",
        );
        let label = compose_label(&kept);
        (Some(kept), kept_operations, Some(label))
    }

    /// D5 for part of a batch. When the batch cannot retire some held
    /// sources, it retires none: every operation, and every proposed word that
    /// replaces or overlaps a held word, is withheld and kept as one
    /// alternative over the blocked sources. A word over speech no held word
    /// labels still lands (field shape W27 of take fc40155d): appending it
    /// replaces nothing. `None` when the batch proposes no such word.
    pub(super) fn withhold_unproven_retirements(
        &mut self,
        observation: &ObservationIdentity,
        slots: Option<&[WordSlot]>,
        operations: &[SlotOperationReceipt],
        blocked: &[WordSlot],
    ) -> Option<(Vec<WordSlot>, String)> {
        let proposed = slots?;
        let held = self
            .slots_of(&observation.occurrence)
            .unwrap_or(&[])
            .to_vec();
        let gap_word = |word: &WordSlot| {
            !operations
                .iter()
                .any(|operation| operation.outputs.contains(word))
                && !held.iter().any(|source| {
                    source.sample_start < word.sample_end && word.sample_start < source.sample_end
                })
        };
        let (kept, withheld): (Vec<WordSlot>, Vec<WordSlot>) = proposed
            .iter()
            .cloned()
            .partition(|word| held.contains(word) || gap_word(word));
        if kept.iter().all(|word| held.contains(word)) {
            return None;
        }
        self.retain_slot_alternative(
            observation,
            &compose_label(&withheld),
            blocked.to_vec(),
            "absence_requires_acoustic_evidence",
        );
        let label = compose_label(&kept);
        Some((kept, label))
    }

    pub(super) fn admit_slot_batch(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        slots: Option<Vec<WordSlot>>,
        slot_revision: bool,
        operations: &[SlotOperationReceipt],
    ) -> MutationReceipt {
        self.trail_input = crate::pipeline::trail::is_enabled(&observation.occurrence).then(|| {
            crate::pipeline::trail::TrailAdmission {
                source_slots: self
                    .slots_of(&observation.occurrence)
                    .unwrap_or(&[])
                    .to_vec(),
                offered_slots: slots.clone(),
                slot_revision,
                capture_rate_hz: self.capture_rate_hz,
                non_mutating: None,
            }
        });
        let (slots, kept_operations, kept_label) =
            self.withhold_lower_authority_over_speech(observation, slots, operations);
        let operations = kept_operations.as_slice();
        if kept_label.is_some()
            && slots.as_ref().is_some_and(Vec::is_empty)
            && operations.is_empty()
        {
            let held_by = self
                .committed
                .get(&observation.occurrence)
                .map_or(observation.producer, |held| held.producer);
            self.offered_observations += 1;
            self.note_answered(observation);
            let decision = MutationReceipt::Preserve {
                occurrence: observation.occurrence.clone(),
                held_by,
            };
            self.record_layer_decision(observation, text, &decision, None);
            return decision;
        }
        let text = kept_label.as_deref().unwrap_or(text);
        // Sources this batch retires one by one: each held word that a proposed
        // word replaces as the same anchored word.
        let individually_retired = slots
            .as_ref()
            .map(|proposed| {
                self.slots_of(&observation.occurrence)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|source| {
                        proposed.iter().any(|output| {
                            output != *source && self.same_anchored_word(source, output)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let blocked_sources = if observation.producer == ObservationProducer::ManualHuman {
            Vec::new()
        } else {
            self.slots_of(&observation.occurrence)
                .unwrap_or(&[])
                .iter()
                .filter(|source| {
                    let operation_outputs = operations
                        .iter()
                        .find(|operation| operation.sources.contains(source))
                        .map(|operation| operation.outputs.as_slice());
                    let individual_outputs = slots
                        .as_ref()
                        .filter(|proposed| {
                            proposed.iter().any(|output| {
                                output != *source && self.same_anchored_word(source, output)
                            })
                        })
                        .map(Vec::as_slice);
                    let retired = operations
                        .iter()
                        .find(|operation| operation.sources.contains(source))
                        .map_or(individually_retired.as_slice(), |operation| {
                            operation.sources.as_slice()
                        });
                    operation_outputs
                        .or(individual_outputs)
                        .is_some_and(|outputs| {
                            !self.slot_retirement_has_acoustic_coverage(
                                observation,
                                source,
                                outputs,
                                retired,
                            )
                        })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        // D5: a source the batch cannot retire keeps its words. The rest of
        // the batch still applies: proven retirements, and words over speech
        // no held word labels (field shape W27 of take fc40155d).
        let unproven = if blocked_sources.is_empty() {
            None
        } else if let Some(kept) = self.withhold_unproven_retirements(
            observation,
            slots.as_deref(),
            operations,
            &blocked_sources,
        ) {
            Some(kept)
        } else {
            let held_by = self
                .committed
                .get(&observation.occurrence)
                .map_or(observation.producer, |held| held.producer);
            self.offered_observations += 1;
            self.note_answered(observation);
            self.retain_slot_alternative(
                observation,
                text,
                blocked_sources,
                "absence_requires_acoustic_evidence",
            );
            let decision = MutationReceipt::Preserve {
                occurrence: observation.occurrence.clone(),
                held_by,
            };
            self.record_layer_decision(observation, text, &decision, None);
            return decision;
        };
        let (slots, operations, text) = match unproven.as_ref() {
            Some((kept, label)) => (Some(kept.clone()), &[][..], label.as_str()),
            None => (slots, operations, text),
        };
        self.offered_observations += 1;
        let has_pinned_sources = self
            .slots_of(&observation.occurrence)
            .is_some_and(|sources| {
                sources
                    .iter()
                    .any(|source| self.word_pin_observations.contains(&source.observation))
            });
        let whole_owner_recovery = slots.is_none() && !slot_revision;
        // A source extent is not a returned Word target. A whole-label
        // observation cannot acquire pinned authority or settle its debt.
        let authorized_recovery = (slots.is_some() || !has_pinned_sources)
            && !text.trim().is_empty()
            && matches!(
                observation.producer,
                ObservationProducer::Whisper
                    | ObservationProducer::CloudLive
                    | ObservationProducer::ManualHuman
            )
            && self
                .committed
                .get(&observation.occurrence)
                .is_none_or(|held| {
                    slots.is_some()
                        || observation.producer.authority_rank() > held.producer.authority_rank()
                        || (observation.producer == held.producer
                            && observation.generation > held.generation)
                });
        let pinned_sources = self
            .slots_of(&observation.occurrence)
            .unwrap_or(&[])
            .iter()
            .filter(|slot| self.word_pin_observations.contains(&slot.observation))
            .cloned()
            .collect::<Vec<_>>();
        if slots.is_none()
            && !pinned_sources.is_empty()
            && self.text_of(&observation.occurrence) != Some(text)
        {
            self.retain_slot_alternative(
                observation,
                text,
                pinned_sources,
                "whole_label_has_no_word_targets",
            );
        }
        if slots.is_none()
            && observation.producer != ObservationProducer::ManualHuman
            && self
                .slots_of(&observation.occurrence)
                .is_some_and(|sources| sources.is_empty())
        {
            let deleted_sources = self
                .word_deletions
                .iter()
                .filter(|deletion| {
                    deletion.operation.observation.occurrence == observation.occurrence
                })
                .flat_map(|deletion| deletion.operation.sources.clone())
                .collect::<Vec<_>>();
            if !deleted_sources.is_empty() {
                self.retain_slot_alternative(
                    observation,
                    text,
                    deleted_sources,
                    "word_targets_required_after_no_speech",
                );
            }
        }
        let decision = self.decide_observation(observation, text, slot_revision, slots.is_some());
        let applies_slots = decision.grants_mutation()
            || (slot_revision && matches!(decision, MutationReceipt::Preserve { .. }));
        // D3: a word that replaces another engine's word on the same range
        // carries that engine's verdict. Group the proposed words by the held
        // word each one replaces and stamp them before any receipt records
        // them, so receipts and held slots stay the same words.
        let slots = if applies_slots && let Some(mut proposed) = slots {
            let held_sources = self
                .slots_of(&observation.occurrence)
                .unwrap_or(&[])
                .to_vec();
            let mut groups: Vec<(WordSlot, Vec<usize>)> = Vec::new();
            for (index, output) in proposed.iter().enumerate() {
                if operations
                    .iter()
                    .any(|group| group.outputs.contains(output))
                {
                    continue;
                }
                let Some(source) = held_sources
                    .iter()
                    .find(|source| *source != output && self.same_anchored_word(source, output))
                else {
                    continue;
                };
                match groups.iter_mut().find(|(held, _)| held == source) {
                    Some((_, members)) => members.push(index),
                    None => groups.push((source.clone(), vec![index])),
                }
            }
            for (source, members) in groups {
                let mut outputs = members
                    .iter()
                    .map(|index| proposed[*index].clone())
                    .collect::<Vec<_>>();
                self.stamp_word_witness(observation, std::slice::from_ref(&source), &mut outputs);
                for (index, output) in members.into_iter().zip(outputs) {
                    proposed[index] = output;
                }
            }
            Some(proposed)
        } else {
            slots
        };
        let mut individual_sources = Vec::new();
        if applies_slots && let Some(proposed) = slots.as_ref() {
            for output in proposed {
                if operations
                    .iter()
                    .any(|group| group.outputs.contains(output))
                {
                    continue;
                }
                let source = self
                    .slots_of(&observation.occurrence)
                    .unwrap_or(&[])
                    .iter()
                    .find(|source| {
                        self.same_anchored_word(source, output)
                            || (output.producer == ObservationProducer::ManualHuman
                                && source.sample_start == output.sample_start
                                && source.sample_end == output.sample_end)
                    })
                    .cloned();
                if source.as_ref() == Some(output) {
                    continue;
                }
                individual_sources.extend(source.iter().cloned());
                let source_ranges = source
                    .as_ref()
                    .map(|source| self.slot_source_ranges(source))
                    .unwrap_or_else(|| {
                        let original = self
                            .assigned_word_pin_ranges(observation)
                            .into_iter()
                            .filter(|(owner, pin)| {
                                *owner == observation.occurrence
                                    && pin.sample_start.max(owner.sample_start)
                                        == output.sample_start
                                    && pin.sample_end.min(owner.sample_end) == output.sample_end
                            })
                            .map(|(_, pin)| pin)
                            .collect::<Vec<_>>();
                        if original.len() == 1 {
                            original
                        } else {
                            vec![OccurrenceIdentity::new(
                                &observation.occurrence.session,
                                observation.occurrence.capture_epoch,
                                output.sample_start,
                                output.sample_end,
                            )]
                        }
                    });
                self.record_slot_operation(SlotOperationReceipt {
                    observation: observation.clone(),
                    kind: if source.is_some() {
                        SlotOperationKind::Correct
                    } else {
                        SlotOperationKind::Insert
                    },
                    sources: source.into_iter().collect(),
                    outputs: vec![output.clone()],
                    source_ranges,
                    rule_id: "unique_pcm_slot/v1".to_string(),
                });
            }
        }
        if applies_slots
            && let Some(slots) = slots
            && let Some(held) = self.committed.get_mut(&observation.occurrence)
        {
            self.word_pin_observations.insert(observation.clone());
            // Only explicit operations retire a source. Remove predecessors
            // before inserting outputs: an identical output is still held.
            held.slots.retain(|source| {
                !individual_sources.contains(source)
                    && !operations
                        .iter()
                        .any(|group| group.sources.contains(source))
            });
            for slot in slots {
                if !held.slots.contains(&slot) {
                    held.slots.push(slot);
                }
            }
            held.slots
                .sort_by_key(|word| (word.sample_start, word.sample_end));
            held.recompose();
        }
        if applies_slots {
            for operation in operations {
                self.record_slot_operation(operation.clone());
            }
            // A partition retains its predecessor lineage as one operation.
            // Its clipped children also need an exact association to their
            // own original words. Record that association in the same receipt
            // history without changing any output geometry or surface.
            for output in operations
                .iter()
                .filter(|operation| {
                    operation.kind == SlotOperationKind::Split || operation.outputs.len() > 1
                })
                .flat_map(|operation| &operation.outputs)
            {
                let original = self
                    .assigned_word_pin_ranges(observation)
                    .into_iter()
                    .filter(|(owner, pin)| {
                        *owner == observation.occurrence
                            && pin.same_capture(owner)
                            && pin.sample_start.max(owner.sample_start) == output.sample_start
                            && pin.sample_end.min(owner.sample_end) == output.sample_end
                    })
                    .map(|(_, pin)| pin)
                    .collect::<Vec<_>>();
                if let [pin] = original.as_slice()
                    && (pin.sample_start != output.sample_start
                        || pin.sample_end != output.sample_end)
                {
                    self.record_slot_operation(SlotOperationReceipt {
                        observation: observation.clone(),
                        kind: SlotOperationKind::Correct,
                        sources: vec![output.clone()],
                        outputs: vec![output.clone()],
                        source_ranges: original,
                        rule_id: "measured_word_source/v1".into(),
                    });
                }
            }
        }
        if applies_slots {
            self.remember_word_outputs(observation);
        }
        self.record_layer_decision(observation, text, &decision, None);
        let accepted = decision.grants_mutation()
            || (slot_revision && matches!(decision, MutationReceipt::Preserve { .. }));
        if applies_slots && decision.grants_mutation() {
            self.account_published_coarse_words(observation, operations);
        }
        if authorized_recovery
            && observation.producer == ObservationProducer::ManualHuman
            && !slot_revision
            && (decision.grants_mutation() || matches!(decision, MutationReceipt::Preserve { .. }))
        {
            // A whole-owner human decision can settle the whole text debt.
            // A targeted human edit below settles only its addressed PCM.
            self.rejected_word_pins.remove(&observation.occurrence);
        } else if accepted {
            self.reconcile_returned_word_debt(&observation.occurrence);
        }
        // Historical alternatives are recovery evidence, not merely UI copy.
        // A later agreement or partial revision cannot discharge debt while
        // an exact refused source remains held. Successful targeted revisions
        // replace those sources; an explicit human decision may supersede them.
        let retained_refused_source = observation.producer != ObservationProducer::ManualHuman
            && self.retained_recovery_source(&observation.occurrence);
        let recovery_accounted = !self.text_recovery_pending(&observation.occurrence)
            || (whole_owner_recovery && observation.producer == ObservationProducer::ManualHuman)
            || self.returned_word_scope_accounted(observation);
        if authorized_recovery
            && recovery_accounted
            && !retained_refused_source
            && !self
                .rejected_word_pins
                .contains_key(&observation.occurrence)
            && (decision.grants_mutation() || matches!(decision, MutationReceipt::Preserve { .. }))
        {
            self.pending_text_recovery.remove(&observation.occurrence);
        }
        decision
    }

    /// The admission decision itself, without the trail bookkeeping.
    pub(super) fn decide_observation(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        slot_revision: bool,
        has_word_pins: bool,
    ) -> MutationReceipt {
        // An observation that names no audio may be shown but may not act. It
        // is NOT written to the ledger: a zero-width prior that entered the map
        // would relate as `Unanchored` to every later span and refuse all of
        // them (the D3 poisoning mode).
        if !observation.occurrence.is_anchored() {
            self.kept_visible += 1;
            self.note_answered(observation);
            return MutationReceipt::KeepVisibleUnanchored {
                occurrence: observation.occurrence.clone(),
                label: text.to_string(),
                reason: NoAuthorityReason::ZeroWidth,
            };
        }

        if self.answered.contains(observation) {
            self.note_answered(observation);
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::BatchDuplicate,
            };
        }
        self.note_answered(observation);

        // Formatter authors derived presentation only. Every Raw label and
        // slot admission reaches this decision owner, regardless of pin shape,
        // qualification, producer rank or seal state. Keep its proposal in the
        // decision trail without inserting or replacing acoustic content.
        if observation.producer == ObservationProducer::Formatter {
            if !has_word_pins
                && let Some(held) = self.committed.get(&observation.occurrence).cloned()
                && held.slots.len() == 1
                && held.label != text
            {
                self.retain_slot_alternative(
                    observation,
                    text,
                    held.slots,
                    "whole_label_has_no_word_targets",
                );
            }
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::AuthorityConflict,
            };
        }

        // A sealed occurrence is finished. Its physical claim, its serial and
        // its layer history are immutable from here. Only an explicit human
        // edit may supersede the lexical label, and it leaves provenance.
        let sealed_by = self
            .seals
            .get(&observation.occurrence)
            .map(|seal| seal.receipt_id.clone());
        if sealed_by.is_some() && observation.producer != ObservationProducer::ManualHuman {
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::SealedReplay,
            };
        }
        if !has_word_pins
            && self.slots_of(&observation.occurrence).is_some_and(|slots| {
                slots
                    .iter()
                    .any(|slot| self.word_pin_observations.contains(&slot.observation))
                    || (slots.is_empty()
                        && observation.producer != ObservationProducer::ManualHuman
                        && self.word_deletions.iter().any(|deletion| {
                            deletion.operation.observation.occurrence == observation.occurrence
                        }))
            })
        {
            return MutationReceipt::Preserve {
                occurrence: observation.occurrence.clone(),
                held_by: self.committed[&observation.occurrence].producer,
            };
        }

        if let Some(supersedes_seal) = sealed_by {
            let held = self.committed.get(&observation.occurrence).cloned();
            let superseded_label = held
                .as_ref()
                .map(|previous| previous.label.clone())
                .unwrap_or_default();
            let from = held
                .as_ref()
                .map_or(ObservationProducer::ManualHuman, |previous| {
                    previous.producer
                });
            let manual_ordinal = self.manual_edits.len();
            self.note_covered_span(&observation.occurrence);
            if has_word_pins && held.is_some() {
                let current = self.committed.get_mut(&observation.occurrence).unwrap();
                current.producer = observation.producer;
                current.generation = observation.generation;
            } else {
                self.committed.insert(
                    observation.occurrence.clone(),
                    if has_word_pins {
                        CommittedObservation::empty(observation)
                    } else {
                        CommittedObservation::from_label(observation, text)
                    },
                );
            }
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
            if let Some(held) = self.committed.get(&observation.occurrence) {
                return MutationReceipt::Preserve {
                    occurrence: observation.occurrence.clone(),
                    held_by: held.producer,
                };
            }
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
            if held.label == text {
                return MutationReceipt::Preserve {
                    occurrence: observation.occurrence.clone(),
                    held_by: held.producer,
                };
            }
            if outranks || same_lane_revision || slot_revision {
                if !has_word_pins
                    && self
                        .replace_neighbour(&observation.occurrence, &held.slots)
                        .is_some()
                {
                    self.retain_slot_alternative(
                        observation,
                        text,
                        held.slots.clone(),
                        "clock_lie",
                    );
                    return MutationReceipt::Refuse {
                        occurrence: observation.occurrence.clone(),
                        reason: RefuseReason::ClockLie,
                    };
                }
                self.note_covered_span(&observation.occurrence);
                if has_word_pins {
                    let current = self.committed.get_mut(&observation.occurrence).unwrap();
                    current.producer = observation.producer;
                    current.generation = observation.generation;
                } else {
                    let single_source = held.slots.len() == 1
                        && held.slots[0].sample_start == observation.occurrence.sample_start
                        && held.slots[0].sample_end == observation.occurrence.sample_end;
                    // A recovery decode's omission is still not acoustic
                    // absence. Whole-label revisions retain unpaired content;
                    // measured word operations own any physical retirement.
                    if single_source && observation.producer != ObservationProducer::ManualHuman {
                        let source = &held.slots[0];
                        let alignment = slot_ops::preserve_group_content(&source.text, text);
                        let (label, retained, reason) = match alignment {
                            Ok((label, retained)) => (label, retained, "held_token_retained"),
                            Err(reason) => (source.text.clone(), true, reason),
                        };
                        if retained {
                            self.retain_slot_alternative(
                                observation,
                                text,
                                held.slots.clone(),
                                reason,
                            );
                            self.record_slot_operation(SlotOperationReceipt {
                                observation: observation.clone(),
                                kind: SlotOperationKind::Correct,
                                sources: held.slots.clone(),
                                outputs: held.slots.clone(),
                                source_ranges: self.slot_source_ranges(source),
                                rule_id: format!("{reason}/held_token_retained/v1"),
                            });
                            return MutationReceipt::Preserve {
                                occurrence: observation.occurrence.clone(),
                                held_by: held.producer,
                            };
                        }
                        let mut output = source.clone();
                        output.text = label;
                        output.producer = observation.producer;
                        output.observation = observation.clone();
                        let source_ranges = self.slot_source_ranges(source);
                        let mut current = held.clone();
                        current.producer = observation.producer;
                        current.generation = observation.generation;
                        current.slots = vec![output.clone()];
                        current.recompose();
                        self.committed
                            .insert(observation.occurrence.clone(), current);
                        self.record_slot_operation(SlotOperationReceipt {
                            observation: observation.clone(),
                            kind: SlotOperationKind::Correct,
                            sources: held.slots.clone(),
                            outputs: vec![output],
                            source_ranges,
                            rule_id: "group_alignment/v1".into(),
                        });
                    } else {
                        self.committed.insert(
                            observation.occurrence.clone(),
                            CommittedObservation::from_label(observation, text),
                        );
                    }
                }
                return MutationReceipt::Correct {
                    occurrence: observation.occurrence.clone(),
                    from: held.producer,
                    to: observation.producer,
                };
            }
            if !has_word_pins && held.slots.len() == 1 {
                self.retain_slot_alternative(observation, text, held.slots, "protected_source");
            }
            return MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason: RefuseReason::AuthorityConflict,
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
        if overlaps && !has_word_pins {
            // Shares audio with something committed, but the payload carries no
            // word pins, so no token can be attributed to the shared part.
            // Clipping here would delete speech on a guess; the honest answer is
            // to show it and grant it nothing.
            self.kept_visible += 1;
            return MutationReceipt::KeepVisibleUnanchored {
                occurrence: observation.occurrence.clone(),
                label: text.to_string(),
                reason: NoAuthorityReason::OverlapWithoutWordPins,
            };
        }

        // An automatic label claiming PCM no authenticated observer measured
        // as speech is a phantom: it stays visible as evidence and is granted
        // no document. Only an Observed measurement gates — absent or degraded
        // evidence changes nothing.
        if observation.producer != ObservationProducer::ManualHuman
            && !has_word_pins
            && let Some(speech) = self.speech_evidence.as_ref()
            && matches!(speech.availability(), AcousticAvailability::Observed { .. })
            && !speech.ranges().iter().any(|range| {
                range.sample_start < observation.occurrence.sample_end
                    && observation.occurrence.sample_start < range.sample_end
            })
        {
            self.kept_visible += 1;
            return MutationReceipt::KeepVisibleUnanchored {
                occurrence: observation.occurrence.clone(),
                label: text.to_string(),
                reason: NoAuthorityReason::NoMeasuredSpeech,
            };
        }

        self.note_covered_span(&observation.occurrence);
        self.committed.insert(
            observation.occurrence.clone(),
            if has_word_pins {
                CommittedObservation::empty(observation)
            } else {
                CommittedObservation::from_label(observation, text)
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
        (receipts, self.conservation())
    }

    /// Conservation over every receipt the ledger has issued.
    ///
    /// `observations_in` is the number of admission calls. `receipts_out` is
    /// the number of receipts stored. They are separate counters; the law is
    /// that they match. `refusals_by_reason` is counted at issue time.
    pub fn conservation(&self) -> ConservationTally {
        ConservationTally {
            observations_in: self.offered_observations,
            receipts_out: self.issued_receipts,
            observations_delivered: self.delivered_observations,
            occurrences_held: self.committed.len(),
            kept_visible_unanchored: self.kept_visible,
            refusals_by_reason: self.named_refusals.clone(),
        }
    }

    /// One ClockLie gate: a flagged authorizer may retain its own text but
    /// cannot replace a neighbour. An empty source set is always insertable.
    pub(super) fn replace_neighbour(
        &self,
        authorizer: &OccurrenceIdentity,
        sources: &[WordSlot],
    ) -> Option<OccurrenceIdentity> {
        self.clock_lie_occurrences
            .iter()
            .find(|flagged| {
                matches!(
                    authorizer.relate(flagged),
                    OccurrenceRelation::Same | OccurrenceRelation::Overlapping { .. }
                ) && sources.iter().any(|source| {
                    source.observation.occurrence != **flagged
                        && source.observation.occurrence.same_capture(flagged)
                })
            })
            .cloned()
    }

    /// Select one qualified owner whose range contains the pin centre
    /// ([`declared_range_contains_midpoint`]). An open owner precedes a sealed
    /// owner; equal seal states prefer the later closure start. Equal starts
    /// use the shorter extent deterministically.
    pub(crate) fn word_owner_index(
        &self,
        pin: &OccurrenceIdentity,
        owners: &[OccurrenceIdentity],
    ) -> Option<usize> {
        if !pin.is_anchored() {
            return None;
        }
        owners
            .iter()
            .enumerate()
            .filter(|(_, owner)| {
                pin.same_capture(owner)
                    && self.is_qualified(owner)
                    && declared_range_contains_midpoint(
                        owner.sample_start,
                        owner.sample_end,
                        pin.sample_start,
                        pin.sample_end,
                    )
            })
            .max_by_key(|(_, owner)| {
                (
                    !self.is_sealed(owner),
                    owner.sample_start,
                    std::cmp::Reverse(owner.sample_end),
                )
            })
            .map(|(index, _)| index)
    }

    /// Route word pins by midpoint to any supplied owner, including owners
    /// outside this window's member list. Outside the admit range is not by
    /// itself replay: the caller must prove matching normalized slot text and
    /// overlap of at least half the shorter owner-clipped span. Sealed owners
    /// remain routable so their late words receive visible K5 receipts.
    /// Utterance grain retains whole-range containment and range replay.
    pub fn classify_overlap_pin(
        &self,
        pin: &OccurrenceIdentity,
        admit_start: u64,
        admit_end: u64,
        open_members: &[OccurrenceIdentity],
        word_grain: bool,
    ) -> OverlapPinClass {
        if !pin.is_anchored() {
            return OverlapPinClass::Unanchored(NoAuthorityReason::ZeroWidth);
        }
        if !open_members.iter().any(|member| pin.same_capture(member)) {
            return OverlapPinClass::Unanchored(NoAuthorityReason::NoRange);
        }
        if word_grain && let Some(member_index) = self.word_owner_index(pin, open_members) {
            return OverlapPinClass::ExclusiveTail { member_index };
        }
        // Unqualified geometry candidates have no ledger owner to break a tie.
        // Keep their uniqueness check and the utterance-grain fence below.
        let inside_admit = if word_grain {
            declared_range_contains_midpoint(
                admit_start,
                admit_end,
                pin.sample_start,
                pin.sample_end,
            )
        } else {
            pin.sample_start >= admit_start && pin.sample_end <= admit_end
        };
        if word_grain || inside_admit {
            let mut owners = open_members.iter().enumerate().filter(|(_, member)| {
                pin.same_capture(member)
                    && if word_grain {
                        declared_range_contains_midpoint(
                            member.sample_start,
                            member.sample_end,
                            pin.sample_start,
                            pin.sample_end,
                        )
                    } else {
                        pin.sample_start >= member.sample_start
                            && pin.sample_end <= member.sample_end
                    }
            });
            let first = owners.next();
            let another = owners.next();
            if let (Some((member_index, _)), None) = (first, another) {
                return OverlapPinClass::ExclusiveTail { member_index };
            }
            return OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins);
        }
        let overlaps_admit = pin.sample_end > admit_start && pin.sample_start < admit_end;
        if !overlaps_admit {
            let covered = self.committed.keys().any(|held| {
                pin.same_capture(held)
                    && held.is_anchored()
                    && pin.sample_start >= held.sample_start
                    && pin.sample_end <= held.sample_end
            });
            if covered {
                return OverlapPinClass::Replay;
            }
        }
        OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins)
    }

    /// Keep one observation visible and record the receipt. The committed map
    /// does not gain an occurrence.
    pub fn keep_visible_unanchored(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        reason: NoAuthorityReason,
    ) -> MutationReceipt {
        self.record_non_mutating_input(
            observation,
            crate::pipeline::trail::TrailNonMutatingInput::KeepVisible { reason },
        );
        self.offered_observations += 1;
        self.kept_visible += 1;
        self.note_answered(observation);
        let decision = MutationReceipt::KeepVisibleUnanchored {
            occurrence: observation.occurrence.clone(),
            label: text.to_string(),
            reason,
        };
        self.record_layer_decision(observation, text, &decision, None);
        decision
    }

    /// Refuse a covered overlap by range identity. The committed label stands,
    /// whatever string the later window carried.
    pub fn refuse_replayed_range(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
    ) -> MutationReceipt {
        self.refuse_replacement(observation, text, RefuseReason::ReplayedRangeIdentity)
    }

    /// One material uncovered range the stop path could not recover.
    ///
    /// Idempotent on the same PCM: a second settlement of the same gap does not
    /// mint a second observation. The refusal is the conservation row; nothing
    /// is inserted into the committed document.
    pub fn note_unrecovered_speech(&mut self, occurrence: &OccurrenceIdentity) -> MutationReceipt {
        let observation = ObservationIdentity::new(
            ObservationProducer::Whisper,
            occurrence.sample_start,
            occurrence.sample_end,
            occurrence.clone(),
        );
        if self.answered.contains(&observation) {
            return MutationReceipt::Refuse {
                occurrence: occurrence.clone(),
                reason: RefuseReason::UnrecoveredSpeech,
            };
        }
        self.refuse_replacement(&observation, "", RefuseReason::UnrecoveredSpeech)
    }

    /// Refuse one replacement of an occurrence. The committed label stands.
    /// The reason is the receipt; there is no generic bucket.
    pub fn refuse_replacement(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        reason: RefuseReason,
    ) -> MutationReceipt {
        self.record_non_mutating_input(
            observation,
            crate::pipeline::trail::TrailNonMutatingInput::Refuse { reason },
        );
        self.offered_observations += 1;
        self.note_answered(observation);
        let decision = if text.trim().is_empty()
            && matches!(
                reason,
                RefuseReason::EmptyLabel | RefuseReason::SlotAdmissionRejected
            )
            && let Some(held) = self.committed.get(&observation.occurrence)
        {
            MutationReceipt::Preserve {
                occurrence: observation.occurrence.clone(),
                held_by: held.producer,
            }
        } else {
            MutationReceipt::Refuse {
                occurrence: observation.occurrence.clone(),
                reason,
            }
        };
        self.record_layer_decision(observation, text, &decision, None);
        decision
    }

    pub(crate) fn refuse_superseded_stub(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
        pin: &OccurrenceIdentity,
    ) -> Option<MutationReceipt> {
        if !pin.same_capture(&observation.occurrence) {
            return None;
        }
        let sources = self
            .occurrences()
            .filter(|owner| owner.same_capture(&observation.occurrence))
            .flat_map(|owner| self.slots_of(owner).unwrap_or(&[]).iter())
            .filter(|slot| {
                slot.producer == ObservationProducer::Whisper
                    && self.word_slot_targets_pin(slot, pin)
                    && self.complete_word_slot(slot)
            })
            .cloned()
            .collect::<Vec<_>>();
        if sources.len() != 1 {
            return None;
        }
        self.record_non_mutating_input(
            observation,
            crate::pipeline::trail::TrailNonMutatingInput::SupersededStub { pin: pin.clone() },
        );
        self.retain_slot_alternative(observation, text, sources, "window_stub_superseded");
        Some(self.refuse_replacement(observation, text, RefuseReason::ReplacedByWhisper))
    }

    pub(super) fn record_non_mutating_input(
        &mut self,
        observation: &ObservationIdentity,
        operation: crate::pipeline::trail::TrailNonMutatingInput,
    ) {
        if self.trail_input.is_none() && crate::pipeline::trail::is_enabled(&observation.occurrence)
        {
            self.trail_input = Some(crate::pipeline::trail::TrailAdmission {
                source_slots: self
                    .slots_of(&observation.occurrence)
                    .unwrap_or(&[])
                    .to_vec(),
                offered_slots: None,
                slot_revision: false,
                capture_rate_hz: self.capture_rate_hz,
                non_mutating: Some(operation),
            });
        }
    }
}
