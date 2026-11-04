/// rc-w1-live-ledger: acoustic speech coverage, occurrence-safe decode context,
/// and the diagnostic-versus-ledger receipt.
///
/// These falsifiers run beside the session, on the production seal functions.
use super::*;
use crate::audio::capture_receipt::{AcousticAvailability, CAPTURE_ENERGY_PRODUCER};
use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

const RATE: u32 = 16_000;
/// `ACOUSTIC_SPEECH_PAD_SECS` (64 ms) at [`RATE`].
const PAD: u64 = 1_024;
/// `SEAL_COVERAGE_INCOMPLETE_MS` (250 ms) at [`RATE`].
const THRESHOLD: u64 = 4_000;

fn at(secs: f32) -> u64 {
    (secs * RATE as f32) as u64
}

fn push_capture(state: &mut AppleSealState, secs: f32) {
    let total = (secs * RATE as f32) as usize;
    let session = vec![0.25f32; total];
    for chunk in session.chunks(1024) {
        state.audio.push(chunk);
    }
}

fn crossing(kind: VadBoundaryKind, sample: u64) -> VadBoundaryEvidence {
    VadBoundaryEvidence {
        kind,
        sample,
        speech_probability: match kind {
            VadBoundaryKind::SpeechStart => 0.9,
            VadBoundaryKind::SpeechEnd => 0.1,
        },
    }
}

/// The producer decision: the narrowest observer that actually measured
/// speech wins, padded ownership windows are never a candidate, and when no
/// observer measured, the answer carries its own unavailability instead of
/// an empty set that reads as silence.
#[test]
fn coverage_producer_picks_a_measuring_observer_and_never_ownership_windows() {
    // Ownership windows spanning the whole take, no crossings, no ladder.
    let mut state = AppleSealState::new_for_session(RATE, "rc-w3-producer".into(), 0);
    push_capture(&mut state, 10.0);
    let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
    ingress
        .ledger_mut()
        .open_or_extend(&state.session_id, 0, 0, at(10.0));
    ingress.ledger_mut().close_open(at(10.0));
    state.fusion = Some(ingress);
    assert_eq!(
        sum_range_samples(&fusion_utterance_ranges(&state)),
        at(10.0),
        "ownership keeps its padded window; this test is about who may use it"
    );

    let evidence = coverage_speech_evidence(&state);
    assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
    assert_eq!(
        evidence.availability(),
        AcousticAvailability::NotObserved,
        "padded ownership is not an acoustic measurement, and nothing else measured"
    );
    assert!(evidence.ranges().is_empty());

    // The capture energy ladder measured the take: it becomes the observer.
    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    writer.push_samples(&vec![0.25f32; at(1.0) as usize]);
    writer.push_samples(&vec![0.0f32; at(9.0) as usize]);
    let measured = coverage_speech_evidence(&state);
    assert_eq!(measured.producer(), CAPTURE_ENERGY_PRODUCER);
    assert!(measured.observed_speech());
    assert_eq!(measured.ranges().len(), 1);
    assert_eq!(measured.ranges()[0].sample_end, at(1.0));

    // Silero crossings are narrower still and outrank the ladder.
    let fusion = state.fusion.as_mut().unwrap();
    fusion.ingest(
        &vec![0.25f32; at(10.0) as usize],
        &vec![0.25f32; at(10.0) as usize],
        at(10.0),
    );
    fusion.observe_boundaries(&[
        crossing(VadBoundaryKind::SpeechStart, at(2.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(3.0)),
    ]);
    let acoustic = coverage_speech_evidence(&state);
    assert_eq!(acoustic.producer(), SILERO_BOUNDARIES_PRODUCER);
    assert_eq!(acoustic.ranges().len(), 1);
    assert_eq!(acoustic.ranges()[0].sample_start, at(2.0) - PAD);
}

/// Take 09ededb1: the energy ladder heard -70 dB crumbs, Silero heard
/// nothing, and every Whisper window's own VAD selected nothing over them.
/// That take is measured silence and ends as "no speech detected". Each
/// missing witness, or one that heard speech, keeps the ladder and its
/// refusal, so speech never turns into silence.
#[test]
fn loudness_is_silence_only_when_both_speech_passes_measured_none() {
    let take = |name: &str| {
        let mut state = AppleSealState::new_for_session(RATE, name.into(), 0);
        push_capture(&mut state, 10.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.0f32; at(2.0) as usize]);
        writer.push_samples(&vec![0.25f32; at(1.0) as usize]);
        writer.push_samples(&vec![0.0f32; at(7.0) as usize]);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress.ingest(
            &vec![0.0f32; at(10.0) as usize],
            &vec![0.0f32; at(10.0) as usize],
            at(10.0),
        );
        state.fusion = Some(ingress);
        state
    };
    let quiet_windows = [(0, at(5.0), false), (at(4.0), at(10.0), false)];

    let unmeasured = take("rc-w3-loud-unmeasured");
    let evidence = coverage_speech_evidence(&unmeasured);
    assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
    assert!(
        evidence.observed_speech(),
        "without a tail VAD verdict the ladder stays the observer"
    );

    let mut silent = take("rc-w3-loud-silent");
    silent.tail_vad_windows.extend(quiet_windows);
    let evidence = coverage_speech_evidence(&silent);
    assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
    assert!(evidence.ranges().is_empty());
    let mut ledger = AcousticLedger::new();
    let receipt = ledger.assess_seal_coverage(&silent.session_id, 0, &evidence, 0);
    ledger.record_seal_coverage(receipt);
    assert!(
        matches!(
            ledger.terminal_finality(&silent.session_id, 0),
            crate::pipeline::acoustic_ledger::TerminalFinality::ObservedSilence(_)
        ),
        "two measured speech passes over the loud crumbs are no speech detected"
    );

    let mut heard = take("rc-w3-loud-heard");
    heard.tail_vad_windows.extend(quiet_windows);
    heard.tail_vad_windows.push((at(2.0), at(4.0), true));
    assert_eq!(
        coverage_speech_evidence(&heard).producer(),
        CAPTURE_ENERGY_PRODUCER,
        "one window whose VAD selected speech over the loud range keeps it speech"
    );

    let mut uncovered = take("rc-w3-loud-uncovered");
    uncovered.tail_vad_windows.push((0, at(2.5), false));
    assert_eq!(
        coverage_speech_evidence(&uncovered).producer(),
        CAPTURE_ENERGY_PRODUCER,
        "loudness outside every measured window keeps the ladder"
    );

    let mut apple = take("rc-w3-loud-apple");
    apple.tail_vad_windows.extend(quiet_windows);
    apple.last_apple_segment_end = 2.5;
    assert_eq!(
        coverage_speech_evidence(&apple).producer(),
        CAPTURE_ENERGY_PRODUCER,
        "a recognizer that produced a word rules out measured silence"
    );

    // Quiet speech: the ladder is loud and Silero caught an utterance.
    let mut quiet_speech = take("rc-w3-loud-quiet-speech");
    quiet_speech.tail_vad_windows.extend(quiet_windows);
    quiet_speech.fusion.as_mut().unwrap().observe_boundaries(&[
        crossing(VadBoundaryKind::SpeechStart, at(2.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(3.0)),
    ]);
    let evidence = coverage_speech_evidence(&quiet_speech);
    assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
    assert!(evidence.observed_speech(), "caught speech is never silence");
}

/// Ownership padding cannot raise the acoustic speech figure, and a real
/// measured gap still refuses. Both halves in one witness, because the
/// failure this cut exists for was the padded set passing as coverage.
#[test]
fn ownership_padding_cannot_increase_measured_speech() {
    let mut state = AppleSealState::new_for_session(RATE, "rc-w3-padding".into(), 0);
    push_capture(&mut state, 10.0);
    let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
    ingress.ingest(
        &vec![0.25f32; at(10.0) as usize],
        &vec![0.25f32; at(10.0) as usize],
        at(10.0),
    );
    ingress.observe_boundaries(&[
        crossing(VadBoundaryKind::SpeechStart, at(1.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
    ]);
    // The ownership window is the whole take; the crossing is one second.
    ingress
        .ledger_mut()
        .open_or_extend(&state.session_id, 0, 0, at(10.0));
    ingress.ledger_mut().close_open(at(10.0));
    state.fusion = Some(ingress);

    let evidence = coverage_speech_evidence(&state);
    assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
    assert_eq!(
        sum_range_samples(evidence.ranges()),
        at(1.0) + 2 * PAD,
        "the acoustic figure is the crossing plus its 64 ms margin, not the window"
    );
    assert!(
        sum_range_samples(evidence.ranges()) < sum_range_samples(&fusion_utterance_ranges(&state)),
        "ownership padding may not raise measured speech"
    );

    // Nothing is committed, so the measured second is an uncovered gap.
    let receipt = {
        let ledger = state.acoustic_ledger.lock().unwrap();
        ledger.assess_seal_coverage(&state.session_id, 0, &evidence, THRESHOLD)
    };
    assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
    assert_eq!(receipt.max_uncovered_samples, at(1.0) + 2 * PAD);
}

/// The regression this cut exists for. A padded ownership window spanning a
/// whole take must not be counted as speech; the crossings inside it are.
///
/// Shape taken from an archived 50-minute take: one utterance window that
/// stayed open for its whole length reported 99.5% of the recording as
/// speech, against an offline Silero measurement of 454 s.
#[test]
fn coverage_speech_set_measures_crossings_not_the_padded_ownership_window() {
    let mut state = AppleSealState::new_for_session(RATE, "rc-w1-coverage".into(), 0);
    push_capture(&mut state, 10.0);

    let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
    // Two seconds of real speech inside a ten-second take.
    ingress.observe_boundaries(&[
        crossing(VadBoundaryKind::SpeechStart, at(1.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        crossing(VadBoundaryKind::SpeechStart, at(8.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(9.0)),
    ]);
    // One padded ownership window covering the entire take, exactly as the
    // fusion ledger legitimately mints it.
    ingress.observe(Some((0, at(10.0))), true, at(10.0));
    ingress.note_observed_pcm(at(10.0), at(10.0));
    state.fusion = Some(ingress);

    let evidence = coverage_speech_evidence(&state);
    assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
    let speech = evidence.ranges();
    assert_eq!(speech.len(), 2, "two bursts, not one window");
    assert_eq!(speech[0].session, "rc-w1-coverage");

    let acoustic_samples = sum_range_samples(speech);
    let utterance_samples = sum_range_samples(&fusion_utterance_ranges(&state));
    assert_eq!(utterance_samples, at(10.0), "ownership keeps its window");
    // Two 1 s bursts, each padded 64 ms on both sides.
    assert_eq!(acoustic_samples, 2 * (at(1.0) + 2 * 1_024));
    assert!(
        acoustic_samples * 4 < utterance_samples,
        "the coverage set must shrink to the speech, not to the take"
    );
}

/// Without a fusion ingress at all, the capture energy ladder still answers.
/// This lane may not remove the fallback that keeps a VAD-less take
/// measurable — but the ladder must have actually measured to answer.
#[test]
fn coverage_falls_back_to_capture_energy_without_a_fusion_ingress() {
    let mut state = AppleSealState::new_for_session(RATE, "rc-w1-no-vad".into(), 0);
    push_capture(&mut state, 4.0);
    assert!(state.fusion.is_none());

    let unmeasured = coverage_speech_evidence(&state);
    assert_eq!(unmeasured.producer(), CAPTURE_ENERGY_PRODUCER);
    assert_eq!(
        unmeasured.availability(),
        AcousticAvailability::NotObserved,
        "a ladder nobody fed cannot answer for a VAD-less take"
    );

    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    writer.push_samples(&vec![0.25f32; at(4.0) as usize]);
    let measured = coverage_speech_evidence(&state);
    assert_eq!(measured.producer(), CAPTURE_ENERGY_PRODUCER);
    assert!(measured.observed_speech());
}

/// An open crossing at stop is speech up to the capture cursor, and the
/// coverage set may not claim audio past what was captured.
#[test]
fn coverage_speech_set_clamps_an_open_crossing_at_end_of_capture() {
    let mut state = AppleSealState::new_for_session(RATE, "rc-w1-eof".into(), 0);
    push_capture(&mut state, 3.0);

    let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
    ingress.observe_boundaries(&[crossing(VadBoundaryKind::SpeechStart, at(2.0))]);
    ingress.note_observed_pcm(at(3.0), at(3.0));
    state.fusion = Some(ingress);

    let evidence = coverage_speech_evidence(&state);
    assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
    let speech = evidence.ranges();
    assert_eq!(speech.len(), 1);
    assert_eq!(speech[0].sample_start, at(2.0) - 1_024);
    assert_eq!(
        speech[0].sample_end,
        at(3.0),
        "the right pad stops at the last captured sample"
    );
}

/// A word shorter than any proposed minimum-utterance figure is still
/// speech the receipt must account for. Nothing on this lane may quietly
/// drop it.
#[test]
fn coverage_speech_set_keeps_a_short_word() {
    let mut state = AppleSealState::new_for_session(RATE, "rc-w1-short".into(), 0);
    push_capture(&mut state, 5.0);

    let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
    // 80 ms of speech.
    ingress.observe_boundaries(&[
        crossing(VadBoundaryKind::SpeechStart, at(1.0)),
        crossing(VadBoundaryKind::SpeechEnd, at(1.0) + 1_280),
    ]);
    ingress.note_observed_pcm(at(5.0), at(5.0));
    state.fusion = Some(ingress);

    let evidence = coverage_speech_evidence(&state);
    let speech = evidence.ranges();
    assert_eq!(speech.len(), 1, "a short word is not discarded");
    assert_eq!(speech[0].sample_start, at(1.0) - 1_024);
    assert_eq!(speech[0].sample_end, at(1.0) + 1_280 + 1_024);
}

/// The disputed labels were admitted, not lost. A `skipped`
/// char-diff verdict is diagnostics; the payload it "skipped" is forwarded
/// to occurrence admission all the same, and the receipt says both.
#[test]
fn a_legacy_skip_verdict_does_not_discard_the_forwarded_payload() {
    // The exact shape of the three disputed jobs: the char-diff clamped on
    // change ratio, and every one of those payloads still reached the
    // ledger and persisted to the Bus.
    let skipped = legacy_char_diff_receipt(
        &TailPatchOutcome::skipped(SkipReasonCode::ChangeRatio, "ratio 1.31 exceeds max 0.50"),
        true,
    );
    assert_eq!(skipped.verdict, "skipped");
    assert!(
        skipped.payload_forwarded,
        "a skipped verdict must not read as a discarded payload"
    );
    assert_eq!(skipped.admission_authority, LEDGER_ADMISSION_AUTHORITY);

    for (outcome, verdict) in [
        (TailPatchOutcome::NoChange, "no_change"),
        (
            TailPatchOutcome::skipped(SkipReasonCode::ChangeRatio, "ratio 1.90"),
            "skipped",
        ),
        (TailPatchOutcome::Patches(Vec::new()), "patches"),
    ] {
        let receipt = legacy_char_diff_receipt(&outcome, true);
        assert_eq!(receipt.verdict, verdict);
        assert!(
            receipt.payload_forwarded,
            "every completed job forwards its payload regardless of verdict"
        );
        assert_ne!(
            receipt.admission_authority, receipt.verdict,
            "the verdict is never the admission authority"
        );
    }

    // A failed job is the one case with no payload, and it says so.
    let failed = legacy_char_diff_receipt(&TailPatchOutcome::NoChange, false);
    assert!(!failed.payload_forwarded);
}
