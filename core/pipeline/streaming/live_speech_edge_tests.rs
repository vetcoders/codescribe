//! Live speech-edge contract: a max-duration split is not silence.
//!
//! The product hands-free threshold stays 5.0 s.

use super::{EpochDecision, EpochGate, SileroIngress};

const RATE: u32 = 16_000;
const FRAME: usize = 512;

#[test]
fn continuous_speech_coverage_survives_maximum_duration_splits() {
    let mut driver = Driver::new();
    for _ in 0..1000 {
        driver.feed(0.9);
    }
    let evidence = driver.ingress.acoustic_speech_evidence();
    let ranges = evidence.ranges();
    let speech_samples: u64 = ranges.iter().map(|r| r.sample_end - r.sample_start).sum();
    assert_eq!(
        speech_samples, driver.seen,
        "a duration split must not end the acoustic speech set: {ranges:?}"
    );
    let voiced_end = driver.seen;
    for _ in 0..200 {
        driver.feed(0.01);
    }
    let evidence = driver.ingress.acoustic_speech_evidence();
    let pad = (crate::pipeline::streaming::silero_fusion::ACOUSTIC_SPEECH_PAD_SECS * RATE as f32)
        .round() as u64;
    assert!(
        evidence.ranges().last().unwrap().sample_end <= voiced_end + FRAME as u64 + pad,
        "real silence must close the speech set instead of swallowing the quiet tail"
    );
}

/// Explicit local corpus acceptance; recordings stay outside Git.
#[test]
#[ignore = "Requires .vibecrafted/fixtures/silero-regression/{quiet,long,silence}.wav"]
fn preserved_capture_silero_ingress_keeps_long_speech() {
    use crate::vad::AccumulatingVad;
    use codescribe_core::audio::{load_audio_file, resample_to_16k};
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".vibecrafted/fixtures/silero-regression");
    for (name, minimum) in [("quiet", 2.0), ("long", 80.0), ("silence", 0.0)] {
        let (raw, rate) = load_audio_file(&fixtures.join(format!("{name}.wav"))).unwrap();
        assert_eq!(rate, 48_000);
        let projection = crate::audio::speech_input::replay_projection_for_test(&raw, 7.943_282);
        assert_eq!(projection.len(), raw.len());
        let mono = resample_to_16k(&projection, rate).unwrap();
        let mut reference = AccumulatingVad::new(RATE).unwrap();
        let positives = mono
            .as_chunks::<FRAME>()
            .0
            .iter()
            .filter(|frame| reference.feed(frame.as_slice()) >= 0.5)
            .count();
        let reference_seconds = positives as f64 * FRAME as f64 / RATE as f64;
        let mut previous = None;
        for partition in [480, 1024] {
            let mut ingress = SileroIngress::new(rate, name, 1);
            let mut seen = 0;
            for (capture, model) in raw.chunks(partition).zip(projection.chunks(partition)) {
                seen += capture.len() as u64;
                ingress.ingest(capture, model, seen);
            }
            let evidence = ingress.acoustic_speech_evidence();
            assert_eq!(ingress.observed_samples(), raw.len() as u64);
            let ranges: Vec<_> = evidence
                .ranges()
                .iter()
                .map(|range| (range.sample_start, range.sample_end))
                .collect();
            let seconds =
                ranges.iter().map(|(start, end)| end - start).sum::<u64>() as f64 / rate as f64;
            assert!(
                seconds >= minimum,
                "{name}: only {seconds}s of speech survived"
            );
            if name == "silence" {
                assert!(ranges.is_empty());
                assert_eq!(reference_seconds, 0.0);
            }
            if let Some(previous) = previous.as_ref() {
                assert_eq!(
                    &ranges, previous,
                    "{name}: capture partition changed speech evidence"
                );
            }
            println!(
                "SILERO_REPLAY {}",
                serde_json::json!({"fixture":name,"partition":partition,
                "samples":raw.len(),"speech_extent_seconds":seconds,"reference_threshold_seconds":reference_seconds,
                "ranges":ranges,"availability":format!("{:?}",evidence.availability())})
            );
            previous = Some(ranges);
        }
    }
}

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
        let ingest = self.ingress.ingest(&pcm, &pcm, self.seen);
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

#[test]
fn retroactive_split_closes_at_reopen_boundary_without_overlapping_pcm() {
    let mut ingress = SileroIngress::new(RATE, "retroactive", 1);
    let mut seen = 0;
    let mut split_cursor = None;
    let probabilities = std::iter::repeat_n(0.90, 340)
        .chain(std::iter::repeat_n(0.05, 6))
        .chain(std::iter::repeat_n(0.90, 100));
    for prob in probabilities {
        ingress.push_scripted_speech_prob_for_test(prob);
        seen += FRAME as u64;
        let observed = ingress.ingest(&[0.2; FRAME], &[0.2; FRAME], seen);
        if !observed.closed.is_empty() {
            split_cursor = Some(seen);
            break;
        }
    }
    let split_cursor = split_cursor.expect("script must force a retroactive split");
    ingress.push_scripted_speech_prob_for_test(0.90);
    seen += FRAME as u64;
    ingress.ingest(&[0.2; FRAME], &[0.2; FRAME], seen);
    let utterances = ingress.ledger().utterances();
    assert_eq!(utterances.len(), 2, "reopen must mint another occurrence");
    let closed_end = utterances[0].range.sample_end;
    let reopen_start = utterances[1].range.sample_start;
    assert!(
        reopen_start < split_cursor,
        "fixture must split in the past"
    );
    assert_eq!(
        closed_end, reopen_start,
        "closed end must be the chunker final boundary"
    );
    assert!(
        utterances
            .windows(2)
            .all(|pair| pair[0].range.sample_end <= pair[1].range.sample_start)
    );
}

#[test]
fn witness_attempts_preserve_the_budget_of_a_distant_target() {
    let mut attempts = Vec::new();
    let targets = [(0, 1), (60, 61)];
    for _ in 0..2 {
        super::record_dictation_witness_attempts(&mut attempts, &targets, 0, 30, 30);
    }
    assert_eq!(attempts, vec![((0, 1), 2)]);
    super::record_dictation_witness_attempts(&mut attempts, &targets[1..], 58, 61, 30);
    assert_eq!(attempts, vec![((0, 1), 2), ((60, 61), 1)]);
}

#[test]
fn witness_attempts_do_not_charge_a_sliver_of_a_later_oversized_target() {
    let mut attempts = Vec::new();
    let targets = [(0, 1), (29, 61)];
    super::record_dictation_witness_attempts(&mut attempts, &targets, 0, 30, 30);
    assert_eq!(attempts, vec![((0, 1), 1)]);
    super::record_dictation_witness_attempts(&mut attempts, &targets[1..], 27, 57, 30);
    assert_eq!(attempts, vec![((0, 1), 1), ((29, 61), 1)]);
}

#[test]
fn witness_attempts_charge_full_targets_and_only_the_leading_oversized_one() {
    let mut attempts = Vec::new();
    super::record_dictation_witness_attempts(&mut attempts, &[(0, 90), (10, 100)], 0, 30, 30);
    assert_eq!(attempts, vec![((0, 90), 1)]);
    super::record_dictation_witness_attempts(&mut attempts, &[(100, 110)], 98, 110, 30);
    assert_eq!(attempts, vec![((0, 90), 1), ((100, 110), 1)]);
}

// div0 2026-10-15..17: 130 live witness lookups failed on only 7 windows, the
// 0..30 s window 85 times. The oldest debt had left the 120 s buffer, the
// window stayed anchored on it, and newer gaps were never asked.
#[test]
fn live_witness_skips_evicted_debt_and_asks_the_retained_gap() {
    let question = super::live_witness_question(&[(0, 2), (130, 134)], 100, 140, 3, 30);
    assert_eq!(question, Some(((127, 137), vec![(130, 134)])));
}

#[test]
fn live_witness_context_never_reaches_before_the_retained_start() {
    let question = super::live_witness_question(&[(101, 104)], 100, 140, 3, 30);
    assert_eq!(question, Some(((100, 107), vec![(101, 104)])));
}

#[test]
fn live_witness_with_only_evicted_debt_asks_nothing() {
    assert_eq!(
        super::live_witness_question(&[(0, 2), (10, 12)], 100, 140, 3, 30),
        None
    );
}

#[test]
fn live_witness_without_eviction_keeps_the_oldest_first_window() {
    let question = super::live_witness_question(&[(0, 1), (60, 61)], 0, 70, 3, 30);
    assert_eq!(question, Some(((0, 30), vec![(0, 1), (60, 61)])));
}

// div0 2026-10-15..17: the terminal budget (4 requests, 90 s) ran out in 53
// takes. In time order it went to the first minutes of a long take.
#[test]
fn terminal_witness_spends_the_budget_on_the_largest_debt_first() {
    let windows = super::terminal_witness_windows(&[(0, 2), (100, 120), (200, 201)], 300, 3, 30);
    assert_eq!(windows, vec![(97, 123), (0, 5), (197, 204)]);
}

#[test]
fn terminal_witness_breaks_debt_ties_in_time_order() {
    let windows = super::terminal_witness_windows(&[(0, 10), (50, 60)], 300, 3, 30);
    assert_eq!(windows, vec![(0, 13), (47, 63)]);
}

#[test]
fn terminal_witness_still_clusters_neighbouring_targets_into_one_window() {
    let windows = super::terminal_witness_windows(&[(0, 5), (8, 10), (60, 61)], 300, 3, 30);
    assert_eq!(windows, vec![(0, 13), (57, 64)]);
}
