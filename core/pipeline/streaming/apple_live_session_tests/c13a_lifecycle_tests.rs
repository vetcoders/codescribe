/// C13/C13A lifecycle falsifiers kept active without reviving the stale legacy
/// Apple test canvas below.
use super::*;

const TEST_SAMPLE_RATE: u32 = 16_000;

fn consultation_owner() -> (
    LiveConsultationCapture,
    mpsc::Receiver<LiveConsultationRequest>,
    std_mpsc::Sender<LiveConsultationReturn>,
) {
    let (requests, rx) = mpsc::channel(CONSULTATION_QUEUE_CAP);
    let (tx, returns) = std_mpsc::channel();
    (
        LiveConsultationCapture {
            queue: ConsultationInputQueue::new("max-capture".into(), 1).unwrap(),
            requests,
            returns,
            last_assessed: None,
            assessment_pending: false,
            answers_pending: 0,
            speech_open: false,
            refused: false,
        },
        rx,
        tx,
    )
}

#[test]
fn max_capture_uses_closed_edges_not_the_speech_live_bit_or_silence() {
    use super::super::silero_fusion::SileroIngest;
    let (mut owner, mut requests, _returns) = consultation_owner();
    owner.observe(&SileroIngest::default(), 16_000).unwrap();
    assert_eq!(
        owner.queue.pending_groups(),
        0,
        "silence cannot nominate speech"
    );
    owner
        .observe(
            &SileroIngest {
                open: Some(1),
                speech_live: true,
                ..Default::default()
            },
            32_000,
        )
        .unwrap();
    assert!(owner.speech_open);
    assert_eq!(owner.queue.pending_groups(), 0);
    owner
        .observe(
            &SileroIngest {
                closed: vec![1],
                speech_live: true,
                ..Default::default()
            },
            64_000,
        )
        .unwrap();
    assert!(
        !owner.speech_open,
        "the closing chunk still carries speech_live"
    );
    assert_eq!(owner.queue.pending_groups(), 1);
    owner.observe(&SileroIngest::default(), 72_000).unwrap();
    assert_eq!(
        owner.queue.pending_groups(),
        1,
        "quiet ticks keep one candidate"
    );
    owner
        .observe(
            &SileroIngest {
                open: Some(2),
                speech_live: true,
                ..Default::default()
            },
            80_000,
        )
        .unwrap();
    assert_eq!(
        owner.queue.pending_groups(),
        0,
        "continuation invalidates assessment input"
    );
    owner
        .observe(
            &SileroIngest {
                closed: vec![2],
                speech_live: true,
                ..Default::default()
            },
            96_000,
        )
        .unwrap();
    assert_eq!(owner.queue.pending_groups(), 1);
    assert!(
        requests.try_recv().is_err(),
        "edges alone cannot request semantic assessment"
    );
    assert!(!owner.assessment_pending);
}

#[test]
fn max_stop_refuses_disconnected_return_owner_without_waiting_forever() {
    let (mut owner, _requests, returns) = consultation_owner();
    owner.assessment_pending = true;
    owner.answers_pending = 1;
    drop(returns);
    let state = state_for_session("max-capture");
    let (events, mut observed) = mpsc::unbounded_channel();
    owner.settle(&state, &events, 16_000);
    assert!(owner.refused);
    assert!(!owner.assessment_pending);
    assert_eq!(owner.answers_pending, 0);
    assert!(
        matches!(observed.try_recv().unwrap(), EngineEvent::Warning { code, .. }
            if code == "max_consultation_refused")
    );
    assert!(observed.try_recv().is_err());
    assert_eq!(
        owner.queue.pending_groups(),
        1,
        "lost transport cannot acknowledge source"
    );
}

#[test]
fn max_async_errors_return_to_capture_without_fabricating_publication() {
    let (tx, rx) = std_mpsc::channel();
    let sink = crate::pipeline::sinks::CollectorEventSink::new();
    deliver_consultation_result(
        LiveConsultationResult::Assessed(Err(anyhow::anyhow!("assessment"))),
        &tx,
        &sink,
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        LiveConsultationReturn::Assessed(Err(_))
    ));
    deliver_consultation_result(
        LiveConsultationResult::Prepared(Err(anyhow::anyhow!("disk"))),
        &tx,
        &sink,
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        LiveConsultationReturn::Prepared(Err(_))
    ));
    deliver_consultation_result(
        LiveConsultationResult::Answer(Err(anyhow::anyhow!("execution"))),
        &tx,
        &sink,
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        LiveConsultationReturn::Published(Err(_))
    ));
    assert!(sink.events().is_empty());
}

fn state_for_session(session_id: &str) -> AppleSealState {
    AppleSealState::new_for_session(TEST_SAMPLE_RATE, session_id.to_string(), 1)
}

#[test]
fn every_untimed_phrase_final_emits_a_typed_refusal_for_its_preview_revision() {
    use crate::pipeline::contracts::PreviewFinalDisposition;
    let mut state = state_for_session("phrase-disposition");
    state.fusion_seal_armed = true;
    let (tx, mut rx) = mpsc::unbounded_channel();
    for rev in 1..=2 {
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "Iwo".into(),
                    segments: Vec::new(),
                },
                LiveStreamEvent::PhraseFinal {
                    text: "Iwo".into(),
                    segments: Vec::new(),
                },
            ],
            &tx,
            &mut state,
            1.0,
        );
        let mut dispositions = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::PreviewDisposition {
                superseded_through_rev,
                final_disposition,
                refused_evidence,
            } = event
            {
                assert_eq!(refused_evidence.len(), 1);
                assert_eq!(refused_evidence[0].range, None);
                assert_eq!(refused_evidence[0].text, "Iwo");
                assert_eq!(refused_evidence[0].reason, "apple_final_without_pcm_timing");
                dispositions.push((superseded_through_rev, final_disposition));
            }
        }
        assert_eq!(
            dispositions,
            vec![(
                rev,
                PreviewFinalDisposition::Refused {
                    reason: "apple_final_without_pcm_timing".into(),
                }
            )]
        );
    }
    emit_stream_events(
        vec![LiveStreamEvent::PhraseFinal {
            text: "another final without a preview".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        1.0,
    );
    while let Ok(event) = rx.try_recv() {
        if let EngineEvent::PreviewDisposition {
            superseded_through_rev,
            ..
        } = event
        {
            assert_eq!(
                superseded_through_rev, 0,
                "a final without a current preview cannot claim an earlier phrase"
            );
        }
    }
}

#[test]
fn preserved_apple_final_is_admitted_without_replaying_its_label() {
    use crate::pipeline::contracts::PreviewFinalDisposition;
    let occurrence = OccurrenceIdentity {
        session: "preserved-phrase".into(),
        capture_epoch: 1,
        sample_start: 0,
        sample_end: 16_000,
    };
    let event = EngineEvent::LedgerMutation {
        observation: LedgerObservationIdentity::new(
            LedgerObservationProducer::Apple,
            1,
            1,
            occurrence.clone(),
        ),
        label: "already committed".into(),
        receipt: MutationReceipt::Preserve {
            occurrence,
            held_by: LedgerObservationProducer::Apple,
        },
    };
    let (disposition, evidence) = phrase_final_disposition(&[event], "already committed");
    assert_eq!(disposition, PreviewFinalDisposition::Admitted);
    assert!(evidence.is_empty());
}

#[test]
fn partially_refused_phrase_retains_only_each_refused_apple_slice() {
    use crate::pipeline::acoustic_ledger::RefuseReason;
    use crate::pipeline::contracts::{PreviewFinalDisposition, RefusedPreviewEvidence};
    let admitted = OccurrenceIdentity {
        session: "partial-refusal".into(),
        capture_epoch: 1,
        sample_start: 0,
        sample_end: 16_000,
    };
    let refused = OccurrenceIdentity {
        sample_start: 16_000,
        sample_end: 32_000,
        ..admitted.clone()
    };
    let second_refused = OccurrenceIdentity {
        sample_start: 32_000,
        sample_end: 48_000,
        ..admitted.clone()
    };
    let mut events = vec![EngineEvent::LedgerMutation {
        observation: LedgerObservationIdentity::new(
            LedgerObservationProducer::Apple,
            1,
            1,
            admitted.clone(),
        ),
        label: "admitted".into(),
        receipt: MutationReceipt::Insert {
            occurrence: admitted,
        },
    }];
    for (generation, occurrence) in [refused.clone(), second_refused.clone()]
        .into_iter()
        .enumerate()
    {
        events.push(EngineEvent::LedgerMutation {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                1,
                generation as u64 + 2,
                occurrence.clone(),
            ),
            label: "Iwo".into(),
            receipt: MutationReceipt::Refuse {
                occurrence,
                reason: RefuseReason::SealedReplay,
            },
        });
    }
    let (disposition, evidence) = phrase_final_disposition(&events, "admitted Iwo Iwo");
    let reason = RefuseReason::SealedReplay.as_str().to_string();
    assert_eq!(
        disposition,
        PreviewFinalDisposition::Refused {
            reason: reason.clone()
        }
    );
    assert_eq!(
        evidence,
        vec![
            RefusedPreviewEvidence {
                range: Some(refused),
                text: "Iwo".into(),
                reason: reason.clone()
            },
            RefusedPreviewEvidence {
                range: Some(second_refused),
                text: "Iwo".into(),
                reason
            },
        ]
    );
}

#[test]
fn final_without_visible_receipts_does_not_invent_refused_text() {
    use crate::pipeline::contracts::PreviewFinalDisposition;
    let (disposition, evidence) = phrase_final_disposition(&[], "final text without receipt");
    assert_eq!(disposition, PreviewFinalDisposition::Admitted);
    assert!(evidence.is_empty());
}

fn stage_pending_occurrence(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    occurrence: OccurrenceIdentity,
    label: &str,
) {
    let calibration = EnergyCalibration {
        version: "c13a-test".to_string(),
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

/// Current-API seam for sibling modules in this file.
/// Extends the capture buffer through `sample_end` with the same 0.5
/// amplitude the retired piece fixture used. Does not plan or admit.
pub(super) fn place_pcm_through(state: &mut AppleSealState, sample_end: u64) {
    let have = state.audio.session_sample_end();
    if have < sample_end {
        state.audio.push(&vec![0.5; (sample_end - have) as usize]);
    }
}

/// Arms `capture_stopping`, runs `pump_capture_windows`, and drains every
/// offer the lane accepted. Callers set `state.tail_patch` first so the
/// first Apple admission reserves Whisper.
pub(super) fn offer_capture_windows(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    tail_rx: &mut mpsc::Receiver<TailPatchRequest>,
    stopping: bool,
) -> Vec<TailPatchRequest> {
    state.capture_stopping = stopping;
    state.pump_capture_windows(ev_tx);
    let mut offered = Vec::new();
    while let Ok(request) = tail_rx.try_recv() {
        offered.push(request);
    }
    offered
}

/// Dropped-receiver pump. True when the plan, the lane, or the unresolved
/// count moved. A later pump on a finished plan returns false.
pub(super) fn drive_capture_plan(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    stopping: bool,
) -> bool {
    let before_admitted = state.windows_admitted;
    let before_horizon = state.window_plan.admission_horizon();
    let before_lost = state.refinement_lane_lost;
    let before_unresolved = state.unresolved_windows;
    let before_finished = state.window_plan.is_finished();
    state.capture_stopping = stopping;
    state.pump_capture_windows(ev_tx);
    state.windows_admitted != before_admitted
        || state.window_plan.admission_horizon() != before_horizon
        || state.refinement_lane_lost != before_lost
        || state.unresolved_windows != before_unresolved
        || state.window_plan.is_finished() != before_finished
}

fn return_lexicon(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    occurrence: &OccurrenceIdentity,
) {
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
                label: "Iwo",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .is_some()
    );
    state.emit_pending_seal(ev_tx, utterance_id);
}

fn no_payload_completion(request: &TailPatchRequest) -> TailPatchCompletion {
    TailPatchCompletion {
        submission_sequence: request.submission_sequence,
        utterance_id: request.utterance_id,
        request_identity: Some(request.provider_request.identity.clone()),
        payload: None,
        member_occurrences: request.member_occurrences.clone(),
    }
}

fn ai_result(status: AiFormatStatus, text: &str) -> AiFormatResult {
    AiFormatResult {
        text: text.to_string(),
        reasoning_text: None,
        status,
    }
}

/// W4-T14 regression reconstructed from the build-755 take. The Apple
/// occurrence already owns the phrase before sample 837632. A whole-pass
/// Whisper segment that repeats those words crosses that boundary, while
/// the novel continuation is pinned wholly inside the uncovered range.
/// Only the latter may become a document occurrence.
#[test]
fn real_take_gap_repair_rejects_straddled_repeat_and_keeps_novel_pcm_segment() {
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailTimingQuality,
    };

    const CAPTURE_END: usize = 1_082_880;
    let session = "walkaround-755-gap-repro";
    let (tx, mut event_rx) = mpsc::unbounded_channel();
    let mut state = state_for_session(session);
    // The capture reaches the energy owner with the buffer, on a device
    // calibrated to a 0.01 RMS existence floor; the take is 0.25 RMS.
    let pcm = vec![0.25; CAPTURE_END];
    state.existence_floor_rms = Some(0.01);
    state.audio.push(&pcm);
    CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, TEST_SAMPLE_RATE)
        .push_samples(&pcm);

    let apple_occurrence = OccurrenceIdentity::new(session, 1, 694_272, 837_632);
    stage_pending_occurrence(
        &mut state,
        &tx,
        4,
        apple_occurrence,
        "Bo generalnie jest to dużo skuteczniejsze jeśli takie rzeczy są oczywiste w tym przypadku",
    );
    let gap = TailSampleRange {
        session: session.to_string(),
        capture_epoch: 1,
        sample_start: 837_632,
        sample_end: 1_082_880,
    };
    let payload = TailProviderPayload {
            identity: TailRequestIdentity {
                request_id: u64::MAX,
                range: TailSampleRange {
                    session: session.to_string(),
                    capture_epoch: 1,
                    sample_start: 694_272,
                    sample_end: 1_082_880,
                },
            },
            text: "Bo generalnie jest to dużo skuteczniejsze jeśli takie rzeczy są oczywiste w tym przypadku miały jakąś kanwę falsyfikacji"
                .to_string(),
            segments: vec![
                TimedTailSegment {
                    confidence: None,
                    grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                    text: "takie rzeczy są oczywiste w tym przypadku".to_string(),
                    range: TailSampleRange {
                        session: session.to_string(),
                        capture_epoch: 1,
                        sample_start: 800_000,
                        sample_end: 880_000,
                    },
                },
                TimedTailSegment {
                    confidence: None,
                    grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                    text: "miały jakąś kanwę falsyfikacji".to_string(),
                    range: TailSampleRange {
                        session: session.to_string(),
                        capture_epoch: 1,
                        sample_start: 900_000,
                        sample_end: 1_080_000,
                    },
                },
            ],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.0),
            provider_id: TailProviderId::AppleDictation,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: TailEvidenceSource::AppleDictation,
                revision: Some("walkaround-755".to_string()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        };
    payload.validate().expect("repro payload must be valid");

    assert_eq!(
        admit_full_pass_gap_segments(
            &mut state,
            &tx,
            &payload,
            &[gap],
            4_000,
            EnergyAdmission::QualifyFinalPassGap,
        ),
        1
    );

    let rendered = state
        .acoustic_ledger
        .lock()
        .expect("ledger")
        .rendered_text();
    assert_eq!(rendered.matches("takie rzeczy").count(), 1, "{rendered}");
    assert!(rendered.ends_with("miały jakąś kanwę falsyfikacji"));

    let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        EngineEvent::Warning { code, .. }
            if code == "seal_coverage_full_pass_segment_straddles_gap"
    )));
    assert!(events.iter().all(|event| !matches!(
        event,
        EngineEvent::LedgerMutation { label, .. }
            if label == "takie rzeczy są oczywiste w tym przypadku"
    )));
}

#[test]
fn formatter_configuration_without_transport_ownership_never_opens_a_frontier() {
    assert!(!formatter_lane_is_armed(FormattingPolicy::Off, true));
    assert!(!formatter_lane_is_armed(
        FormattingPolicy::Correction,
        false
    ));
    assert!(formatter_lane_is_armed(FormattingPolicy::Correction, true));

    // Keep a healthy receiver alive: Max must refuse this lane because of
    // its policy, not because the transport happens to be unavailable.
    let (max_tx, mut max_rx) = mpsc::channel(1);
    let max_armed =
        live_formatter_lane_is_armed(CaptureTurnIntent::HandsFree, FormattingPolicy::Max, || true);
    let max_formatter = max_armed.then_some(max_tx);
    for (session, formatter) in {
        let (closed_tx, closed_rx) = mpsc::channel(1);
        drop(closed_rx);
        [
            ("formatter-disabled", None),
            ("formatter-closed", Some(closed_tx)),
            ("max-grouped-consultation", max_formatter),
        ]
    } {
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        let mut state = state_for_session(session);
        state.formatter = formatter;
        let occurrence = OccurrenceIdentity::new(session, 1, 0, 16_000);
        stage_pending_occurrence(&mut state, &ev_tx, 1, occurrence.clone(), "Iwo");
        return_lexicon(&mut state, &ev_tx, 1, &occurrence);

        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert!(ledger.is_sealed(&occurrence));
        assert!(
            ledger
                .frontier_of(&occurrence)
                .expect("frontier")
                .open_producers()
                .is_empty(),
        );
        assert!(state.formatter_requested.is_empty());
    }
    assert!(
        max_rx.try_recv().is_err(),
        "Max must not enqueue an occurrence job"
    );
}

#[test]
fn formatter_results_map_to_typed_occurrence_dispositions() {
    let occurrence = OccurrenceIdentity::new("formatter-map", 7, 160, 320);
    let cases = [
        (
            AiFormatStatus::Applied,
            "Sformatowane Iwo",
            LabelProposalDisposition::Propose,
            "Sformatowane Iwo",
        ),
        (
            AiFormatStatus::AiNoop,
            "Iwo",
            LabelProposalDisposition::PreserveExisting,
            "",
        ),
        (
            AiFormatStatus::Skipped,
            "Iwo",
            LabelProposalDisposition::PreserveExisting,
            "",
        ),
        (
            AiFormatStatus::Failed,
            "Iwo",
            LabelProposalDisposition::Refuse,
            "",
        ),
        (
            AiFormatStatus::Applied,
            "   ",
            LabelProposalDisposition::Refuse,
            "",
        ),
    ];

    for (status, text, disposition, proposed_label) in cases {
        let completion = FormatterCompletion::from_result(
            FormatterRequest {
                occurrence: occurrence.clone(),
                existing_label: "Iwo".to_string(),
                source_observation: None,
                policy: FormattingPolicy::Smart,
            },
            ai_result(status, text),
        );
        assert!(completion.carries_same_occurrence());
        assert_eq!(completion.proposal.disposition, disposition);
        assert_eq!(completion.proposal.proposed_label, proposed_label);
    }
}

#[test]
fn formatter_transport_failure_after_raw_seal_keeps_lane_and_terminal_seal_alive() {
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let mut state = state_for_session("formatter-transport-failure");
    state.formatter = Some(formatter_tx);
    let first = OccurrenceIdentity::new("formatter-transport-failure", 1, 0, 16_000);

    stage_pending_occurrence(&mut state, &ev_tx, 1, first.clone(), "Surowe zdanie");
    return_lexicon(&mut state, &ev_tx, 1, &first);
    let request = formatter_rx.try_recv().expect("exact formatter request");
    let completion = FormatterCompletion::from_result(
        request,
        // Deterministic provider/transport refusal stub: this is the same
        // typed outcome produced after a 401 or an exhausted HTTP failure.
        ai_result(AiFormatStatus::Failed, "Surowe zdanie"),
    );

    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&first));
    assert!(state.pending_events.is_empty());
    let sink = crate::pipeline::sinks::CollectorEventSink::new();
    assert!(deliver_formatter_completion(completion, &sink, None));
    assert!(
        matches!(sink.events().as_slice(), [EngineEvent::OccurrenceLabelProposal { proposal }]
            if proposal.disposition == LabelProposalDisposition::Refuse)
    );
    assert_eq!(state.formatter_requested.len(), 1);

    // A later utterance proves that the live lane remained usable after
    // the formatter failure instead of terminating at the acknowledgement.
    state.formatter = None;
    let second = OccurrenceIdentity::new("formatter-transport-failure", 1, 16_000, 32_000);
    stage_pending_occurrence(&mut state, &ev_tx, 2, second.clone(), "Dalszy surowy tekst");
    return_lexicon(&mut state, &ev_tx, 2, &second);

    let terminal = state
        .acoustic_ledger
        .lock()
        .expect("ledger")
        .seal_terminal(&state.session_id, state.capture_epoch)
        .expect("formatter refusal must not block the terminal ledger seal");
    assert_eq!(terminal.sealed_occurrences, vec![first, second]);
    let finals = std::iter::from_fn(|| ev_rx.try_recv().ok())
        .filter_map(|event| match event {
            EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                raw_text,
                ..
            } => Some((utterance_id, text, raw_text)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        finals,
        vec![
            (1, "Surowe zdanie".to_string(), "Surowe zdanie".to_string(),),
            (
                2,
                "Dalszy surowy tekst".to_string(),
                "Dalszy surowy tekst".to_string(),
            ),
        ],
    );
}

#[test]
fn formatter_waits_for_ledger_word_trial_disposition() {
    use crate::pipeline::acoustic_ledger::WordPin;
    for resolved in [false, true] {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
        let mut state = state_for_session("formatter-settled-input");
        state.capture_epoch = 1;
        state.formatter = Some(formatter_tx);
        let owner = OccurrenceIdentity::new(state.session_id.clone(), 1, 0, 160_000);
        let trial = {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            let calibration = EnergyCalibration::new("formatter-ready", 1.0, 1);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: owner.clone(),
                            duration_ms: 10_000.0,
                            energy_integral: 100.0,
                            mean_rms_dbfs: -20.0,
                            peak_dbfs: -10.0,
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(160_000),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(owner.clone(), [LedgerObservationProducer::Whisper]);
            ledger.admit_word_slots(
                &LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    1,
                    0,
                    owner.clone(),
                ),
                &[WordPin::new(48_000, 64_000, "56")],
            );
            ledger.admit_word_slots(
                &LedgerObservationIdentity::new(
                    LedgerObservationProducer::Whisper,
                    1,
                    1,
                    owner.clone(),
                ),
                &[WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 128_000)],
            );
            ledger
                .next_word_trial(true)
                .expect("actual lexical conflict")
        };
        state.finish_whisper_frontier(&ev_tx, &owner);
        assert!(
            formatter_rx.try_recv().is_err(),
            "formatter must never adjudicate an open lexical dispute"
        );
        assert!(state.formatter_requested.is_empty());
        if resolved {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(ledger.word_trial_has_decode(
                &trial,
                &OccurrenceIdentity::new(&owner.session, owner.capture_epoch, 0, 128_000),
            ));
            assert!(!ledger.word_trial_has_decode(
                &trial,
                &OccurrenceIdentity::new(&owner.session, owner.capture_epoch, 0, 160_000),
            ));
            let observation =
                ledger.next_word_observation(LedgerObservationProducer::Whisper, 2, &owner);
            // Corroboration requires a different PCM frame, not another
            // request ID for the evidence that opened the conflict.
            let words = [WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 160_000)];
            ledger.admit_word_trial(&trial, &observation, &words, &words);
            assert_eq!(ledger.text_of(&owner), Some("1286"));
        } else {
            // An inconclusive closed trial keeps the selected floor and
            // alternative. Formatting may read that committed floor;
            // it cannot select or adjudicate the alternative.
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .close_word_trial(&trial, "unresolved");
        }
        state.publish_resolved_word_seals(&ev_tx);
        {
            let request = formatter_rx
                .try_recv()
                .expect("a committed floor can now be formatted");
            assert_eq!(request.existing_label, if resolved { "1286" } else { "56" });
            assert_eq!(request.occurrence, owner);
            assert!(
                std::iter::from_fn(|| ev_rx.try_recv().ok()).any(|event| {
                    matches!(event, EngineEvent::LedgerSeal { receipt }
                            if receipt.sealed_occurrences.contains(&owner))
                }),
                "the source seal reaches the emitter before a formatter can reply"
            );
            assert_eq!(state.formatter_requested.len(), 1);
            state.publish_resolved_word_seals(&ev_tx);
            assert!(
                formatter_rx.try_recv().is_err(),
                "one decision schedules only one formatter"
            );
        }
    }
}

#[test]
fn formatter_cannot_launch_or_close_an_open_word_trial() {
    use crate::pipeline::acoustic_ledger::{SealRefusal, WordPin};
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let mut state = state_for_session("formatter-pending-word-trial");
    state.formatter = Some(formatter_tx);
    let owner = OccurrenceIdentity::new(state.session_id.clone(), 1, 0, 160_000);
    let trial = {
        let mut ledger = state.acoustic_ledger.lock().expect("ledger");
        let calibration = EnergyCalibration::new("formatter-trial", 1.0, 1);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 10_000.0,
                        energy_integral: 100.0,
                        mean_rms_dbfs: -20.0,
                        peak_dbfs: -10.0,
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(160_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        ledger.schedule_frontier(
            owner.clone(),
            [
                LedgerObservationProducer::Apple,
                LedgerObservationProducer::Whisper,
            ],
        );
        ledger.admit_word_slots(
            &LedgerObservationIdentity::new(LedgerObservationProducer::Apple, 1, 0, owner.clone()),
            &[WordPin::new(48_000, 64_000, "56")],
        );
        assert!(!ledger.note_frontier_return(&owner, LedgerObservationProducer::Apple));
        ledger.admit_word_slots(
            &LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                1,
                1,
                owner.clone(),
            ),
            &[WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 128_000)],
        );
        ledger
            .next_word_trial(true)
            .expect("actual pending word trial")
    };
    state.finish_whisper_frontier(&ev_tx, &owner);
    assert!(formatter_rx.try_recv().is_err());
    assert!(state.formatter_requested.is_empty());
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.seal(&owner), Err(SealRefusal::FrontierOpen));
        assert_eq!(ledger.text_of(&owner), Some("56"));
    }
    state.formatter = None;
    let later = OccurrenceIdentity::new(state.session_id.clone(), 1, 160_000, 176_000);
    stage_pending_occurrence(&mut state, &ev_tx, 2, later.clone(), "Dalsza mowa");
    return_lexicon(&mut state, &ev_tx, 2, &later);
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    assert!(ledger.is_sealed(&later));
    assert!(!ledger.is_sealed(&owner));
    ledger.close_word_trial(&trial, "test_trial_finished");
    assert!(
        ledger.seal(&owner).is_ok(),
        "only the acoustic trial permits sealing"
    );
}

#[test]
fn formatter_protocol_identity_refusal_does_not_touch_the_seal() {
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let mut state = state_for_session("formatter-local-refusal");
    state.formatter = Some(formatter_tx);
    let owner = OccurrenceIdentity::new(state.session_id.clone(), 1, 0, 16_000);
    stage_pending_occurrence(&mut state, &ev_tx, 1, owner.clone(), "Iwo");
    return_lexicon(&mut state, &ev_tx, 1, &owner);
    let completion = FormatterCompletion::from_result(
        formatter_rx.try_recv().unwrap(),
        ai_result(AiFormatStatus::Applied, "Iwo!"),
    );
    let mut malformed = completion;
    malformed.proposal.capture_epoch += 1;
    let sink = crate::pipeline::sinks::CollectorEventSink::new();
    assert!(!deliver_formatter_completion(malformed, &sink, None));
    assert!(sink.events().is_empty());
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    assert!(ledger.is_sealed(&owner));
    assert!(
        ledger
            .frontier_of(&owner)
            .unwrap()
            .open_producers()
            .is_empty()
    );
}

#[test]
fn five_equal_labels_keep_five_text_jobs_and_seal_before_any_reply() {
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let mut state = state_for_session("formatter-five-iwo");
    state.formatter = Some(formatter_tx);
    let occurrences = (0..5_u64)
        .map(|index| {
            OccurrenceIdentity::new(
                "formatter-five-iwo",
                1,
                index * 16_000,
                (index + 1) * 16_000,
            )
        })
        .collect::<Vec<_>>();
    for (index, occurrence) in occurrences.iter().enumerate() {
        stage_pending_occurrence(
            &mut state,
            &ev_tx,
            index as u64 + 1,
            occurrence.clone(),
            "Iwo",
        );
        assert_eq!(
            formatter_rx.len(),
            index,
            "no formatting before final label"
        );
        return_lexicon(&mut state, &ev_tx, index as u64 + 1, occurrence);
        assert_eq!(formatter_rx.len(), index + 1);
    }
    drop(state.formatter.take());
    let requests = (0..5)
        .map(|_| formatter_rx.try_recv().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        formatter_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert_eq!(state.formatter_requested.len(), 5);
    for (request, occurrence) in requests.iter().zip(&occurrences) {
        assert_eq!(&request.occurrence, occurrence);
        assert_eq!(request.existing_label, "Iwo");
        assert_eq!(
            request.source_observation.as_ref().unwrap().occurrence,
            *occurrence
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_sealed(occurrence));
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .is_empty()
        );
    }
    assert!(state.pending_events.is_empty());
    let finals = std::iter::from_fn(|| ev_rx.try_recv().ok())
        .filter_map(|event| match event {
            EngineEvent::UtteranceFinal {
                utterance_id, text, ..
            } => Some((utterance_id, text)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        finals,
        (1..=5).map(|id| (id, "Iwo".into())).collect::<Vec<_>>()
    );
    let sink = crate::pipeline::sinks::CollectorEventSink::new();
    for request in requests {
        assert!(deliver_formatter_completion(
            FormatterCompletion::from_result(request, ai_result(AiFormatStatus::AiNoop, "Iwo")),
            &sink,
            None
        ));
    }
    assert_eq!(sink.events().len(), 5);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().rendered_text(),
        "Iwo Iwo Iwo Iwo Iwo"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn retained_formatter_drains_queued_and_in_flight_work_after_stop() {
    use crate::pipeline::sinks::CollectorEventSink;
    use crate::test_isolation::EnvGuard;
    let root = tempfile::tempdir().unwrap();
    let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
    let _policy = EnvGuard::set("FORMATTING_LEVEL", "off");
    let settings =
        Arc::new(crate::config::Config::load_runtime_snapshot_without_keychain().unwrap());
    let (sender, requests) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let (release, waiting) = tokio::sync::oneshot::channel();
    let first = FormatterRequest {
        occurrence: OccurrenceIdentity::new("stopped-take", 1, 0, 16_000),
        existing_label: "first".into(),
        source_observation: None,
        policy: FormattingPolicy::Smart,
    };
    let second = FormatterRequest {
        occurrence: OccurrenceIdentity::new("stopped-take", 1, 16_000, 32_000),
        existing_label: "second".into(),
        source_observation: None,
        policy: FormattingPolicy::Smart,
    };
    sender.send(second).await.unwrap();
    let mut jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    jobs.push_back(Box::pin(async move {
        waiting.await.unwrap();
        FormatterCompletion::from_result(first, ai_result(AiFormatStatus::Applied, "First!"))
    }));
    let sink = Arc::new(CollectorEventSink::new());
    let next_take_sink = CollectorEventSink::new();
    let retained = RetainedFormatterJobs {
        requests,
        jobs,
        runtime_settings: settings,
        language: None,
        chain: TakeFormatterChain::new(),
        event_sink: sink.clone(),
        stream_log_path: None,
    }
    .retain()
    .expect("accepted text jobs must retain an owner");
    // No sender or acoustic session is needed once Stop hands off work.
    drop(sender);
    tokio::task::yield_now().await;
    assert!(!retained.is_finished());
    assert!(sink.events().is_empty());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), retained)
        .await
        .unwrap()
        .unwrap();
    let events = sink.events();
    assert_eq!(events.len(), 2);
    for (index, event) in events.iter().enumerate() {
        let EngineEvent::OccurrenceLabelProposal { proposal } = event else {
            panic!("text completion cannot manufacture an acoustic event");
        };
        assert_eq!(proposal.session, "stopped-take");
        assert_eq!(proposal.sample_start, index as u64 * 16_000);
    }
    assert!(next_take_sink.events().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn retained_formatter_owner_deadline_cancels_without_raw_events() {
    use crate::test_isolation::EnvGuard;
    use std::sync::atomic::Ordering;
    struct CancelReceipt(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for CancelReceipt {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let root = tempfile::tempdir().unwrap();
    let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
    let _policy = EnvGuard::set("FORMATTING_LEVEL", "off");
    let settings =
        Arc::new(crate::config::Config::load_runtime_snapshot_without_keychain().unwrap());
    let (_sender, requests) = mpsc::channel(FORMATTER_QUEUE_CAP);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let guard = CancelReceipt(cancelled.clone());
    let mut jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    jobs.push_back(Box::pin(async move {
        let _guard = guard;
        std::future::pending::<FormatterCompletion>().await
    }));
    let sink = Arc::new(crate::pipeline::sinks::CollectorEventSink::new());
    RetainedFormatterJobs {
        requests,
        jobs,
        runtime_settings: settings,
        language: None,
        chain: TakeFormatterChain::new(),
        event_sink: sink.clone(),
        stream_log_path: None,
    }
    .run(Duration::from_millis(10))
    .await;
    assert!(cancelled.load(Ordering::SeqCst));
    assert!(sink.events().is_empty());
    let result = bound_formatter_reply(
        "Raw survives",
        Duration::from_millis(10),
        std::future::pending::<AiFormatResult>(),
    )
    .await;
    assert_eq!(result.status, AiFormatStatus::Failed);
    assert_eq!(result.text, "Raw survives");
}

#[tokio::test]
async fn provider_latency_does_not_block_engine_event_drainage() {
    let mut formatter_jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    formatter_jobs.push_back(Box::pin(std::future::pending::<FormatterCompletion>()));
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    ev_tx
        .send(EngineEvent::Preview {
            rev: 1,
            text: "live".to_string(),
            pin: PreviewPin::open_occurrence(TailSampleRange {
                session: "drainage".into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 0,
            }),
        })
        .expect("event receiver");

    let event = tokio::select! {
        Some(event) = ev_rx.recv() => event,
        Some(_) = formatter_jobs.next() => panic!("pending provider completed"),
    };
    assert!(matches!(event, EngineEvent::Preview { text, .. } if text == "live"));
}

#[test]
fn terminal_seal_refusal_is_visible_without_becoming_success() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    report_terminal_seal_refusal(&tx, SealRefusal::NotQualified);
    assert!(
        rx.try_recv().is_err(),
        "no-speech is the quiet terminal case"
    );

    report_terminal_seal_refusal(&tx, SealRefusal::FrontierOpen);
    assert!(matches!(
        rx.try_recv().expect("frontier refusal diagnostic"),
        EngineEvent::Warning { code, message }
            if code == LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE
                && message == SealRefusal::FrontierOpen.as_str()
    ));
    assert!(rx.try_recv().is_err(), "refusal emits no ledger seal");
}

#[test]
fn whisper_no_payload_closes_exact_coalesced_members_once() {
    let (tx, mut event_rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
    let mut state = state_for_session("coalesced");
    state.tail_patch = Some(tail_tx);
    let first = OccurrenceIdentity::new("coalesced", 1, 0, 16_000);
    let second = OccurrenceIdentity::new("coalesced", 1, 16_000, 32_000);
    assert_ne!(
        first, second,
        "disjoint PCM ranges are distinct occurrences"
    );

    place_pcm_through(&mut state, 32_000);
    stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 1, &first);
    stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 2, &second);
    // Stopping before a full 9 s window emits one partial that contains
    // both members and finishes the plan.
    let offered = offer_capture_windows(&mut state, &tx, &mut tail_rx, true);
    assert_eq!(offered.len(), 1, "one exact coalesced request");
    let request = &offered[0];
    assert_eq!(request.admit_sample_start, 0);
    assert_eq!(request.admit_sample_end, 32_000);
    assert_eq!(request.member_occurrences.len(), 2);
    assert!(
        request
            .member_occurrences
            .iter()
            .any(|(_, owner)| owner == &first)
    );
    assert!(
        request
            .member_occurrences
            .iter()
            .any(|(_, owner)| owner == &second)
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    while event_rx.try_recv().is_ok() {}

    state.complete_whisper_window(&tx, no_payload_completion(request), 2.0);
    state.close_admission_horizon(&tx, 32_000);
    let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        2,
        "each exact member occurrence seals once"
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        EngineEvent::LedgerMutation { observation, receipt, .. }
            if observation.producer == LedgerObservationProducer::Whisper
                && receipt.grants_mutation()
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, .. }
                    if observation.producer == LedgerObservationProducer::Whisper
            ))
            .count(),
        0,
        "Apple text already exists, so a no-payload close does not mint a Whisper mutation"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::Warning { code, .. }
                    if code == RefinementFailure::NotScheduled.code()
            ))
            .count(),
        2,
        "each member retains its no-label refusal as live_refinement_not_scheduled"
    );
    let ledger = state.acoustic_ledger.lock().expect("ledger");
    assert!(ledger.is_sealed(&first));
    assert!(ledger.is_sealed(&second));
    for occurrence in [&first, &second] {
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .is_empty()
        );
    }
    assert_eq!(ledger.text_of(&first), Some("Iwo"));
    assert_eq!(ledger.text_of(&second), Some("Iwo"));
    drop(ledger);
    assert!(state.pending_events.is_empty());
    assert_eq!(state.tail_patch_awaiting_completion(), 0);

    state.complete_whisper_window(&tx, no_payload_completion(request), 2.0);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(event_rx.try_recv().is_err(), "replay emits no second seal");
}

// 76d0 (2026-10-23, DT asked through Stop): DT answered about 0.5 s after
// an owner closed, and a Whisper window still due then corrected over that
// answer. DT follows the tank: an owner is asked once no window may still
// cover it, and then once.
#[test]
fn live_witness_asks_an_owner_once_after_its_last_whisper_window_returns() {
    let (tx, _event_rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
    let mut state = state_for_session("tank-then-dt");
    state.tail_patch = Some(tail_tx);
    let owner = OccurrenceIdentity::new("tank-then-dt", 1, 0, 16_000);
    place_pcm_through(&mut state, 16_000);
    stage_pending_occurrence(&mut state, &tx, 1, owner.clone(), "Iwo");
    let ask =
        |state: &AppleSealState| live_witness_askable_owners(state, vec![owner.clone()], 16_000);
    assert!(
        ask(&state).is_empty(),
        "no Whisper window over the owner is planned yet"
    );

    place_pcm_through(&mut state, 144_000);
    let offered = offer_capture_windows(&mut state, &tx, &mut tail_rx, false);
    assert_eq!(offered.len(), 1, "the first 9 s window");
    assert!(
        ask(&state).is_empty(),
        "the window over the owner has not returned"
    );

    state.complete_whisper_window(&tx, no_payload_completion(&offered[0]), 2.0);
    assert_eq!(ask(&state), vec![owner.clone()]);

    record_dictation_witness_attempts(
        &mut state.dictation_witness_attempts,
        &[(0, 16_000)],
        0,
        16_000,
        480_000,
    );
    assert!(ask(&state).is_empty(), "an owner is asked once");
}

#[test]
fn live_witness_without_a_whisper_lane_asks_a_closed_owner_at_once() {
    let (tx, _event_rx) = mpsc::unbounded_channel();
    let mut state = state_for_session("no-tank");
    let owner = OccurrenceIdentity::new("no-tank", 1, 0, 16_000);
    place_pcm_through(&mut state, 16_000);
    stage_pending_occurrence(&mut state, &tx, 1, owner.clone(), "Iwo");
    assert_eq!(
        live_witness_askable_owners(&state, vec![owner.clone()], 16_000),
        vec![owner]
    );
}

/// W3B falsifier for the retired concat-space remap.
///
/// The deleted `remap_range` refused a char range that crossed a join
/// between two coalesced utterances, because pouring it into the first span
/// invents text at the wrong acoustic identity. That refusal is now
/// structural, not arithmetic: `complete_whisper_window` keeps only the
/// provider segments whose PCM range lies wholly inside one member's
/// occurrence, and the whole-window text fallback is reachable for a
/// single-member window only.
///
/// A straddling segment intersects both members, so neither member is
/// relabelled. The pin that sits wholly inside the second member stays
/// visible and does not replace that member: one non-exclusive pin blocks
/// the whole span.
#[test]
fn a_candidate_straddling_two_member_occurrences_labels_neither() {
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailTimingQuality,
    };

    let (tx, mut event_rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
    let mut state = state_for_session("straddle");
    state.tail_patch = Some(tail_tx);
    let first = OccurrenceIdentity::new("straddle", 1, 0, 16_000);
    let second = OccurrenceIdentity::new("straddle", 1, 16_000, 32_000);

    place_pcm_through(&mut state, 32_000);
    stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 1, &first);
    stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 2, &second);
    let offered = offer_capture_windows(&mut state, &tx, &mut tail_rx, true);
    assert_eq!(offered.len(), 1, "one exact coalesced request");
    let request = &offered[0];
    assert_eq!(request.admit_sample_start, 0);
    assert_eq!(request.admit_sample_end, 32_000);
    assert_eq!(request.member_occurrences.len(), 2);
    while event_rx.try_recv().is_ok() {}

    // One segment crosses the join (8 000..24 000); one is pinned wholly
    // inside the second member: the positive control owns 24 000..32 000.
    let identity = request.provider_request.identity.clone();
    let straddling = TimedTailSegment {
        confidence: None,
        grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
        text: "przez granice".to_string(),
        range: TailSampleRange {
            session: "straddle".to_string(),
            capture_epoch: 1,
            sample_start: 8_000,
            sample_end: 24_000,
        },
    };
    let pinned = TimedTailSegment {
        confidence: None,
        grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
        text: "Iwo drugie".to_string(),
        range: TailSampleRange {
            session: "straddle".to_string(),
            capture_epoch: 1,
            sample_start: 24_000,
            sample_end: 32_000,
        },
    };
    let payload = TailProviderPayload {
        identity: identity.clone(),
        text: "cale okno przez granice".to_string(),
        segments: vec![straddling, pinned],
        avg_logprob: Some(-0.2),
        compression_ratio: Some(1.0),
        provider_id: TailProviderId::Fake,
        elapsed_ms: 1,
        evidence: TailProviderEvidence {
            segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            source: TailEvidenceSource::Whisper,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: Some(-0.2),
            vad_selected_samples: None,
        },
    };
    assert!(payload.validate().is_ok());
    state.complete_whisper_window(
        &tx,
        TailPatchCompletion {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: Some(identity),
            payload: Some(payload),
            member_occurrences: request.member_occurrences.clone(),
        },
        2.0,
    );

    state.close_admission_horizon(&tx, 32_000);
    let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
    let whisper_labels = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::LedgerMutation {
                observation,
                label,
                receipt,
            } if observation.producer == LedgerObservationProducer::Whisper
                && receipt.grants_mutation() =>
            {
                Some((observation.occurrence.clone(), label.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        whisper_labels,
        Vec::<(OccurrenceIdentity, String)>::new(),
        "a straddling pin blocks replacement of every member it intersects"
    );
    assert!(
        !whisper_labels
            .iter()
            .any(|(occurrence, _)| occurrence == &first),
        "a straddling candidate must not be poured into the first span"
    );

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        0
    );
    assert!(events.iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation { observation, receipt, .. }
            if observation.producer == LedgerObservationProducer::Whisper
                && observation.occurrence == first && !receipt.grants_mutation()
    )));
    let ledger = state.acoustic_ledger.lock().expect("ledger");
    for occurrence in [&first, &second] {
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .is_empty()
        );
    }
    assert!(!ledger.is_sealed(&first));
    assert!(ledger.text_recovery_pending(&first));
    assert!(!ledger.is_sealed(&second));
    assert!(ledger.text_recovery_pending(&second));
    assert_eq!(
        ledger.text_of(&first),
        Some("Iwo"),
        "the Apple floor survives a candidate that named no exact occurrence"
    );
    assert_eq!(
        ledger.text_of(&second),
        Some("Iwo"),
        "the wholly owned pin stays visible and does not replace the member"
    );
    drop(ledger);

    // Post-seal immutability: the same completion replayed after the seal
    // cannot reopen either occurrence or move a single label.
    state.complete_whisper_window(&tx, no_payload_completion(request), 2.0);
    assert!(
        event_rx.try_recv().is_err(),
        "a replayed completion after the terminal seal emits nothing"
    );
    let ledger = state.acoustic_ledger.lock().expect("ledger");
    assert_eq!(ledger.text_of(&first), Some("Iwo"));
    assert_eq!(ledger.text_of(&second), Some("Iwo"));
}

#[test]
fn pause_flush_rejection_conserves_newly_held_member() {
    let (tx, mut event_rx) = mpsc::unbounded_channel();
    let (tail_tx, tail_rx) = mpsc::channel::<TailPatchRequest>(1);
    drop(tail_rx);
    let mut state = state_for_session("pause-rejection");
    state.tail_patch = Some(tail_tx);
    let first = OccurrenceIdentity::new("pause-rejection", 1, 0, 16_000);
    let second = OccurrenceIdentity::new("pause-rejection", 1, 48_000, 64_000);

    place_pcm_through(&mut state, 144_000);
    stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 1, &first);

    stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 2, &second);
    // A dead sender cannot supply either owner later. Both Apple labels
    // survive and settle exactly once; an unissued frame is not live work.
    assert!(
        drive_capture_plan(&mut state, &tx, false),
        "failure to queue prior window A cannot revoke newly held B"
    );
    state.close_admission_horizon(&tx, first.sample_end);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .is_sealed(&first)
    );
    assert!(!state.pending_events.contains_key(&1));
    assert!(!state.pending_events.contains_key(&2));
    assert!(
        !state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .frontier_of(&second)
            .expect("B frontier")
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );

    // Later grid accounting on the dead lane cannot emit either final twice.
    place_pcm_through(&mut state, 192_000);
    assert!(drive_capture_plan(&mut state, &tx, true));
    assert!(state.window_plan.is_finished());
    assert!(state.refinement_lane_lost);
    assert!(state.refinement_pending.is_empty());
    assert!(state.refinement_submitted.is_empty());
    // Both scheduled jobs are terminal and the capture epoch can close.
    state.close_admission_horizon(&tx, second.sample_end);
    let ledger = state.acoustic_ledger.lock().expect("ledger");
    for occurrence in [&first, &second] {
        assert!(ledger.is_sealed(occurrence));
        assert_eq!(ledger.text_of(occurrence), Some("Iwo"));
        assert!(
            !ledger
                .frontier_of(occurrence)
                .expect("frontier")
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
    }
    drop(ledger);
    assert!(state.pending_events.is_empty());
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        2
    );
    let final_count = events
        .iter()
        .filter(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
        .count();
    assert_eq!(
        final_count, 2,
        "A and B each emit exactly one pending final"
    );
    assert!(!drive_capture_plan(&mut state, &tx, true));
    assert!(
        event_rx.try_recv().is_err(),
        "drained members cannot seal twice"
    );
}

#[test]
fn replayed_completion_does_not_consume_another_jobs_debt() {
    let (tx, _event_rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
    let mut state = state_for_session("two-jobs");
    state.tail_patch = Some(tail_tx);
    let first = OccurrenceIdentity::new("two-jobs", 1, 0, 16_000);
    // B starts on the second grid so it is not a member of A's 0..9 s
    // window and is wholly inside the following 3..12 s window.
    let second = OccurrenceIdentity::new("two-jobs", 1, 48_000, 64_000);

    place_pcm_through(&mut state, 144_000);
    stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 1, &first);
    let mut offered = offer_capture_windows(&mut state, &tx, &mut tail_rx, false);
    assert_eq!(offered.len(), 1, "first accepted job");
    let first_request = offered.pop().expect("first accepted job");
    assert_eq!(first_request.member_occurrences, vec![(1, first.clone())]);

    place_pcm_through(&mut state, 192_000);
    stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
    return_lexicon(&mut state, &tx, 2, &second);
    let mut offered = offer_capture_windows(&mut state, &tx, &mut tail_rx, false);
    assert_eq!(offered.len(), 1, "second accepted job");
    let second_request = offered.pop().expect("second accepted job");
    assert_eq!(second_request.member_occurrences, vec![(2, second.clone())]);
    assert_eq!(state.tail_patch_awaiting_completion(), 2);

    state.complete_whisper_window(&tx, no_payload_completion(&first_request), 3.0);
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    state.complete_whisper_window(&tx, no_payload_completion(&first_request), 3.0);
    assert_eq!(
        state.tail_patch_awaiting_completion(),
        1,
        "replayed A cannot consume B's accepted job debt"
    );
    assert!(
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .frontier_of(&second)
            .expect("B frontier")
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );
    assert!(state.pending_events.contains_key(&2));

    state.return_outstanding_whisper_without_label(&tx);
    state.seal_remaining_at_session_end(&tx);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .is_sealed(&second)
    );
    assert!(state.pending_events.is_empty());
}
