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
