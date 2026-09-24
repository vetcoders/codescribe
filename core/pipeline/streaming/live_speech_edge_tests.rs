//! Live speech-edge contract: a max-duration split is not silence.
//!
//! The product hands-free threshold stays 5.0 s.

use super::{
    AcousticLedger, AppleSealState, EnergyCalibration, EpochDecision, EpochGate, EngineEvent,
    SileroIngress, TailPatchRequest, seal_sliced_by_silero,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

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

/// L1 observes the PCM clock while a long utterance is still open. Apple may
/// have produced no words at all; neither an Apple final nor a Silero close is
/// a prerequisite for the first two overlapping Whisper requests.
///
/// This drives the PCM side of the production worker in its actual order. It
/// is a synthetic scheduling contract, not proof of recognition on real audio.
#[test]
fn layer1_observes_open_speech_before_silero_close() {
    let (events, _event_rx) = mpsc::unbounded_channel::<EngineEvent>();
    let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(8);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        RATE,
        "live-l1-cadence".into(),
        1,
        tail_tx,
        Arc::new(Mutex::new(AcousticLedger::new())),
        Some(EnergyCalibration {
            version: "live-cadence-fixture".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        }),
    );
    state.fusion = Some(SileroIngress::new(RATE, "live-l1-cadence", 1));
    state.fusion_seal_armed = true;

    let mut sample_end = 0_u64;
    let mut launched = Vec::new();
    // 250 x 512 samples = eight seconds, below Silero's max-duration split.
    // The scripted VAD is an explicit speech witness; no Apple callback is
    // submitted, so any L1 job is independent of an Apple text buffer.
    for _ in 0..250 {
        // The production worker ticks pending refinements before receiving
        // the next PCM chunk, then retains and ingests that chunk.
        state.tick_refinements(&events, Instant::now());
        let pcm = [0.2_f32; FRAME];
        sample_end += FRAME as u64;
        state.audio.push(&pcm);
        let (speech_live, speech_evidence) = {
            let fusion = state.fusion.as_mut().expect("speech witness");
            fusion.push_scripted_speech_prob_for_test(0.90);
            let ingress = fusion.ingest(&pcm, sample_end);
            (ingress.speech_live, fusion.acoustic_speech_evidence())
        };
        state
            .speech_progress
            .observe_speech(&speech_evidence, speech_live, RATE);
        seal_sliced_by_silero(&mut state, &events, &[]);
        while let Ok(request) = tail_rx.try_recv() {
            request
                .provider_request
                .validate_pcm(&request.audio)
                .expect("L1 request names the exact captured PCM it carries");
            launched.push(request);
        }
    }
    state.tick_refinements(&events, Instant::now());
    while let Ok(request) = tail_rx.try_recv() {
        request
            .provider_request
            .validate_pcm(&request.audio)
            .expect("L1 request names the exact captured PCM it carries");
        launched.push(request);
    }

    let utterances = state
        .fusion
        .as_ref()
        .expect("speech witness")
        .ledger()
        .utterances();
    assert!(!utterances.is_empty(), "scripted Silero never opened speech");
    assert!(
        utterances.iter().all(|utterance| !utterance.closed),
        "the fixture must keep speech open; a closed utterance would hide the scheduler coupling"
    );
    assert!(
        launched.len() >= 2,
        "L1 must launch overlapping PCM observations at about 4 s and 7 s, before Silero closes; launched {}",
        launched.len()
    );
    let first = &launched[0].provider_request.identity.range;
    let second = &launched[1].provider_request.identity.range;
    let first_len = first.sample_end.saturating_sub(first.sample_start);
    let overlap = first.sample_end.saturating_sub(second.sample_start);
    assert!(
        (3 * RATE as u64..=5 * RATE as u64).contains(&first_len),
        "first L1 window is not about four seconds: {first_len} samples"
    );
    assert!(
        (RATE as u64 / 2..=RATE as u64 * 3 / 2).contains(&overlap),
        "adjacent L1 windows should share about one second: {overlap} samples"
    );
}
