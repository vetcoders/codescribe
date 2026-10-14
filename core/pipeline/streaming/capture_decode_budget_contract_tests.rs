//! Integrator-owned owner/queue contracts. Supplied hypotheses test admission,
//! never recognition quality. Requests come from the production capture plan.
use super::*;
use crate::pipeline::acoustic_ledger::WordPin;
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
    TailSegmentGrain, TailTimingQuality,
};

const RATE: u32 = 16_000;

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
    let pcm = vec![0.25; seconds * RATE as usize];
    state.audio.push(&pcm);
    CaptureLevelAccumulator::bound_to(&state.capture_energy).push_samples(&pcm);
    let (tail, jobs) = mpsc::channel(capacity);
    state.tail_patch = Some(tail);
    let (tx, _) = mpsc::unbounded_channel();
    (state, tx, jobs)
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
        .map(|i| WordPin::new(128_000 + i as u64 * 200, 128_160 + i as u64 * 200, "56"))
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
            raw_text: vec!["56"; words].join(" "),
            layer1_baseline: vec!["56"; words].join(" "),
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

fn completion(request: &TailPatchRequest, words: usize, label: &str) -> TailPatchCompletion {
    let segments = (0..words)
        .map(|i| TimedTailSegment {
            confidence: None,
            grain: TailSegmentGrain::Word,
            text: label.into(),
            range: TailSampleRange {
                session: request.provider_request.identity.range.session.clone(),
                capture_epoch: request.provider_request.identity.range.capture_epoch,
                sample_start: 128_000 + i as u64 * 200,
                sample_end: 128_160 + i as u64 * 200,
            },
        })
        .collect::<Vec<_>>();
    let payload = TailProviderPayload {
        identity: request.provider_request.identity.clone(),
        text: vec![label; words].join(" "),
        segments,
        avg_logprob: Some(-0.2),
        compression_ratio: Some(1.1),
        provider_id: TailProviderId::Fake,
        elapsed_ms: 1,
        evidence: TailProviderEvidence {
            segment_grain: TailSegmentGrain::Word,
            source: TailEvidenceSource::Whisper,
            revision: Some("synthetic-capture-owner-proof".into()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: Some(-0.2),
        },
    };
    payload.validate().unwrap();
    TailPatchCompletion {
        submission_sequence: request.submission_sequence,
        utterance_id: request.utterance_id,
        request_identity: Some(request.provider_request.identity.clone()),
        member_occurrences: request.member_occurrences.clone(),
        payload: Some(payload),
    }
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
    let mut outcomes = Vec::new();
    for late in [false, true] {
        let (mut state, tx, mut jobs) = capture(32, 15);
        let mut owner = None;
        if !late {
            owner = Some(register_owner(&mut state, &tx, 40));
        }
        state.pump_capture_windows(&tx);
        let requests = take_requests(&mut jobs);
        assert_eq!(requests.len(), 3);
        for (request, label) in requests.iter().zip(["1286", "999", "1286"]) {
            state.complete_whisper_window(&tx, completion(request, 40, label), 15.0);
        }
        if late {
            owner = Some(register_owner(&mut state, &tx, 40));
            state.pump_capture_windows(&tx);
        }
        let owner = owner.unwrap();
        let trials = take_requests(&mut jobs);
        assert!(trials.len() <= 1, "forty components share a bounded return");
        for trial in &trials {
            let range = &trial.provider_request.identity.range;
            assert!(trial.provider_request.identity.request_id >= (1_u64 << 63));
            assert!(range.sample_start < 128_000 && range.sample_end >= 136_000);
            assert!(range.sample_end - range.sample_start <= 9 * u64::from(RATE));
            trial.provider_request.validate_pcm(&trial.audio).unwrap();
            state.complete_whisper_window(&tx, completion(trial, 40, "1286"), 15.1);
        }
        let before = state.acoustic_ledger.lock().unwrap().word_choices().len();
        for _ in 0..3 {
            state.replay_completed_windows(&tx);
        }
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.word_choices().len(),
            before,
            "replay minted an extra vote"
        );
        assert_eq!(
            ledger.text_of(&owner),
            Some(vec!["1286"; 40].join(" ").as_str())
        );
        assert!(!ledger.has_word_conflicts());
        assert_eq!(
            ledger.slots_of(&owner).unwrap().len(),
            40,
            "fanout stopped at 32"
        );
        assert_eq!(ledger.conservation().residue(), 0);
        outcomes.push(ledger.text_of(&owner).unwrap().to_owned());
        drop(ledger);
        assert_eq!(state.windows_admitted, 3 + trials.len() as u64);
        assert!(
            take_requests(&mut jobs).is_empty(),
            "disputes scheduled extra inference"
        );
        assert_eq!(state.retained_word_decodes.len(), 3 + trials.len());
    }
    assert_eq!(outcomes[0], outcomes[1]);
}
