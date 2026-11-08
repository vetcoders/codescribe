/// Layer 1 contracts lifted out of the never-compiled `mod tests`.
///
/// Each function keeps its parked name. The rows that were strict subsets of a
/// running test stay on that test. Scenarios that read `progressive` or
/// `SealedSpan` described a canvas this session no longer has.
use super::*;
use crate::stt::tail_patcher::compute_tail_patch;
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailTimingQuality,
};
use std::sync::Mutex;

const TEST_SAMPLE_RATE: u32 = 16_000;

fn synthetic_tail_payload(request_id: u64, range: TailSampleRange) -> TailProviderPayload {
    TailProviderPayload {
        identity: TailRequestIdentity { request_id, range },
        text: String::new(),
        segments: Vec::new(),
        avg_logprob: None,
        compression_ratio: None,
        provider_id: crate::stt::tail_provider::TailProviderId::Fake,
        elapsed_ms: 0,
        evidence: TailProviderEvidence {
            segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            source: TailEvidenceSource::Whisper,
            revision: Some("synthetic-test".to_string()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::Synthetic,
            avg_logprob: None,
            vad_selected_samples: None,
        },
    }
}

fn synthetic_tail_job(utterance_id: u64, outcome: TailPatchOutcome) -> TailPatchJobResult {
    let range = TailSampleRange {
        session: "test-session".to_string(),
        capture_epoch: 0,
        sample_start: 0,
        sample_end: 16_000,
    };
    TailPatchJobResult {
        utterance_id,
        outcome,
        payload: synthetic_tail_payload(utterance_id, range),
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<EngineEvent>>);

impl EventSink for RecordingSink {
    fn on_event(&self, event: &EngineEvent) {
        self.0.lock().expect("lock").push(event.clone());
    }
}

impl RecordingSink {
    fn events(&self) -> Vec<EngineEvent> {
        self.0.lock().expect("lock").clone()
    }
}

/// A bounded stop that abandons accepted refinement must be observable on
/// the ordered event surface; zero abandoned work stays quiet.
#[test]
fn tail_patch_drain_degrade_is_typed_once_and_zero_is_silent() {
    let sink = RecordingSink::default();
    report_tail_patch_drain_degrade(&sink, 0);
    assert!(sink.events().is_empty());

    report_tail_patch_drain_degrade(&sink, 2);
    emit_session_finalised(
        &sink,
        "test-session".to_string(),
        0,
        SessionConservationReceipt::default(),
    );
    let events = sink.events();
    assert_eq!(events.len(), 2);
    let EngineEvent::Warning { code, message } = &events[0] else {
        panic!("expected typed Warning, got {:?}", events[0]);
    };
    assert_eq!(code, TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE);
    assert!(message.contains('2'));
    assert!(message.contains("Apple live text was preserved"));
    assert!(matches!(events[1], EngineEvent::SessionFinalised { .. }));
}

/// Settings/runtime degradation keeps a stable, separately actionable
/// warning code that carries the disposition and no transcript text.
#[test]
fn local_tail_patch_degraded_warning_is_a_typed_event() {
    let sink = RecordingSink::default();
    emit_local_tail_patch_degraded_warning(&sink, "degraded_invalid_override");

    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        EngineEvent::Warning { code, message }
            if code == LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE
                && message == "degraded_invalid_override"
    ));
}

fn stage_pending_occurrence(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    occurrence: OccurrenceIdentity,
    label: &str,
) {
    let calibration = EnergyCalibration {
        version: "c13-test".to_string(),
        min_energy_integral: 1.0,
        min_valley_samples: 1,
    };
    let evidence = AcousticEvidence {
        occurrence: occurrence.clone(),
        duration_ms: 1_000.0,
        energy_integral: 10.0,
        mean_rms_dbfs: -12.0,
        peak_dbfs: -3.0,
        vad_open_sample: Some(occurrence.sample_start),
        vad_close_sample: Some(occurrence.sample_end),
        evidence_calibration_version: calibration.version.clone(),
    };
    state
        .acoustic_ledger
        .lock()
        .expect("ledger")
        .qualify(&evidence, &calibration);
    state.energy_calibration = Some(calibration);
    assert!(
        admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    utterance_id,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .is_some()
    );
    state.pending_events.insert(
        utterance_id,
        PendingAppleSeal {
            occurrence,
            raw_text: label.to_string(),
            layer1_baseline: label.to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
        },
    );
}

fn launch_whisper_and_return_lexicon(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    occurrence: &OccurrenceIdentity,
    label: &str,
) {
    assert!(
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .schedule_observer(occurrence.clone(), LedgerObservationProducer::Whisper)
    );
    assert!(
        admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    utterance_id,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .is_some()
    );
}

/// Missing audio never launches Whisper. Queue loss and stop-time timeout
/// return already-launched slots; none can strand an occurrence frontier.
#[test]
fn no_window_queue_rejection_and_timeout_conserve_whisper_frontiers() {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let (not_launched_tx, mut not_launched_rx) = mpsc::channel::<TailPatchRequest>(1);
    let mut no_window =
        AppleSealState::new_for_session(TEST_SAMPLE_RATE, "no-window".to_string(), 1);
    no_window.tail_patch = Some(not_launched_tx);
    let absent = OccurrenceIdentity::new("no-window", 1, 0, 16_000);
    stage_pending_occurrence(&mut no_window, &tx, 1, absent.clone(), "Iwo");
    assert!(
        no_window
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .frontier_of(&absent)
            .expect("frontier")
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper),
        "an armed lane reserves Whisper before its PCM lookup"
    );
    // The capture existed but its first half has left bounded retention.
    // A finished physical plan can return the reservation; zero capture
    // head cannot honestly expire an occurrence ending at sample 16_000.
    no_window.audio = LiveAudioBuffer::new(TEST_SAMPLE_RATE, 0.5);
    let pcm = vec![0.5; 16_000];
    let mut capture = CaptureLevelAccumulator::bound_to(&no_window.capture_energy);
    for chunk in pcm.chunks(320) {
        capture.push_samples(chunk);
    }
    no_window.audio.push(&pcm);
    assert_eq!(no_window.audio.session_sample_end(), absent.sample_end);
    assert!(no_window.window_by_samples(0, absent.sample_end).is_none());
    no_window.capture_stopping = true;
    no_window.pump_capture_windows(&tx);
    assert!(no_window.window_plan.is_finished());
    assert_eq!(no_window.tail_patch_awaiting_completion(), 0);
    assert!(not_launched_rx.try_recv().is_err());
    let _ = admit_ledger_label(
        &mut no_window,
        &tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Lexicon,
                1,
                0,
                absent.clone(),
            ),
            label: "Iwo",
            energy: EnergyAdmission::RequireExistingQualification,
        },
    );
    {
        let ledger = no_window.acoustic_ledger.lock().expect("ledger");
        assert!(ledger.frontier_of(&absent).unwrap().is_closed());
        assert_eq!(ledger.text_of(&absent), Some("Iwo"));
        assert!(ledger.text_recovery_pending(&absent));
        assert!(
            !ledger.is_sealed(&absent),
            "evicted PCM cannot prove recovery"
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }
    while rx.try_recv().is_ok() {}

    let (tail_tx, tail_rx) = mpsc::channel::<TailPatchRequest>(1);
    drop(tail_rx);
    let mut rejected =
        AppleSealState::new_for_session(TEST_SAMPLE_RATE, "queue-rejected".to_string(), 1);
    rejected.tail_patch = Some(tail_tx);
    let occurrence = OccurrenceIdentity::new("queue-rejected", 1, 0, 16_000);
    stage_pending_occurrence(&mut rejected, &tx, 1, occurrence.clone(), "Iwo");
    super::c13a_lifecycle_tests::place_pcm_through(&mut rejected, 32_000);
    let _ = admit_ledger_label(
        &mut rejected,
        &tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Lexicon,
                1,
                0,
                occurrence.clone(),
            ),
            label: "Iwo",
            energy: EnergyAdmission::RequireExistingQualification,
        },
    );
    // Admission succeeds before transport observes the closed receiver.
    // The stopping partial is the whole 2 s head and finishes the plan.
    assert!(super::c13a_lifecycle_tests::drive_capture_plan(
        &mut rejected,
        &tx,
        true
    ));
    assert!(rejected.refinement_lane_lost);
    assert_eq!(rejected.tail_patch_awaiting_completion(), 0);
    assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|event| {
        matches!(event, EngineEvent::Warning { code, .. }
                if code == RefinementFailure::LaneGone.code())
    }));
    // A terminal queue refusal releases the reserved producer immediately.
    assert!(
        rejected
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .is_sealed(&occurrence)
    );
    rejected.return_outstanding_whisper_without_label(&tx);
    assert!(
        rejected
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .is_sealed(&occurrence)
    );

    while rx.try_recv().is_ok() {}
    let mut timed_out =
        AppleSealState::new_for_session(TEST_SAMPLE_RATE, "timed-out".to_string(), 1);
    let timed = OccurrenceIdentity::new("timed-out", 1, 0, 16_000);
    stage_pending_occurrence(&mut timed_out, &tx, 1, timed.clone(), "Iwo");
    launch_whisper_and_return_lexicon(&mut timed_out, &tx, 1, &timed, "Iwo");
    let (tail_tx, mut tail_rx) = mpsc::channel(1);
    timed_out.tail_patch = Some(tail_tx);
    super::c13a_lifecycle_tests::place_pcm_through(&mut timed_out, 32_000);
    let offered =
        super::c13a_lifecycle_tests::offer_capture_windows(&mut timed_out, &tx, &mut tail_rx, true);
    assert_eq!(offered.len(), 1, "the stopping partial is the held job");
    assert!(
        offered[0]
            .member_occurrences
            .iter()
            .any(|(_, owner)| owner == &timed)
    );
    assert_eq!(timed_out.tail_patch_awaiting_completion(), 1);
    assert!(
        timed_out.pending_events.contains_key(&1),
        "a held seal is not the job counter"
    );
    timed_out.return_outstanding_whisper_without_label(&tx);
    assert_eq!(timed_out.tail_patch_awaiting_completion(), 0);
    assert!(
        timed_out
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .is_sealed(&timed)
    );
}

/// Mixed worker buckets, plus a worker that returns no accounting.
///
/// Timeout-only ownership is
/// `local_power_stop_deadline_preserves_clock_residue_and_disposition`.
/// The one-job shortfall is `tail_patch_receipt_names_missing_terminal_evidence`.
#[test]
fn tail_patch_receipt_uses_worker_adjudicated_job_buckets() {
    let receipt = tail_patch_receipt_after_stop(
        true,
        3,
        Some(TailPatchWorkerAccounting {
            applied_jobs: 1,
            skipped_jobs: 1,
            timeout_residue: 1,
        }),
        SessionConservationReceipt::default(),
    );
    assert_eq!(receipt.applied, 1);
    assert_eq!(receipt.skipped, 1);
    assert_eq!(receipt.timed_out, 1);
    assert_eq!(receipt.abandoned, 0);
    assert!(receipt.is_reconciled());

    let worker_failed =
        tail_patch_receipt_after_stop(true, 2, None, SessionConservationReceipt::default());
    assert_eq!(worker_failed.abandoned, 2);
    assert_eq!(worker_failed.drain, TailPatchDrainDisposition::Abandoned);
    assert!(worker_failed.is_reconciled());
}

fn inflight(
    submission_sequence: u64,
    utterance_id: u64,
    sample_start: u64,
    sample_end: u64,
) -> TailPatchInFlight {
    TailPatchInFlight {
        submission_sequence,
        utterance_id,
        request_identity: TailRequestIdentity {
            request_id: utterance_id,
            range: TailSampleRange {
                session: "test-session".to_string(),
                capture_epoch: 0,
                sample_start,
                sample_end,
            },
        },
        admit_sample_start: sample_start,
        admit_sample_end: sample_end,
        member_occurrences: vec![(
            utterance_id,
            OccurrenceIdentity::new("test-session", 0, sample_start, sample_end),
        )],
    }
}

/// Computing a bearing patch is not delivery. The seal owner receives the
/// request identity and the provider payload, and a dropped worker channel
/// refuses that handoff.
#[test]
fn finishing_tail_patch_only_hands_identity_to_the_seal_owner() {
    let mut lane = AppleTailPatchLane::new(
        TEST_SAMPLE_RATE,
        None,
        crate::stt::tail_provider::TailProviderId::InProcess,
    );
    let outcome = compute_tail_patch(
        "ala ma kota w domu",
        "ala ma kota w domu swoim",
        1,
        &TailPatchConfig::default(),
    );
    assert!(
        outcome
            .events()
            .iter()
            .any(|event| matches!(event, EngineEvent::ReplaceRange { .. })),
        "fixture must carry a bearing patch"
    );
    let completion = lane.finish_for_worker(
        Some(inflight(1, 1, 0, 16_000)),
        Ok(synthetic_tail_job(1, outcome)),
    );
    assert!(
        completion.payload.is_some(),
        "a completed job forwards its provider payload to the seal owner"
    );
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    assert!(lane.forward_completion_to_worker(&done_tx, completion));
    let accepted = done_rx.try_recv().expect("live worker receives completion");
    assert!(accepted.request_identity.is_some());
    assert_eq!(
        accepted.member_occurrences,
        vec![(1, OccurrenceIdentity::new("test-session", 0, 0, 16_000))],
        "worker completion preserves the exact member occurrence"
    );

    drop(done_rx);
    let rejected_outcome = compute_tail_patch(
        "drugi fragment",
        "drugi fragment odzyskany",
        2,
        &TailPatchConfig::default(),
    );
    let rejected = lane.finish_for_worker(
        Some(inflight(2, 2, 16_000, 32_000)),
        Ok(synthetic_tail_job(2, rejected_outcome)),
    );
    assert!(!lane.forward_completion_to_worker(&done_tx, rejected));
}
