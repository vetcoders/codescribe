//! Integrator-owned owner/queue contracts. Supplied hypotheses test admission,
//! never recognition quality. Requests come from the production capture plan.
use super::*;
use crate::pipeline::acoustic_ledger::WordPin;
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
    TailSegmentGrain, TailTimingQuality,
};

const RATE: u32 = 16_000;
/// Calibrated existence floor of the capture device; the take is 0.25 RMS.
const EXISTENCE_FLOOR_RMS: f32 = 0.01;

fn capture(
    capacity: usize,
    seconds: usize,
) -> (
    AppleSealState,
    mpsc::UnboundedSender<EngineEvent>,
    mpsc::Receiver<TailPatchRequest>,
) {
    let mut state = AppleSealState::new_for_session(RATE, "capture-owner-proof".into(), 4);
    state.energy_calibration = Some(EnergyCalibration::new("capture-owner-proof", 1.0, 1));
    state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
    let pcm = vec![0.25; seconds * RATE as usize];
    state.audio.push(&pcm);
    CaptureLevelAccumulator::bound_to(&state.capture_energy).push_samples(&pcm);
    let (tail, jobs) = mpsc::channel(capacity);
    state.tail_patch = Some(tail);
    let (tx, _) = mpsc::unbounded_channel();
    (state, tx, jobs)
}

/// Component `i` of a numeric run that starts at `first`. Components are
/// distinct words: identical neighbours on one span are one occurrence unless
/// each has its own voiced segment (D4), which a 10 ms component cannot have.
fn numeral(first: u64, i: usize) -> String {
    (first + i as u64).to_string()
}

fn numerals(first: u64, words: usize) -> String {
    (0..words)
        .map(|i| numeral(first, i))
        .collect::<Vec<_>>()
        .join(" ")
}

fn register_owner(
    state: &mut AppleSealState,
    tx: &mpsc::UnboundedSender<EngineEvent>,
    words: usize,
) -> OccurrenceIdentity {
    let owner = OccurrenceIdentity::new(
        state.session_id.clone(),
        state.capture_epoch,
        128_000,
        136_000,
    );
    assert!(qualify_owned_occurrence(state, &owner));
    let pins = (0..words)
        .map(|i| {
            WordPin::new(
                128_000 + i as u64 * 200,
                128_160 + i as u64 * 200,
                numeral(56, i),
            )
        })
        .collect::<Vec<_>>();
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        let observation =
            ledger.next_word_observation(LedgerObservationProducer::Apple, 100, &owner);
        assert!(
            ledger
                .admit_word_slots(&observation, &pins)
                .grants_mutation()
        );
    }
    state.pending_events.insert(
        100,
        PendingAppleSeal {
            occurrence: owner.clone(),
            raw_text: numerals(56, words),
            layer1_baseline: numerals(56, words),
            start_ts: 8.0,
            end_ts: 8.5,
            segments: Vec::new(),
        },
    );
    assert!(state.register_whisper_owner(tx, 100, &owner));
    owner
}

fn take_requests(jobs: &mut mpsc::Receiver<TailPatchRequest>) -> Vec<TailPatchRequest> {
    std::iter::from_fn(|| jobs.try_recv().ok()).collect()
}

#[test]
fn capture_owner_connects_the_plan_to_seven_exact_requests_at_stop() {
    let (mut state, tx, mut jobs) = capture(32, 25);
    register_owner(&mut state, &tx, 1);
    state.pump_capture_windows(&tx);
    assert_eq!(state.windows_admitted, 6);
    state.capture_stopping = true;
    state.pump_capture_windows(&tx);
    let requests = take_requests(&mut jobs);
    assert_eq!(requests.len(), 7);
    assert_eq!(state.windows_admitted, 7);
    assert!(state.window_plan.is_finished());
    for (i, job) in requests.iter().enumerate() {
        let frame = &job.provider_request.identity.range;
        assert_eq!(frame.sample_start, i as u64 * 3 * u64::from(RATE));
        assert_eq!(
            frame.sample_end,
            ((i * 3 + 9) * RATE as usize).min(25 * RATE as usize) as u64
        );
        job.provider_request.validate_pcm(&job.audio).unwrap();
        assert!(job.audio.iter().all(|sample| *sample == 0.25));
    }
    for sample in 0..25 * u64::from(RATE) {
        let visits = requests
            .iter()
            .filter(|job| {
                let range = &job.provider_request.identity.range;
                range.sample_start <= sample && sample < range.sample_end
            })
            .count();
        assert!(
            (1..=3).contains(&visits),
            "sample {sample}: {visits} visits"
        );
    }
}

#[test]
fn accepted_failed_work_is_consumed_once_without_stop_recovery_requests() {
    let (mut state, tx, mut jobs) = capture(32, 25);
    state.capture_stopping = true;
    state.pump_capture_windows(&tx);
    for request in take_requests(&mut jobs) {
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity),
                member_occurrences: request.member_occurrences,
                payload: None,
            },
            25.0,
        );
    }
    state.pump_capture_windows(&tx);
    assert!(take_requests(&mut jobs).is_empty());
    assert_eq!(state.windows_admitted, 7);
    assert_eq!(state.tail_patch_jobs_skipped, 7);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(state.window_plan.is_finished());
}

#[test]
fn backpressure_keeps_one_exact_unsent_offer_without_advancing_it() {
    let (mut state, tx, mut jobs) = capture(1, 15);
    state.pump_capture_windows(&tx);
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(state.refinement_pending.len(), 1);
    let held = state
        .refinement_pending
        .front()
        .unwrap()
        .provider_request
        .identity
        .clone();
    state.audio.push(&vec![0.5; RATE as usize * 3]);
    for _ in 0..5 {
        state.pump_capture_windows(&tx);
    }
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(state.refinement_pending.len(), 1);
    assert_eq!(
        state
            .refinement_pending
            .front()
            .unwrap()
            .provider_request
            .identity,
        held
    );
    jobs.try_recv().unwrap();
    state.pump_capture_windows(&tx);
    let submitted = jobs.try_recv().unwrap();
    assert_eq!(submitted.provider_request.identity, held);
    assert_eq!(
        (held.range.sample_start, held.range.sample_end),
        (48_000, 192_000)
    );
    assert!(submitted.audio.iter().all(|sample| *sample == 0.25));
}

#[test]
fn replay_and_early_owners_share_grid_and_at_most_one_bounded_numeric_witness() {
    // Forty distinct supplied word pins on separate, measured PCM bursts.
    // These labels exercise fanout and ownership, not number recognition WER.
    let positions = (0..40_u64)
        .map(|i| (1600 + i * 2240, 2880 + i * 2240))
        .collect::<Vec<_>>();
    let mut outcomes = Vec::new();
    for late in [false, true] {
        let mut state = AppleSealState::new_for_session(RATE, "capture-owner-proof".into(), 4);
        state.energy_calibration = Some(EnergyCalibration::new("numeric-pin-geometry", 1.0, 1));
        state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
        let mut pcm = vec![0.00001; 15 * RATE as usize];
        for &(start, end) in &positions {
            pcm[start as usize..end as usize].fill(0.25);
        }
        // A different future burst keeps all three ordinary grid jobs useful.
        pcm[232000..236000].fill(0.25);
        state.audio.push(&pcm);
        CaptureLevelAccumulator::bound_to(&state.capture_energy).push_samples(&pcm);
        let (tail, mut jobs) = mpsc::channel(32);
        state.tail_patch = Some(tail);
        let (tx, _receiver) = mpsc::unbounded_channel();
        let register = |state: &mut AppleSealState| {
            let owner =
                OccurrenceIdentity::new(state.session_id.clone(), state.capture_epoch, 0, 96000);
            assert!(qualify_owned_occurrence(state, &owner));
            let pins = positions
                .iter()
                .enumerate()
                .map(|(i, &(start, end))| WordPin::new(start, end, numeral(56, i)))
                .collect::<Vec<_>>();
            {
                let mut ledger = state.acoustic_ledger.lock().unwrap();
                state.bind_apple_word_measurement(&mut ledger, &owner);
                let observation =
                    ledger.next_word_observation(LedgerObservationProducer::Apple, 100, &owner);
                assert!(
                    ledger
                        .admit_word_slots(&observation, &pins)
                        .grants_mutation()
                );
            }
            state.pending_events.insert(
                100,
                PendingAppleSeal {
                    occurrence: owner.clone(),
                    raw_text: numerals(56, 40),
                    layer1_baseline: numerals(56, 40),
                    start_ts: 0.0,
                    end_ts: 6.0,
                    segments: Vec::new(),
                },
            );
            assert!(state.register_whisper_owner(&tx, 100, &owner));
            owner
        };
        let mut owner = if late {
            None
        } else {
            Some(register(&mut state))
        };
        state.pump_capture_windows(&tx);
        let requests = take_requests(&mut jobs);
        assert_eq!(requests.len(), 3);
        for (index, request) in requests.iter().enumerate() {
            let frame = &request.provider_request.identity.range;
            let first = if index == 0 { 1286 } else { 999 };
            let segments = positions
                .iter()
                .enumerate()
                .filter(|(_, position)| {
                    frame.sample_start <= position.0 && position.1 <= frame.sample_end
                })
                .map(|(i, &(start, end))| TimedTailSegment {
                    confidence: None,
                    grain: TailSegmentGrain::Word,
                    text: numeral(first, i),
                    range: TailSampleRange {
                        session: frame.session.clone(),
                        capture_epoch: frame.capture_epoch,
                        sample_start: start,
                        sample_end: end,
                    },
                })
                .collect::<Vec<_>>();
            let grain = if segments.is_empty() {
                TailSegmentGrain::Phrase
            } else {
                TailSegmentGrain::Word
            };
            let payload = TailProviderPayload {
                identity: request.provider_request.identity.clone(),
                text: segments
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                segments,
                avg_logprob: Some(-0.2),
                compression_ratio: Some(1.1),
                provider_id: TailProviderId::Fake,
                elapsed_ms: 1,
                evidence: TailProviderEvidence {
                    segment_grain: grain,
                    source: TailEvidenceSource::Whisper,
                    revision: Some("synthetic-distinct-numeric-pins".into()),
                    stability: TailEvidenceStability::Final,
                    timing_quality: TailTimingQuality::ExactSampleRange,
                    avg_logprob: Some(-0.2),
                    vad_selected_samples: None,
                },
            };
            payload.validate().unwrap();
            state.complete_whisper_window(
                &tx,
                TailPatchCompletion {
                    submission_sequence: request.submission_sequence,
                    utterance_id: request.utterance_id,
                    request_identity: Some(request.provider_request.identity.clone()),
                    payload: Some(payload),
                    member_occurrences: request.member_occurrences.clone(),
                },
                15.0,
            );
        }
        if late {
            owner = Some(register(&mut state));
            state.pump_capture_windows(&tx);
        }
        let owner = owner.unwrap();
        assert!(
            take_requests(&mut jobs).is_empty(),
            "no extra numeric witness decode"
        );
        let before = state.acoustic_ledger.lock().unwrap().word_choices().len();
        for _ in 0..3 {
            state.replay_completed_windows(&tx);
        }
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.word_choices().len(),
            before,
            "replay cannot mint another vote"
        );
        assert_eq!(
            ledger.text_of(&owner),
            Some(numerals(1286, 40).as_str()),
            "late={late}; choices={:?}; alternatives={:?}",
            ledger.word_choices(),
            ledger.slot_alternatives()
        );
        assert!(!ledger.has_word_conflicts());
        assert_eq!(
            ledger.slots_of(&owner).unwrap().len(),
            40,
            "fanout cannot stop at 32"
        );
        assert_eq!(ledger.conservation().residue(), 0);
        outcomes.push(ledger.text_of(&owner).unwrap().to_owned());
        drop(ledger);
        assert_eq!(state.windows_admitted, 3);
        assert_eq!(state.retained_word_decodes.len(), 3);
    }
    assert_eq!(outcomes[0], outcomes[1]);
}
