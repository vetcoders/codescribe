//! Live-first hold at a decode fence (plan §4, 13 X 2026).
//!
//! A live Whisper window ends at a decode fence the decoder did not choose
//! acoustically. When the capture writer's own hops show speech running
//! through that fence, the words sitting on that open burst are not words
//! yet: the decoder completed clipped phonemes into plausible tokens ("pumpy
//! to" for "Computer Use." on take 36de5e98, build 953). Those pins are held
//! as alternatives (a receipt, never a refusal); the next window that covers
//! the burst with a complete fence owns it. The hold does not open a text
//! recovery job: the hold itself blocks sealing until complete live evidence
//! accounts for it, and the held reading stays visible as an alternative.
//! Nothing waits for Stop: a window
//! whose fence is the end of the captured PCM is never held, because no later
//! window can exist and the last words of a take must reach delivery.
//!
//! The burst is measured relative to the take, not to a fixed floor: the
//! floor is the 10th percentile of hop levels inside the window, speech is
//! floor + 8 dB, gaps up to 60 ms are bridged. This is the same definition
//! the real-take tests use (`word_identity_energy_tests.rs`), so a held word
//! in a test is a held word in the product.
use super::*;

/// Reason on the retained alternative of a held edge word.
pub const EDGE_CUT_OPEN_BURST: &str = "edge_cut_open_burst";

/// Receipt of one live hold: which pins were kept back and why.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFenceHoldReceipt {
    pub observation: ObservationIdentity,
    pub window_start: u64,
    pub fence: u64,
    pub burst_start: u64,
    pub held: Vec<WordPin>,
    /// A later complete window accounted for every held physical word.
    pub settled: bool,
}

const SPEECH_ABOVE_FLOOR_DB: f32 = 8.0;
const BRIDGED_GAP_MS: u64 = 60;
const MIN_HOPS_FOR_FLOOR: usize = 50;
/// How far before its fence a live window may hold words. The next live
/// window starts `window - step` before this fence (`CaptureWindowPlan`:
/// 9 s windows every 3 s, so 6 s), and only words it covers have a later
/// owner. A word on the open burst but earlier than that is delivered: a
/// held word nobody can take is a lost word.
const HOLD_REACH_SECS: u64 = 6;

impl AcousticLedger {
    /// Start of the speech burst running through `fence`, or `None` when the
    /// fence hop is quiet by the take's own measure, when the writer cannot
    /// speak for the window, when the window is too short for a floor, or
    /// when the fence is the end of the captured PCM.
    pub(super) fn open_burst_start_at_fence(
        &self,
        owner: &OccurrenceIdentity,
        window_start: u64,
        fence: u64,
    ) -> Option<u64> {
        let energy = self.decode_fence_energy.as_ref()?;
        if fence <= window_start
            || energy.captured_samples < fence
            || self
                .exhausted_capture
                .as_ref()
                .is_some_and(|capture| capture.same_capture(owner) && capture.sample_end == fence)
        {
            return None;
        }
        // Exact quiet PCM outranks a hop containing both the preceding word
        // and its trailing pause. A complete repetition must not be held.
        let resolution = u64::from(self.capture_rate_hz.unwrap_or(16_000)).div_ceil(50) + 1;
        if self.decode_fence_is_quiet(
            owner,
            fence.saturating_sub(resolution).max(window_start),
            fence,
        ) {
            return None;
        }
        let hops =
            energy
                .owner
                .hop_levels_in(&owner.session, owner.capture_epoch, window_start, fence)?;
        if hops.len() < MIN_HOPS_FOR_FLOOR {
            return None;
        }
        let db = |rms: f32| 20.0 * rms.max(1.0e-9).log10();
        let mut levels = hops
            .iter()
            .map(|(_, _, rms)| db(*rms))
            .collect::<Vec<f32>>();
        levels.sort_by(f32::total_cmp);
        let floor = levels[levels.len() / 10];
        let threshold = floor + SPEECH_ABOVE_FLOOR_DB;
        let (_, _, fence_rms) = *hops.last()?;
        if db(fence_rms) <= threshold {
            return None;
        }
        let bridged_gap =
            u64::from(self.capture_rate_hz.unwrap_or(16_000)) * BRIDGED_GAP_MS / 1_000;
        let mut burst_start = None;
        let mut gap = 0;
        for (start, end, rms) in hops.iter().rev() {
            if db(*rms) > threshold {
                burst_start = Some(*start);
                gap = 0;
            } else {
                gap += end - start;
                if gap > bridged_gap {
                    break;
                }
            }
        }
        burst_start
    }

    /// Keep back the pins of a live window that sit on the burst running
    /// through its decode fence. Returns the receipt when anything was held.
    /// The held pins remain in this receipt until a complete successor owns
    /// them; no additional decoding job is scheduled by the hold.
    pub(super) fn hold_edge_cut_pins(
        &mut self,
        observation: &ObservationIdentity,
        pins: &mut Vec<WordPin>,
    ) -> Option<LiveFenceHoldReceipt> {
        if observation.producer != ObservationProducer::Whisper {
            return None;
        }
        let (window_start, fence) = pins
            .first()
            .and_then(|pin| pin.decode_sample_start.zip(pin.decode_sample_end))?;
        if !pins.iter().all(|pin| {
            pin.decode_sample_start == Some(window_start) && pin.decode_sample_end == Some(fence)
        }) {
            return None;
        }
        let owner = &observation.occurrence;
        if !self.is_qualified(owner) {
            return None;
        }
        let burst_start = self.open_burst_start_at_fence(owner, window_start, fence)?;
        let reach = fence
            .saturating_sub(u64::from(self.capture_rate_hz.unwrap_or(16_000)) * HOLD_REACH_SECS);
        let (held, kept): (Vec<WordPin>, Vec<WordPin>) = pins
            .drain(..)
            .partition(|pin| pin.sample_end > burst_start && pin.sample_start >= reach);
        *pins = kept;
        if held.is_empty() {
            return None;
        }
        for pin in &held {
            self.retain_slot_alternative(observation, &pin.text, Vec::new(), EDGE_CUT_OPEN_BURST);
        }
        let receipt = LiveFenceHoldReceipt {
            observation: observation.clone(),
            window_start,
            fence,
            burst_start,
            held,
            settled: false,
        };
        self.live_fence_holds.push(receipt.clone());
        Some(receipt)
    }

    pub fn live_fence_holds(&self) -> &[LiveFenceHoldReceipt] {
        &self.live_fence_holds
    }

    /// A hold is unresolved speech, not permission to seal a shortened take.
    /// Match only complete words from this accepted successor, one-to-one,
    /// with original held PCM inside its decode window. Labels have no vote.
    pub(super) fn settle_live_fence_holds(&mut self, observation: &ObservationIdentity) {
        let Some((start, end)) = self.decoded_word_windows.get(observation).copied() else {
            return;
        };
        if !self.word_pin_observations.contains(observation) {
            return;
        }
        let owner = &observation.occurrence;
        let slots = self.slots_of(owner).unwrap_or(&[]);
        let settled = self
            .live_fence_holds
            .iter()
            .enumerate()
            .filter_map(|(index, hold)| {
                if hold.settled
                    || hold.observation == *observation
                    || hold.observation.occurrence != *owner
                {
                    return None;
                }
                let mut used = BTreeSet::new();
                for pin in &hold.held {
                    if start > pin.sample_start || end < pin.sample_end {
                        return None;
                    }
                    let target = OccurrenceIdentity::new(
                        &owner.session,
                        owner.capture_epoch,
                        pin.sample_start,
                        pin.sample_end,
                    );
                    let matches = slots
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| {
                            self.complete_word_source(slot)
                                .is_some_and(|source| source.observation == *observation)
                                && self.word_slot_targets_pin(slot, &target)
                        })
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>();
                    let [matched] = matches.as_slice() else {
                        return None;
                    };
                    if !used.insert(*matched) {
                        return None;
                    }
                }
                Some(index)
            })
            .collect::<Vec<_>>();
        for index in settled {
            self.live_fence_holds[index].settled = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::word_adjudication_tests::measured_ledger_at_rate;

    const RATE: u32 = 48_000;
    const SECOND: u64 = RATE as u64;

    /// Ten seconds of a quiet floor (about -95 dBFS, under the ladder's
    /// absolute floor) with speech bursts (about -25 dBFS) over the given spans. Deterministic: no RNG.
    fn synthetic_take(bursts: &[(f64, f64)]) -> Vec<f32> {
        let total = 10 * SECOND as usize;
        let mut state: u32 = 0x9e37_79b9;
        let mut pcm = Vec::with_capacity(total);
        for index in 0..total {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let seconds = index as f64 / RATE as f64;
            let speaking = bursts
                .iter()
                .any(|(start, end)| seconds >= *start && seconds < *end);
            let level = if speaking { 0.1 } else { 0.000_03 };
            pcm.push(noise * level * 2.0);
        }
        pcm
    }

    fn window(words: &[(&str, f64, f64)], decode: (f64, f64)) -> Vec<WordPin> {
        let at = |seconds: f64| (seconds * RATE as f64).round() as u64;
        words
            .iter()
            .map(|(text, start, end)| {
                WordPin::new(at(*start), at(*end), *text)
                    .with_decode_window(at(decode.0), at(decode.1))
            })
            .collect()
    }

    fn ledger(pcm: &[f32]) -> (AcousticLedger, OccurrenceIdentity) {
        let owner = OccurrenceIdentity::new("live-fence-hold", 1, 0, 10 * SECOND);
        let mut ledger = measured_ledger_at_rate(&owner, pcm, RATE);
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        (ledger, owner)
    }

    #[test]
    fn a_burst_running_through_the_fence_holds_the_words_sitting_on_it() {
        let pcm = synthetic_take(&[(1.0, 1.5), (2.0, 2.4), (8.7, 9.3)]);
        let (mut ledger, owner) = ledger(&pcm);
        let live = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(
            &[("a", 1.0, 1.5), ("b", 2.0, 2.4), ("c", 8.7, 9.0)],
            (0.0, 9.0),
        );
        assert!(ledger.admit_word_slots(&live, &pins).grants_mutation());
        assert_eq!(ledger.text_of(&owner), Some("a b"));
        let holds = ledger.live_fence_holds();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].fence, 9 * SECOND);
        assert_eq!(
            holds[0]
                .held
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(
            holds[0].burst_start >= 8 * SECOND + SECOND / 2 && holds[0].burst_start < 9 * SECOND
        );
        assert!(ledger.text_recovery_pending(&owner));
        assert!(
            ledger
                .slot_alternatives
                .iter()
                .any(|alternative| alternative.candidate == "c"
                    && alternative.reason == EDGE_CUT_OPEN_BURST)
        );
    }

    #[test]
    fn a_fence_closed_by_silence_holds_nothing() {
        let pcm = synthetic_take(&[(1.0, 1.5), (2.0, 2.4), (8.0, 8.5)]);
        let (mut ledger, owner) = ledger(&pcm);
        let live = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(
            &[("a", 1.0, 1.5), ("b", 2.0, 2.4), ("c", 8.0, 8.5)],
            (0.0, 9.0),
        );
        assert!(ledger.admit_word_slots(&live, &pins).grants_mutation());
        assert_eq!(ledger.text_of(&owner), Some("a b c"));
        assert!(ledger.live_fence_holds().is_empty());
    }

    #[test]
    fn a_complete_live_successor_settles_the_hold_without_waiting_for_stop() {
        let pcm = synthetic_take(&[(1.0, 1.5), (2.0, 2.4), (8.7, 9.3)]);
        let (mut ledger, owner) = ledger(&pcm);
        let first = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        ledger.admit_word_slots(
            &first,
            &window(
                &[("a", 1.0, 1.5), ("b", 2.0, 2.4), ("clipped", 8.7, 9.0)],
                (0.0, 9.0),
            ),
        );
        assert!(ledger.text_recovery_pending(&owner));
        let next = ObservationIdentity::new(ObservationProducer::Whisper, 1, 1, owner.clone());
        ledger.admit_word_slots(&next, &window(&[("complete", 8.7, 9.3)], (3.0, 10.0)));
        assert_eq!(ledger.text_of(&owner), Some("a b complete"));
        assert!(!ledger.text_recovery_pending(&owner));
        assert!(ledger.live_fence_holds()[0].settled);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn the_live_capture_head_cannot_impersonate_stop() {
        let pcm = synthetic_take(&[(1.0, 1.5), (9.6, 10.0)]);
        let (mut ledger, owner) = ledger(&pcm);
        let first = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(&[("a", 1.0, 1.5), ("c", 9.6, 10.0)], (0.0, 10.0));
        ledger.admit_word_slots(&first, &pins);
        assert_eq!(ledger.text_of(&owner), Some("a"));
        assert!(ledger.text_recovery_pending(&owner));
        ledger.record_exhausted_capture(&owner.session, owner.capture_epoch, pcm.len() as u64);
        let terminal = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, owner.clone());
        ledger.admit_word_slots(&terminal, &pins);
        // Ending capture supplies no missing phonemes at a voiced cut.
        assert_eq!(ledger.text_of(&owner), Some("a"));
        assert!(ledger.text_recovery_pending(&owner));
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn a_hold_cannot_create_capture_facts_without_qualification() {
        let pcm = synthetic_take(&[(8.7, 9.3)]);
        let (mut ledger, owner) = ledger(&pcm);
        ledger.evidence.clear();
        let observation = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner);
        let mut pins = window(&[("cut", 8.7, 9.0)], (0.0, 9.0));
        assert!(ledger.hold_edge_cut_pins(&observation, &mut pins).is_none());
        assert!(ledger.live_fence_holds().is_empty());
        assert!(ledger.has_no_capture_facts());
    }

    #[test]
    fn a_fence_at_the_end_of_the_capture_never_holds() {
        // The final word has measured quiet at its fence. Capture exhaustion
        // cannot replace that independent acoustic proof.
        let pcm = synthetic_take(&[(1.0, 1.5), (9.6, 9.97)]);
        let (mut ledger, owner) = ledger(&pcm);
        let live = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(&[("a", 1.0, 1.5), ("c", 9.6, 10.0)], (0.0, 10.0));
        let energy = ledger.decode_fence_energy.as_ref().unwrap().owner.clone();
        let end = pcm.len() as u64;
        let edge = OccurrenceIdentity::new(
            &owner.session,
            owner.capture_epoch,
            end - (SECOND / 50 + 1),
            end,
        );
        ledger.record_decode_fence_energy(
            &energy,
            end,
            [None, Some((&edge, &pcm[edge.sample_start as usize..]))],
        );
        ledger.record_exhausted_capture(&owner.session, owner.capture_epoch, pcm.len() as u64);
        assert!(ledger.admit_word_slots(&live, &pins).grants_mutation());
        assert_eq!(ledger.text_of(&owner), Some("a c"));
        assert!(ledger.live_fence_holds().is_empty());
    }

    #[test]
    fn a_burst_longer_than_the_next_window_can_reach_holds_only_what_it_covers() {
        // Speech from 2.0 s to the fence without a gap. The next live window
        // starts 6 s before the fence (3.0 s): "a" at 2.0 s has no later
        // owner and is delivered, "b" and "c" are held.
        let pcm = synthetic_take(&[(2.0, 9.3)]);
        let (mut ledger, owner) = ledger(&pcm);
        let live = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(
            &[("a", 2.0, 2.5), ("b", 3.5, 4.0), ("c", 8.7, 9.0)],
            (0.0, 9.0),
        );
        assert!(ledger.admit_word_slots(&live, &pins).grants_mutation());
        assert_eq!(ledger.text_of(&owner), Some("a"));
        let holds = ledger.live_fence_holds();
        assert_eq!(holds.len(), 1);
        assert_eq!(
            holds[0]
                .held
                .iter()
                .map(|pin| pin.text.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        assert!(holds[0].burst_start <= 2 * SECOND + SECOND / 10);
    }

    #[test]
    fn only_whisper_windows_with_one_shared_fence_are_measured() {
        let pcm = synthetic_take(&[(1.0, 1.5), (8.7, 9.3)]);
        let (mut ledger, owner) = ledger(&pcm);
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
        let mut pins = window(&[("a", 1.0, 1.5), ("c", 8.7, 9.0)], (0.0, 9.0));
        assert!(ledger.hold_edge_cut_pins(&apple, &mut pins).is_none());
        assert_eq!(pins.len(), 2);
        let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let mut unfenced = vec![WordPin::new(SECOND, SECOND + SECOND / 2, "a")];
        assert!(ledger.hold_edge_cut_pins(&whisper, &mut unfenced).is_none());
        assert_eq!(unfenced.len(), 1);
    }
}
