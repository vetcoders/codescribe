//! Live-first hold at a decode fence (plan §4, 13 X 2026).
//!
//! A live Whisper window ends at a decode fence the decoder did not choose
//! acoustically. When the capture writer's own hops show speech running
//! through that fence, the words sitting on that open burst are not words
//! yet: the decoder completed clipped phonemes into plausible tokens ("pumpy
//! to" for "Computer Use." on take 36de5e98, build 953). Those pins are held
//! as alternatives with text recovery pending; the next window that covers
//! the burst with a complete fence owns it. Nothing waits for Stop: a window
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
}

const SPEECH_ABOVE_FLOOR_DB: f32 = 8.0;
const BRIDGED_GAP_MS: u64 = 60;
const MIN_HOPS_FOR_FLOOR: usize = 50;

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
        if fence <= window_start || energy.captured_samples <= fence {
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
        let burst_start = self.open_burst_start_at_fence(owner, window_start, fence)?;
        let (held, kept): (Vec<WordPin>, Vec<WordPin>) =
            pins.drain(..).partition(|pin| pin.sample_end > burst_start);
        *pins = kept;
        if held.is_empty() {
            return None;
        }
        for pin in &held {
            self.retain_slot_alternative(observation, &pin.text, Vec::new(), EDGE_CUT_OPEN_BURST);
        }
        self.require_text_recovery(owner);
        let receipt = LiveFenceHoldReceipt {
            observation: observation.clone(),
            window_start,
            fence,
            burst_start,
            held,
        };
        self.live_fence_holds.push(receipt.clone());
        Some(receipt)
    }

    pub fn live_fence_holds(&self) -> &[LiveFenceHoldReceipt] {
        &self.live_fence_holds
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
        assert!(!ledger.text_recovery_pending(&owner));
    }

    #[test]
    fn a_fence_at_the_end_of_the_capture_never_holds() {
        // Speech runs to the very end of the PCM: there is no later window,
        // so the last words must reach delivery.
        let pcm = synthetic_take(&[(1.0, 1.5), (9.6, 10.0)]);
        let (mut ledger, owner) = ledger(&pcm);
        let live = ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        let pins = window(&[("a", 1.0, 1.5), ("c", 9.6, 10.0)], (0.0, 10.0));
        assert!(ledger.admit_word_slots(&live, &pins).grants_mutation());
        assert_eq!(ledger.text_of(&owner), Some("a c"));
        assert!(ledger.live_fence_holds().is_empty());
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
