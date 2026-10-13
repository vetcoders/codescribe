//! Acoustic sequence normalisation: one measured stretch of speech described
//! by two producers as the same number of words, in the same order, over the
//! same PCM, is one sequence of words. The held geometry is adopted before any
//! per-word matcher runs, so a producer's estimated timestamps cannot mint a
//! second occurrence of a word that was spoken once.
//!
//! Authority stays in the ledger: this step only rewrites the *claimed* ranges
//! of the incoming pins onto the ranges the ledger already holds for the same
//! measured speech and records a receipt naming every source. Labels are not
//! compared here — equal text is evidence about a resolved component, never a
//! target finder — and nothing is deleted. Whisper gets no clock authority:
//! whichever producer arrived first holds the geometry, the acoustic observer
//! (Silero / energy) decides which words share one stretch of speech.
use super::*;

/// Receipt rule for a sequence adopted onto held geometry.
pub const SEQUENCE_NORMALISATION_RULE: &str = "acoustic_sequence_normalisation.v1";

/// Index of the measured speech range a PCM span belongs to: the range it
/// overlaps the most. A span touching no measured speech belongs nowhere.
fn speech_range_of(ranges: &[(u64, u64)], start: u64, end: u64) -> Option<usize> {
    if end <= start {
        return None;
    }
    ranges
        .iter()
        .enumerate()
        .map(|(index, (range_start, range_end))| {
            let overlap = end.min(*range_end).saturating_sub(start.max(*range_start));
            (overlap, index)
        })
        .filter(|(overlap, _)| *overlap > 0)
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, index)| index)
}

/// Two sequences describe the same stretch of speech when their unions
/// overlap by at least half of the shorter union.
fn unions_share_speech(held: &[WordSlot], incoming: &[WordPin]) -> bool {
    let (Some(h_start), Some(h_end), Some(i_start), Some(i_end)) = (
        held.iter().map(|slot| slot.sample_start).min(),
        held.iter().map(|slot| slot.sample_end).max(),
        incoming.iter().map(|pin| pin.sample_start).min(),
        incoming.iter().map(|pin| pin.sample_end).max(),
    ) else {
        return false;
    };
    let overlap = h_end.min(i_end).saturating_sub(h_start.max(i_start));
    let shorter = h_end
        .saturating_sub(h_start)
        .min(i_end.saturating_sub(i_start));
    shorter > 0 && overlap >= shorter / 2 + shorter % 2
}

impl AcousticLedger {
    /// Adopt held word geometry for an incoming sequence that names the same
    /// measured speech. Mutates `pins` in place (ranges only) and records one
    /// `SlotOperationReceipt` for the owner with every held source and every
    /// adopted output, so the whole competing sequence is
    /// admitted with provenance instead of a dropped or minted prefix.
    ///
    /// Conditions, all required, over the owner occurrence:
    /// - the acoustic observer (Silero boundaries or the energy ladder) has
    ///   measured this capture, the owner lies within the observed PCM and
    ///   measured speech exists under the union of both claims;
    /// - the ledger holds `n >= 1` measured single-word slots of another
    ///   producer family in the owner, and the incoming batch offers
    ///   exactly `n` pins;
    /// - the two unions overlap by at least half of the shorter one;
    /// - no incoming pin declares a decode fence that would exclude the held
    ///   range it is about to adopt;
    /// - at least one pair is not already the same PCM slot.
    ///
    /// Same-producer regenerations, coarse phrase slots, manual human words
    /// and sequences of different length are left to the existing matchers.
    pub(super) fn normalise_sequence_to_held_speech(
        &mut self,
        observation: &ObservationIdentity,
        pins: &mut [WordPin],
    ) {
        if observation.producer == ObservationProducer::ManualHuman {
            return;
        }
        let owner = &observation.occurrence;
        let Some(speech) = self.measured_speech_for(owner) else {
            return;
        };
        let ranges = speech
            .ranges()
            .iter()
            .map(|range| (range.sample_start, range.sample_end))
            .collect::<Vec<(u64, u64)>>();
        if ranges.is_empty() {
            return;
        }
        let held = self
            .slots_of(owner)
            .unwrap_or(&[])
            .iter()
            .filter(|slot| {
                slot.producer != ObservationProducer::ManualHuman
                    && slot.producer != observation.producer
                    && slot.text.split_whitespace().count() == 1
                    && !self.coarse_word_source(slot)
            })
            .cloned()
            .collect::<Vec<WordSlot>>();
        if held.is_empty() {
            return;
        }
        // One owner occurrence is one acoustically bounded stretch of speech
        // (its VAD open/close samples). The producers' pins are matched as
        // whole ordered sequences over that stretch, not per energy burst:
        // on recorded takes the capture ladder splits one phrase into
        // several bursts and a producer's timestamps can land a word on the
        // burst after the one the other producer chose ("Jest" at 5.93 s
        // against "jest" at 5.22 s on take 018e2177). Membership per burst
        // would read that as two words; the ordered sequence does not.
        let mut held_sorted = held;
        held_sorted.sort_by_key(|slot| (slot.sample_start, slot.sample_end));
        let mut incoming_sorted = pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| !pin.text.trim().is_empty() && pin.sample_start < pin.sample_end)
            .map(|(index, pin)| (index, pin.clone()))
            .collect::<Vec<(usize, WordPin)>>();
        incoming_sorted.sort_by_key(|(_, pin)| (pin.sample_start, pin.sample_end));
        if held_sorted.len() != incoming_sorted.len() {
            return;
        }
        let incoming_pins = incoming_sorted
            .iter()
            .map(|(_, pin)| pin.clone())
            .collect::<Vec<WordPin>>();
        if !unions_share_speech(&held_sorted, &incoming_pins) {
            return;
        }
        // Measured speech must exist under both claims: a sequence over
        // quiet PCM is not speech named twice, it is a claim to refuse later.
        let (Some(union_start), Some(union_end)) = (
            held_sorted
                .iter()
                .map(|slot| slot.sample_start)
                .chain(incoming_pins.iter().map(|pin| pin.sample_start))
                .min(),
            held_sorted
                .iter()
                .map(|slot| slot.sample_end)
                .chain(incoming_pins.iter().map(|pin| pin.sample_end))
                .max(),
        ) else {
            return;
        };
        if speech_range_of(&ranges, union_start, union_end).is_none() {
            return;
        }
        let already_same = held_sorted.iter().zip(&incoming_pins).all(|(slot, pin)| {
            pcm_ranges_share_slot(
                slot.sample_start,
                slot.sample_end,
                pin.sample_start,
                pin.sample_end,
            )
        });
        if already_same {
            return;
        }
        let fences_allow = held_sorted.iter().zip(&incoming_pins).all(|(slot, pin)| {
            match (pin.decode_sample_start, pin.decode_sample_end) {
                (Some(start), Some(end)) => start <= slot.sample_start && slot.sample_end <= end,
                _ => true,
            }
        });
        if !fences_allow {
            return;
        }
        let mut moves: Vec<((u64, u64), (u64, u64))> = Vec::new();
        let mut outputs = Vec::with_capacity(held_sorted.len());
        for (slot, (pin_index, _)) in held_sorted.iter().zip(&incoming_sorted) {
            let pin = &mut pins[*pin_index];
            moves.push((
                (pin.sample_start, pin.sample_end),
                (slot.sample_start, slot.sample_end),
            ));
            pin.sample_start = slot.sample_start;
            pin.sample_end = slot.sample_end;
            outputs.push(WordSlot {
                sample_start: slot.sample_start,
                sample_end: slot.sample_end,
                text: pin.text.clone(),
                producer: observation.producer,
                observation: observation.clone(),
                witness: SlotWitness::Unwitnessed,
                confidence: pin.confidence,
                surface_rewritten: pin.surface_rewritten,
            });
        }
        let source_ranges = held_sorted
            .iter()
            .map(|slot| {
                OccurrenceIdentity::new(
                    &owner.session,
                    owner.capture_epoch,
                    slot.sample_start,
                    slot.sample_end,
                )
            })
            .collect::<Vec<OccurrenceIdentity>>();
        let receipts = vec![SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Correct,
            sources: held_sorted,
            outputs,
            source_ranges,
            rule_id: SEQUENCE_NORMALISATION_RULE.to_string(),
        }];
        for receipt in receipts {
            self.record_slot_operation(receipt);
        }
        // The staged evidence was built from the pins as offered. Adopted
        // words must keep their original label and decode fence under the
        // held geometry, otherwise adjudication reads them as unpinned.
        if !moves.is_empty() {
            self.rebase_word_evidence(observation, &moves);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_range_is_the_one_overlapped_most_and_none_without_overlap() {
        let ranges = [(46_080, 224_256), (256_512, 345_600), (387_072, 473_088)];
        // Whisper's "jest" starts in the gap before the second range and
        // overlaps it by 4 608 samples: it belongs to the second range.
        assert_eq!(speech_range_of(&ranges, 250_560, 261_120), Some(1));
        assert_eq!(speech_range_of(&ranges, 284_640, 290_400), Some(1));
        assert_eq!(speech_range_of(&ranges, 230_000, 250_000), None);
        assert_eq!(speech_range_of(&ranges, 300_000, 300_000), None);
    }

    #[test]
    fn unions_must_overlap_by_half_of_the_shorter_union() {
        let owner = OccurrenceIdentity::new("n", 1, 0, 400_000);
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner);
        let held = [(284_640, 290_400), (303_360, 337_920)].map(|(start, end)| WordSlot {
            sample_start: start,
            sample_end: end,
            text: "x".into(),
            producer: ObservationProducer::Apple,
            observation: observation.clone(),
            witness: SlotWitness::Unwitnessed,
            confidence: None,
            surface_rewritten: false,
        });
        let near = [(250_560, 261_120), (299_520, 328_320)]
            .map(|(start, end)| WordPin::new(start, end, "x"));
        let far = [(100_000, 110_000), (120_000, 130_000)]
            .map(|(start, end)| WordPin::new(start, end, "x"));
        assert!(unions_share_speech(&held, &near));
        assert!(!unions_share_speech(&held, &far));
    }
}
