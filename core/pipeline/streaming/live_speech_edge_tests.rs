//! Live speech-edge contract: a max-duration split is not silence.
//!
//! The product hands-free threshold stays 5.0 s.

use super::{EpochDecision, EpochGate, SileroIngress};

const RATE: u32 = 16_000;
const FRAME: usize = 512;

struct Driver {
    ingress: SileroIngress,
    gate: EpochGate,
    seen: u64,
    false_run: u64,
    max_false_run: u64,
    sleeps: Vec<u64>,
}

impl Driver {
    fn new() -> Self {
        Self {
            ingress: SileroIngress::new(RATE, "live-edge", 1),
            gate: EpochGate::armed(RATE, 5.0),
            seen: 0,
            false_run: 0,
            max_false_run: 0,
            sleeps: Vec::new(),
        }
    }

    fn feed(&mut self, prob: f32) {
        self.ingress.push_scripted_speech_prob_for_test(prob);
        let pcm = vec![0.2_f32; FRAME];
        self.seen += FRAME as u64;
        let ingest = self.ingress.ingest(&pcm, self.seen);
        if ingest.speech_live {
            self.false_run = 0;
        } else {
            self.false_run += FRAME as u64;
            self.max_false_run = self.max_false_run.max(self.false_run);
        }
        if matches!(
            self.gate.feed_pcm(&pcm, self.seen, ingest.speech_live),
            EpochDecision::Sleep { .. }
        ) {
            self.sleeps.push(self.seen);
        }
    }
}

/// While voiced PCM continues past a forced max-duration boundary, `speech_live`
/// stays true and [`EpochGate`] does not answer [`EpochDecision::Sleep`].
/// A length split may close an utterance. It is not a silence edge.
/// Real silence past Silero's hysteresis still lets the 5.0 s gate sleep.
#[test]
fn continuous_voiced_pcm_past_the_max_split_stays_live_and_the_epoch_stays_open() {
    let mut voiced = Vec::new();
    // Onset, a dip shorter than the 0.55 s iterator silence (it arms prev_end
    // without closing), speech held past the 12 s ceiling, one frame inside
    // the hysteresis band (above 0.35, at or below 0.5), then more speech.
    voiced.extend(std::iter::repeat_n(0.90_f32, 20));
    voiced.extend(std::iter::repeat_n(0.05, 6));
    voiced.extend(std::iter::repeat_n(0.90, 400));
    voiced.push(0.45);
    voiced.extend(std::iter::repeat_n(0.90, 200));

    let mut driver = Driver::new();
    for prob in voiced {
        driver.feed(prob);
    }
    let max_false_run = driver.max_false_run;
    assert!(
        driver.sleeps.is_empty(),
        "EpochGate slept at samples {:?} while voiced PCM continued; \
         longest speech_live=false run was {max_false_run} samples ({:.3} s)",
        driver.sleeps,
        max_false_run as f32 / RATE as f32
    );
    assert_eq!(
        max_false_run,
        0,
        "speech_live went false for {max_false_run} samples ({:.3} s) while voiced PCM continued",
        max_false_run as f32 / RATE as f32
    );

    let voiced_end = driver.seen;
    for _ in 0..(30 + 170) {
        driver.feed(0.01);
    }
    assert!(
        !driver.sleeps.is_empty(),
        "real silence must still sleep the epoch"
    );
    assert!(
        driver.sleeps[0] > voiced_end,
        "sleep must land in the silence tail, not in the voiced span"
    );
}
