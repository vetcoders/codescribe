/// rc-w2-test-rehab: current-owner replacements for the 26 parked contracts.
/// Synthetic PCM/edges below are unit fixtures, never measured take receipts.
/// Raw finals are telemetry; every document assertion reads AcousticLedger.
use super::*;
use crate::pipeline::acoustic_ledger::RefuseReason;
use crate::pipeline::sinks::CollectorEventSink as RecordingSink;

/// The stop drain reports collected finals once, before session finality.
#[test]
fn layer1_stop_warns_once_for_collected_finals() {
    use crate::asr_session::{
        AsrSessionEvent, FakeAsrSessionProvider, Layer1Decision, RefinerMode, TranscriptEvent,
    };

    for refiner in [RefinerMode::CloudSession, RefinerMode::LocalHelper] {
        for degraded in [false, true] {
            let sink = RecordingSink::default();
            let input = Layer1SessionInput {
                session_id: Layer1SessionId::new("finals-receipt").expect("session id"),
                locale: None,
                sample_rate: 16_000,
            };
            let script = (1..=3)
                .map(|id| {
                    AsrSessionEvent::Final(TranscriptEvent {
                        session_id: input.session_id.clone(),
                        utterance_id: id,
                        sequence_id: id,
                        text: "private transcript".to_string(),
                        range: None,
                        commit: None,
                    })
                })
                .collect();
            let provider = FakeAsrSessionProvider::with_script(refiner, script);
            let mut lane =
                RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input);
            let layer1_refiner = lane.refiner_mode();
            if degraded {
                let frame_len = if refiner == RefinerMode::CloudSession {
                    crate::asr_session::recorder::cloud_commit_holdback_samples(input.sample_rate)
                        + 1
                } else {
                    320
                };
                let frame = vec![0.1; frame_len];
                for _ in 0..3 {
                    lane.offer_pcm(&frame);
                    lane.poll();
                }
                lane.note_sleep_wake();
                assert_eq!(lane.refiner_mode(), RefinerMode::Off);
            }
            let outcome = lane.stop();
            let counts = outcome.telemetry();
            assert_eq!(counts.finals_accepted, 3);
            emit_layer1_finals_not_admitted_warning(&sink, counts.finals_accepted, layer1_refiner);
            emit_session_finalised(
                &sink,
                input.session_id.as_str().to_string(),
                0,
                SessionConservationReceipt::default(),
            );

            let events = sink.events();
            assert_eq!(events.len(), 2);
            let EngineEvent::Warning { code, message } = &events[0] else {
                panic!("expected typed Warning, got {:?}", events[0]);
            };
            assert_eq!(code, "layer1_finals_not_admitted");
            assert_eq!(
                message,
                &format!(
                    "finals_accepted=3 refiner={}: Layer 1 finals arrived after the worker exited",
                    refiner.as_token()
                )
            );
            assert!(!message.contains("private transcript"));
            assert!(matches!(events[1], EngineEvent::SessionFinalised { .. }));
        }
    }
}

/// A stopped lane with partials but no accepted finals has no such receipt.
#[test]
fn layer1_stop_without_finals_is_silent() {
    use crate::asr_session::{
        AsrSessionEvent, FakeAsrSessionProvider, Layer1Decision, RefinerMode, TranscriptEvent,
    };

    let sink = RecordingSink::default();
    let input = Layer1SessionInput {
        session_id: Layer1SessionId::new("no-finals-receipt").expect("session id"),
        locale: None,
        sample_rate: 16_000,
    };
    let provider = FakeAsrSessionProvider::with_script(
        RefinerMode::CloudSession,
        vec![AsrSessionEvent::Partial(TranscriptEvent {
            session_id: input.session_id.clone(),
            utterance_id: 1,
            sequence_id: 1,
            text: "private partial".to_string(),
            range: None,
            commit: None,
        })],
    );
    let mut lane = RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input);
    let layer1_refiner = lane.refiner_mode();
    let outcome = lane.stop();
    let counts = outcome.telemetry();
    assert_eq!(counts.partials_applied, 1);
    assert_eq!(counts.finals_accepted, 0);
    emit_layer1_finals_not_admitted_warning(&sink, counts.finals_accepted, layer1_refiner);
    assert!(sink.events().is_empty());
}

const RATE: u32 = 16_000;

fn sample(secs: f32) -> u64 {
    (secs * RATE as f32).round() as u64
}

fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
    TranscriptSegment {
        confidence: None,
        text: text.into(),
        start_ts,
        end_ts,
    }
}

fn state(session: &str, secs: f32) -> AppleSealState {
    let mut state = AppleSealState::new_for_session(RATE, session.into(), 7);
    state.energy_calibration = Some(EnergyCalibration::new("rehab-synthetic", 1.0, 1));
    for chunk in vec![0.25; sample(secs) as usize].chunks(1024) {
        state.audio.push(chunk);
    }
    state
}

// The fallback Apple path consumes an EXISTING qualification. Supply that
// exact precondition from this fixture's retained PCM, without admitting a
// label or manufacturing a terminal coverage receipt.
fn qualify(state: &mut AppleSealState, start: f32, end: f32) -> OccurrenceIdentity {
    let occurrence = OccurrenceIdentity::new(
        &state.session_id,
        state.capture_epoch,
        sample(start),
        sample(end),
    );
    let window = state
        .window_by_samples(sample(start), sample(end))
        .expect("fixture PCM");
    assert!(window.samples.iter().all(|value| *value == 0.25));
    let calibration = state
        .energy_calibration
        .as_ref()
        .expect("fixture calibration");
    let evidence = AcousticEvidence {
        occurrence: occurrence.clone(),
        duration_ms: f64::from(end - start) * 1_000.0,
        energy_integral: window.samples.len() as f64 * 0.25_f64.powi(2),
        mean_rms_dbfs: 20.0 * 0.25_f64.log10(),
        peak_dbfs: 20.0 * 0.25_f64.log10(),
        vad_open_sample: Some(sample(start)),
        vad_close_sample: Some(sample(end)),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualify(&evidence, calibration)
            .is_qualified()
    );
    occurrence
}

// Root-owned Cloud controls: actual measured PCM, ingress and source work.
fn forensic_cloud_capture(
    session: &str,
    next: bool,
    tx: &mpsc::UnboundedSender<EngineEvent>,
) -> (
    AppleSealState,
    Vec<OccurrenceIdentity>,
    Vec<f32>,
    mpsc::Receiver<u64>,
) {
    let mut state = state(session, 0.0);
    let mut pcm = vec![0.0; sample(if next { 4.0 } else { 2.0 }) as usize];
    pcm[sample(0.2) as usize..sample(0.6) as usize].fill(0.25);
    if next {
        pcm[sample(1.8) as usize..sample(2.1) as usize].fill(0.25);
    }
    state.audio.push(&pcm);
    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    for chunk in pcm.chunks(320) {
        writer.push_samples(chunk);
    }
    let speech = coverage_speech_evidence(&state);
    assert_eq!(
        speech.producer(),
        crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER
    );
    assert_eq!(
        speech.availability().observed_samples(),
        Some(pcm.len() as u64)
    );
    assert_eq!(speech.ranges().len(), if next { 2 } else { 1 });
    let (commit_tx, commit_rx) = mpsc::channel(4);
    state.cloud_commit_tx = Some(commit_tx);
    let mut owners = Vec::new();
    for (id, start, end) in if next {
        vec![(1, 0.0, 1.0), (2, 1.7, 2.2)]
    } else {
        vec![(1, 0.0, 1.0)]
    } {
        let owner =
            OccurrenceIdentity::new(session, state.capture_epoch, sample(start), sample(end));
        assert!(qualify_owned_occurrence(&state, &owner));
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            ledger.schedule_frontier(
                owner.clone(),
                [
                    LedgerObservationProducer::Apple,
                    LedgerObservationProducer::Lexicon,
                ],
            );
            assert!(ledger.require_text_recovery(&owner));
        }
        for producer in [
            LedgerObservationProducer::Apple,
            LedgerObservationProducer::Lexicon,
        ] {
            assert!(
                admit_ledger_label(
                    &mut state,
                    tx,
                    LabelAdmission {
                        observation: LedgerObservationIdentity::new(producer, id, 0, owner.clone()),
                        label: "zostaje",
                        energy: EnergyAdmission::RequireExistingQualification,
                    }
                )
                .is_some()
            );
        }
        assert!(cloud_open(&state, &owner));
        assert!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_recovery_pending(&owner)
        );
        if id == 1 {
            state.commit_cloud_close(tx, silence_close(1.5));
            assert_eq!(state.cloud_inflight.len(), 1);
        }
        owners.push(owner);
    }
    (state, owners, pcm, commit_rx)
}

fn forensic_cloud_final(
    session: &str,
    phrase: bool,
) -> crate::asr_session::events::TranscriptEvent {
    let words = [("cloud", sample(0.2), sample(0.6))];
    let mut event = cloud_notice_final("cloud", if phrase { &[] } else { &words }, 0, sample(1.5));
    event.session_id = crate::asr_session::events::SessionId::new(session).unwrap();
    event
}

#[test]
fn forensic_cloud_ingress_refuses_foreign_duplicate_and_stale_finals() {
    use crate::asr_session::{
        AsrSessionEvent, FakeAsrSessionProvider, Layer1Decision, RefinerMode,
    };
    let input = Layer1SessionInput {
        session_id: Layer1SessionId::new("cloud-ingress-owned").unwrap(),
        locale: None,
        sample_rate: RATE,
    };
    let valid = forensic_cloud_final("cloud-ingress-owned", false);
    let foreign = forensic_cloud_final("cloud-ingress-foreign", false);
    let mut stale = valid.clone();
    stale.utterance_id = 2;
    stale.text = "stale".into();
    let script = vec![
        AsrSessionEvent::Final(foreign),
        AsrSessionEvent::Final(valid.clone()),
        AsrSessionEvent::Final(valid.clone()),
        AsrSessionEvent::Final(stale),
    ];
    let fake = FakeAsrSessionProvider::with_script(RefinerMode::CloudSession, script);
    let mut lane = RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(fake)), &input);
    let frame = vec![0.25; 320];
    for index in 0..4 {
        let _ = lane.offer_pcm(&frame);
        let _ = lane.flush_holdback();
        lane.poll();
        let finals = lane.take_unforwarded_finals();
        if index == 1 {
            assert_eq!(finals, vec![valid.clone()]);
        } else {
            assert!(finals.is_empty());
        }
    }
    assert_eq!(lane.pushed_samples(), 1_280);
    assert_eq!(lane.telemetry().events_rejected, 2);
    assert_eq!(lane.telemetry().finals_accepted, 1);
    assert_eq!(lane.finals(), &[valid]);
}

fn forensic_cloud_assert_complete_final_source(next: bool) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (mut state, owners, pcm, _commit_rx) =
        forensic_cloud_capture("cloud-complete-source", next, &tx);
    let source = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .slots_of(&owners[0])
        .unwrap()[0]
        .clone();
    assert_eq!(
        state
            .window_by_samples(0, pcm.len() as u64)
            .unwrap()
            .samples,
        pcm
    );
    drain(&mut rx);
    let final_event = forensic_cloud_final(&state.session_id, false);
    if next {
        state.close_admission_horizon(&tx, sample(1.7));
        state.release_cloud_live_behind(&tx, sample(2.7));
        assert!(cloud_open(&state, &owners[0]));
        state.handle_cloud_notice(&tx, CloudWorkerNotice::Final(Box::new(final_event)));
    } else {
        let (notice_tx, notice_rx) = std_mpsc::channel();
        notice_tx
            .send(CloudWorkerNotice::Final(Box::new(final_event)))
            .unwrap();
        notice_tx.send(CloudWorkerNotice::EndSettled).unwrap();
        assert!(drain_cloud_stop_finals(&mut state, &tx, &notice_rx));
    }
    assert_eq!(state.cloud_live_admitted, 1);
    assert!(!cloud_open(&state, &owners[0]));
    assert!(state.cloud_inflight.is_empty());
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&owners[0]), Some("cloud"));
    assert!(
        !ledger.text_recovery_pending(&owners[0]),
        "complete Cloud decode scope must account the source, next={next}"
    );
    assert!(ledger.is_sealed(&owners[0]));
    let pins = ledger.slots_of(&owners[0]).unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].producer, LedgerObservationProducer::CloudLive);
    assert_eq!(
        (pins[0].sample_start, pins[0].sample_end),
        (sample(0.2), sample(0.6))
    );
    assert_eq!(
        ledger
            .slot_operations()
            .iter()
            .filter(
                |op| op.observation.producer == LedgerObservationProducer::CloudLive
                    && op.sources.contains(&source)
                    && op.outputs.contains(&pins[0])
            )
            .count(),
        1
    );
    assert_eq!(ledger.conservation().residue(), 0);
    if next {
        assert!(ledger.text_recovery_pending(&owners[1]));
        drop(ledger);
        assert!(cloud_open(&state, &owners[1]));
    } else {
        drop(ledger);
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        let coverage = ledger.assess_seal_coverage(
            &state.session_id,
            state.capture_epoch,
            &coverage_speech_evidence(&state),
            0,
        );
        assert!(coverage.status.is_complete());
        assert!(ledger.record_seal_coverage(coverage));
        assert!(
            ledger
                .seal_terminal(&state.session_id, state.capture_epoch)
                .is_ok()
        );
    }
    assert_eq!(state.session_conservation().residue(), 0);
    let events = drain(&mut rx);
    assert_eq!(events.iter().filter(|event| matches!(event, EngineEvent::LedgerSeal { receipt } if receipt.coverage == owners[0])).count(), 1);
}

#[test]
fn forensic_cloud_complete_final_accounts_source_before_end() {
    forensic_cloud_assert_complete_final_source(false);
}

#[test]
fn forensic_cloud_complete_final_accounts_source_inside_grace() {
    forensic_cloud_assert_complete_final_source(true);
}

#[test]
fn forensic_cloud_phrase_transport_return_preserves_source_debt() {
    for stop in [false, true] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (mut state, owners, _, _commit_rx) =
            forensic_cloud_capture("cloud-phrase-source", false, &tx);
        drain(&mut rx);
        let event = forensic_cloud_final(&state.session_id, true);
        if stop {
            let (notice_tx, notice_rx) = std_mpsc::channel();
            notice_tx
                .send(CloudWorkerNotice::Final(Box::new(event)))
                .unwrap();
            notice_tx.send(CloudWorkerNotice::EndSettled).unwrap();
            assert!(drain_cloud_stop_finals(&mut state, &tx, &notice_rx));
        } else {
            state.handle_cloud_notice(&tx, CloudWorkerNotice::Final(Box::new(event)));
        }
        assert!(!cloud_open(&state, &owners[0]));
        assert!(state.cloud_inflight.is_empty());
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owners[0]), Some("zostaje"));
        assert_eq!(state.cloud_live_admitted, 0);
        assert!(ledger.text_recovery_pending(&owners[0]));
        assert!(!ledger.is_sealed(&owners[0]));
        assert_eq!(ledger.conservation().residue(), 0);
        drop(ledger);
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        let coverage = ledger.assess_seal_coverage(
            &state.session_id,
            state.capture_epoch,
            &coverage_speech_evidence(&state),
            0,
        );
        assert!(!coverage.status.is_complete());
        assert!(ledger.record_seal_coverage(coverage));
        assert!(
            ledger
                .seal_terminal(&state.session_id, state.capture_epoch)
                .is_err()
        );
        drop(ledger);
        assert_eq!(state.session_conservation().residue(), 0);
        let events = drain(&mut rx);
        assert!(events.iter().any(|event| matches!(event, EngineEvent::LedgerMutation { observation, label, receipt } if observation.producer == LedgerObservationProducer::CloudLive && label == "cloud" && !receipt.grants_mutation())));
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::LedgerSeal { .. } | EngineEvent::UtteranceFinal { .. }
        )));
    }
}

#[test]
fn forensic_cloud_last_observer_seals_before_scheduling_formatter() {
    let session = "cloud-last-formatter";
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state(session, 0.0);
    let mut pcm = vec![0.25; sample(1.0) as usize];
    pcm.extend(vec![0.0; sample(0.5) as usize]);
    state.audio.push(&pcm);
    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    for chunk in pcm.chunks(320) {
        writer.push_samples(chunk);
    }
    let speech = coverage_speech_evidence(&state);
    assert_eq!(speech.ranges().len(), 1);
    assert_eq!(
        speech.availability().observed_samples(),
        Some(pcm.len() as u64)
    );
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    state.cloud_commit_tx = Some(commit_tx);
    let (formatter_tx, mut formatter_rx) = mpsc::channel(4);
    state.formatter = Some(formatter_tx);
    let owner = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    assert!(cloud_open(&state, &owner));
    assert!(
        formatter_rx.try_recv().is_err(),
        "Cloud still owes this span"
    );
    assert!(
        !state
            .acoustic_ledger
            .lock()
            .unwrap()
            .text_recovery_pending(&owner)
    );
    drain(&mut rx);
    let mut event = cloud_notice_final("cloud", &[("cloud", 0, sample(1.0))], 0, sample(1.5));
    event.session_id = crate::asr_session::events::SessionId::new(session).unwrap();
    state.handle_cloud_notice(&tx, CloudWorkerNotice::Final(Box::new(event)));
    assert_eq!(state.cloud_live_admitted, 1);
    assert!(!cloud_open(&state, &owner));
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&owner),
        Some("cloud")
    );
    assert!(
        !state
            .acoustic_ledger
            .lock()
            .unwrap()
            .text_recovery_pending(&owner)
    );
    let request = formatter_rx
        .try_recv()
        .expect("last Cloud observer must schedule the selected formatter");
    assert_eq!(request.occurrence, owner);
    assert_eq!(request.existing_label, "cloud");
    assert_eq!(
        request.source_observation.unwrap().producer,
        LedgerObservationProducer::CloudLive
    );
    assert!(state.formatter_requested.contains(&owner));
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        ledger
            .frontier_of(&owner)
            .unwrap()
            .open_producers()
            .is_empty()
    );
    assert!(ledger.is_sealed(&owner));
    assert_eq!(ledger.conservation().residue(), 0);
    drop(ledger);
    assert_eq!(state.session_conservation().residue(), 0);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
    );
}

fn physical_state(session: &str, secs: f32, ranges: &[(f32, f32)]) -> AppleSealState {
    let mut state = state(session, secs);
    let mut fusion = SileroIngress::new(RATE, session, state.capture_epoch);
    for &(start, end) in ranges {
        let observed = fusion.observe(Some((sample(start), sample(end))), true, sample(end));
        assert_eq!(observed.closed.len(), 1);
    }
    state.fusion = Some(fusion);
    state.fusion_seal_armed = true;
    state
}

fn emit(
    state: &mut AppleSealState,
    tx: &mpsc::UnboundedSender<EngineEvent>,
    words: Vec<TranscriptSegment>,
) {
    let text = words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let secs = state.audio.session_sample_end() as f32 / RATE as f32;
    emit_stream_events(
        vec![LiveStreamEvent::PhraseFinal {
            text,
            segments: words,
        }],
        tx,
        state,
        secs,
    );
}

fn drain(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn document(state: &AppleSealState) -> String {
    state.acoustic_ledger.lock().unwrap().rendered_text()
}

#[test]
fn apple_self_corrections_keep_only_current_word_per_pcm_slot() {
    let pin = |text: &str, start, end| FusionWord {
        confidence: None,
        text: text.into(),
        sample_start: start,
        sample_end: end,
    };
    let (selected, superseded) = select_current_apple_words(vec![
        pin("Vite", 1_000, 2_000),
        pin("po", 3_000, 4_000),
        pin("Vita", 1_000, 2_000),
        pin("Vitae", 1_000, 2_000),
        pin("21.", 5_000, 6_000),
        pin("21.", 5_000, 6_000),
    ]);
    assert_eq!(
        selected
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>(),
        ["Vitae", "po", "21."]
    );
    assert_eq!(
        superseded
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>(),
        ["Vite", "Vita"]
    );
}

#[test]
fn unadmitted_mirror_replaces_pending_word_at_same_pcm_slot() {
    let mut state = state("mirror-revision", 2.0);
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.unmatched_silero_words.push(FusionWord {
        confidence: None,
        text: "Vite".into(),
        sample_start: sample(0.25),
        sample_end: sample(0.5),
    });
    state.pending_silero_words.insert(
        2,
        vec![FusionWord {
            confidence: None,
            text: "Vitae".into(),
            sample_start: sample(0.25),
            sample_end: sample(0.5),
        }],
    );
    state.publish_unadmitted_words(&tx);
    let events = drain(&mut rx);
    let [EngineEvent::UnadmittedAppleWords { words, .. }] = events.as_slice() else {
        panic!("one preview revision expected: {events:?}");
    };
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "Vitae");
    assert_eq!(state.unmatched_silero_words[0].text, "Vite");
}

fn raw_finals(events: &[EngineEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::UtteranceFinal { raw_text, .. } => Some(raw_text.as_str()),
            _ => None,
        })
        .collect()
}

fn count_iwo(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| word.eq_ignore_ascii_case("iwo"))
        .count()
}

fn close_stop_residue(
    state: &mut AppleSealState,
    tx: &mpsc::UnboundedSender<EngineEvent>,
    secs: f32,
) {
    seal_open_partial(state, tx, secs);
    if state.fusion_seal_armed {
        seal_sliced_by_silero(state, tx, &[]);
    }
}

#[test]
fn mirror_replaces_all_held_sources_and_is_silent_when_unchanged() {
    let mut state = state("mirror-sources", 4.0);
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.publish_unadmitted_words(&tx);
    assert!(drain(&mut rx).is_empty());
    state.open_partial = "open".into();
    state.open_partial_segments = vec![segment("open", 0.0, 0.5)];
    state.preview_rev = 1;
    state.pending_silero_words.insert(
        9,
        vec![FusionWord {
            confidence: None,
            text: "pending".into(),
            sample_start: sample(1.0),
            sample_end: sample(1.5),
        }],
    );
    state.unmatched_silero_words.push(FusionWord {
        confidence: None,
        text: "unmatched".into(),
        sample_start: sample(2.0),
        sample_end: sample(2.5),
    });
    state.refused_untimed_words.insert(
        1,
        vec![UnadmittedAppleWord {
            text: "untimed".into(),
            sample_start: sample(3.0),
            sample_end: sample(3.0),
            source: UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
        }],
    );
    state.publish_unadmitted_words(&tx);
    let events = drain(&mut rx);
    let [
        EngineEvent::UnadmittedAppleWords {
            revision, words, ..
        },
    ] = events.as_slice()
    else {
        panic!("one complete mirror expected: {events:?}");
    };
    assert_eq!(*revision, 1);
    assert_eq!(
        words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>(),
        vec!["pending", "unmatched", "untimed", "open"]
    );
    assert!(matches!(
        words[0].source,
        UnadmittedAppleWordSource::Pending { utterance_id: 9 }
    ));
    assert!(matches!(
        words[1].source,
        UnadmittedAppleWordSource::Unmatched
    ));
    assert!(matches!(
        words[2].source,
        UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 }
    ));
    assert!(matches!(
        words[3].source,
        UnadmittedAppleWordSource::OpenPartial {
            rev: 1,
            phrase_id: 1
        }
    ));
    state.publish_unadmitted_words(&tx);
    assert!(drain(&mut rx).is_empty());
    state.pending_silero_words.clear();
    state.publish_unadmitted_words(&tx);
    let events = drain(&mut rx);
    let [
        EngineEvent::UnadmittedAppleWords {
            revision: 2, words, ..
        },
    ] = events.as_slice()
    else {
        panic!("replacement mirror expected: {events:?}");
    };
    assert_eq!(words.len(), 3);
    assert!(words.iter().all(|word| word.text != "pending"));
}

#[test]
fn worker_keeps_stuck_partial_pin_after_committed_speech_for_twenty_revisions() {
    for start in [0.0, 0.3] {
        let mut state = physical_state("stuck-pin", 70.0, &[(start, 4.0)]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        emit(&mut state, &tx, vec![segment("committed", start, 3.9)]);
        for rev in 1..=20 {
            emit_stream_events(
                vec![LiveStreamEvent::Partial {
                    text: "Iwo Iwo".into(),
                    segments: Vec::new(),
                }],
                &tx,
                &mut state,
                70.0,
            );
            assert_eq!(state.open_partial_pin, 0);
            assert_eq!(state.published_unadmitted_words.len(), 2);
            assert!(state.published_unadmitted_words.iter().all(|word| {
                word.text == "Iwo"
                    && matches!(word.source,
                        UnadmittedAppleWordSource::OpenPartial { rev: current, phrase_id: 2 }
                            if current == rev)
            }));
        }
        drain(&mut rx);
    }
}

#[test]
fn stop_flush_and_trailing_final_keep_one_phrase_and_five_iwo_words() {
    let mut state = physical_state("phrase-five-iwo", 63.0, &[(60.2, 62.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Iwo Iwo Iwo Iwo Iwo".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        63.0,
    );
    state.open_partial_pin = sample(0.8);
    seal_open_partial(&mut state, &tx, 63.0);
    assert_eq!(state.phrase_id, 2);
    assert_eq!(state.refused_untimed_words[&1].len(), 5);
    let words = (0..5)
        .map(|index| {
            let start = 60.2 + index as f32 * 0.3;
            segment("Iwo", start, start + 0.2)
        })
        .collect();
    emit(&mut state, &tx, words);
    assert_eq!(state.phrase_id, 2);
    assert_eq!(state.closed_phrases.len(), 1);
    assert_eq!(
        state.closed_phrases[&1].outcomes,
        BTreeMap::from([(ApplePhraseOutcome::Admitted, 5)])
    );
    assert_eq!(count_iwo(&document(&state)), 5);
    assert!(state.refused_untimed_words.is_empty());
    assert!(state.published_unadmitted_words.is_empty());
    drain(&mut rx);
}

#[test]
fn mixed_admitted_and_unmatched_final_publishes_the_unmatched_word() {
    let mut state = physical_state("mirror-mixed", 3.0, &[(0.0, 1.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit(
        &mut state,
        &tx,
        vec![
            segment("admitted", 0.0, 0.8),
            segment("unmatched", 2.0, 2.5),
        ],
    );
    assert_eq!(document(&state), "admitted");
    let events = drain(&mut rx);
    let words = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::UnadmittedAppleWords { words, .. } => Some(words),
            _ => None,
        })
        .expect("mixed final mirror");
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].text, "unmatched");
    assert!(matches!(
        words[0].source,
        UnadmittedAppleWordSource::Unmatched
    ));
    assert_eq!(
        state.closed_phrases[&1].outcomes,
        BTreeMap::from([
            (ApplePhraseOutcome::Admitted, 1),
            (ApplePhraseOutcome::Unmatched, 1),
        ])
    );
}

#[test]
fn closed_phrase_keeps_its_pending_result_after_periodic_admission() {
    let mut state = physical_state("phrase-pending", 3.0, &[]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    state
        .fusion
        .as_mut()
        .unwrap()
        .observe(Some((sample(0.0), sample(1.0))), false, sample(1.0));
    emit(
        &mut state,
        &tx,
        vec![segment("pending", 0.1, 0.8), segment("unmatched", 2.0, 2.5)],
    );
    let own_result = BTreeMap::from([
        (ApplePhraseOutcome::Pending, 1),
        (ApplePhraseOutcome::Unmatched, 1),
    ]);
    assert_eq!(state.closed_phrases[&1].outcomes, own_result);
    state
        .fusion
        .as_mut()
        .unwrap()
        .observe(None, true, sample(1.0));
    seal_sliced_by_silero(&mut state, &tx, &[]);
    assert_eq!(document(&state), "pending");
    assert_eq!(
        state.closed_phrases[&1].outcomes, own_result,
        "a later periodic admission cannot rewrite the closing call's result"
    );
    drain(&mut rx);
}

#[test]
fn replay_and_novel_word_counts_share_one_closing_result() {
    let mut state = physical_state("phrase-mixed-replay", 2.0, &[(0.0, 2.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, _tail_rx) = mpsc::channel(1);
    state.tail_patch = Some(tail_tx);
    let mut words = vec![segment("Iwo", 0.0, 0.5), segment("Iwo", 0.7, 1.0)];
    emit(&mut state, &tx, words.clone());
    words.push(segment("Iwo", 1.3, 1.6));
    emit(&mut state, &tx, words);
    assert_eq!(
        state.closed_phrases[&2].outcomes,
        BTreeMap::from([
            (ApplePhraseOutcome::Replay, 2),
            (ApplePhraseOutcome::Admitted, 1),
        ])
    );
    assert_eq!(count_iwo(&document(&state)), 3);
    drain(&mut rx);
}

#[test]
fn untimed_revisions_do_not_mint_phrase_closes_or_admission() {
    let mut state = physical_state("phrase-restart", 2.0, &[(0.0, 1.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let words = vec![segment("Iwo", 0.0, 0.8)];
    emit(&mut state, &tx, words.clone());
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Iwo".into(),
            segments: words.clone(),
        }],
        &tx,
        &mut state,
        2.0,
    );
    emit(&mut state, &tx, words);
    assert_eq!(
        state.closed_phrases[&2].outcomes,
        BTreeMap::from([(ApplePhraseOutcome::NoChange, 1)])
    );
    emit_stream_events(
        vec![
            LiveStreamEvent::Partial {
                text: "first phrase".into(),
                segments: Vec::new(),
            },
            LiveStreamEvent::Partial {
                text: "second".into(),
                segments: Vec::new(),
            },
        ],
        &tx,
        &mut state,
        2.0,
    );
    assert_eq!(state.phrase_id, 3);
    assert!(!state.closed_phrases.contains_key(&3));
    assert!(state.published_unadmitted_words.iter().any(|word| matches!(
        word.source,
        UnadmittedAppleWordSource::OpenPartial { phrase_id: 3, .. }
    )));
    drain(&mut rx);
}

#[test]
fn admitted_words_publish_ledger_then_mirror_then_phrase_receipt() {
    let mut state = physical_state("mirror-order", 1.0, &[(0.0, 1.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Iwo".into(),
            segments: vec![segment("Iwo", 0.0, 0.8)],
        }],
        &tx,
        &mut state,
        1.0,
    );
    drain(&mut rx);
    emit(&mut state, &tx, vec![segment("Iwo", 0.0, 0.8)]);
    let events = drain(&mut rx);
    let ledger_at = events
        .iter()
        .position(|event| {
            matches!(event,
                EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
            )
        })
        .expect("ledger publication");
    let mirror_at = events
        .iter()
        .position(|event| {
            matches!(event,
                EngineEvent::UnadmittedAppleWords { words, .. } if words.is_empty()
            )
        })
        .expect("empty mirror after admission");
    let receipt_at = events
        .iter()
        .position(|event| matches!(event, EngineEvent::PreviewDisposition { .. }))
        .expect("phrase receipt");
    assert!(ledger_at < mirror_at && mirror_at < receipt_at);
    let closed = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::UnadmittedAppleWords { closed_phrases, .. } => closed_phrases.get(&1),
            _ => None,
        })
        .expect("closing mirror carries the result");
    assert_eq!(
        closed.outcomes,
        BTreeMap::from([(ApplePhraseOutcome::Admitted, 1)])
    );
    assert!(state.published_unadmitted_words.is_empty());
}

fn retention_receipts(events: &[EngineEvent]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Warning { code, message } if code == "apple_words_retained" => {
                Some(serde_json::from_str(message).expect("retention receipt JSON"))
            }
            _ => None,
        })
        .collect()
}

fn retention_words(segments: &[TranscriptSegment]) -> Vec<FusionWord> {
    segments
        .iter()
        .map(|word| FusionWord {
            confidence: None,
            text: word.text.clone(),
            sample_start: sample(word.start_ts),
            sample_end: sample(word.end_ts),
        })
        .collect()
}

// Exercise the exit's retention seam directly: the full reconcile path
// repartitions these words before a swallowed utterance reaches its exit.
#[test]
fn overlap_retention_admits_two_words_once_and_counts_consumed_replay() {
    let mut state = state("retain-overlap", 2.0);
    let owner = qualify(&mut state, 0.0, 2.0);
    let current = vec![segment("alpha", 0.25, 0.5), segment("beta", 1.0, 1.25)];
    let words = retention_words(&current);
    let (tx, mut rx) = mpsc::unbounded_channel();
    for replay in [false, true] {
        retain_apple_words_at_exit(&mut state, &tx, 2, &words, &current, "overlap_refused");
        let receipts = retention_receipts(&drain(&mut rx));
        assert_eq!(receipts.len(), 1);
        let receipt = &receipts[0];
        assert_eq!(receipt["exit"], "overlap_refused");
        assert_eq!(receipt["utterance_id"], 2);
        assert_eq!(receipt["words"], 2);
        assert_eq!(receipt["unmatched"], 0);
        assert_eq!(receipt["kept_unanchored"], 0);
        assert_eq!(receipt["dropped_by_slot_rules"], if replay { 2 } else { 0 });
        if replay {
            assert_eq!(receipt["admitted_into"], serde_json::json!([]));
        } else {
            assert_eq!(
                receipt["admitted_into"],
                serde_json::json!([
                    { "occurrence": {
                        "session": owner.session,
                        "capture_epoch": owner.capture_epoch,
                        "sample_start": owner.sample_start,
                        "sample_end": owner.sample_end,
                    }, "count": 2 }
                ])
            );
        }
        assert_eq!(document(&state), "alpha beta");
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn swallowed_utterance_has_no_words_left_at_the_reconcile_exit() {
    let mut state = physical_state("retain-swallowed", 2.0, &[]);
    let mut physical = super::super::silero_fusion::UtteranceLedger::new();
    for _ in 0..2 {
        physical.open_or_extend(&state.session_id, state.capture_epoch, 0, sample(2.0));
        physical.close_open(sample(2.0));
    }
    let owner = qualify(&mut state, 0.0, 2.0);
    state.reconciled_silero.insert(1);
    // Permit a blank closed range to reach the overlap exit, as it would
    // with an armed or lost refiner. Apple-only skips the blank earlier.
    state.refinement_lane_lost = true;
    let current = vec![segment("alpha", 0.25, 0.5), segment("beta", 1.0, 1.25)];
    state
        .pending_silero_words
        .insert(2, retention_words(&current));
    let (tx, mut rx) = mpsc::unbounded_channel();
    reconcile_silero_ledger(&mut state, &tx, &physical, &current);
    assert_eq!(document(&state), "alpha beta");
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&owner)
            .unwrap()
            .len(),
        2
    );
    let receipts = retention_receipts(&drain(&mut rx));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["exit"], "overlap_refused");
    assert_eq!(receipts[0]["utterance_id"], 2);
    assert_eq!(receipts[0]["words"], 0, "repartition precedes the exit");
    assert!(state.reconciled_silero.contains(&2));
}

#[test]
fn one_sample_whisper_overlap_does_not_block_a_distinct_apple_word() {
    let mut state = state("retain-slot-rule", 2.0);
    let owner = qualify(&mut state, 0.0, 2.0);
    let whisper =
        LedgerObservationIdentity::new(LedgerObservationProducer::Whisper, 1, 0, owner.clone());
    state.acoustic_ledger.lock().unwrap().admit_word_slots(
        &whisper,
        &[crate::pipeline::acoustic_ledger::WordPin::new(
            sample(0.25),
            sample(0.5),
            "heard",
        )],
    );
    // One shared PCM sample does not identify the same physical word.
    let boundary = (sample(0.5) - 1) as f32 / state.sample_rate as f32;
    let current = vec![
        segment("overlap", boundary, 0.9),
        segment("novel", 1.0, 1.25),
    ];
    let words = retention_words(&current);
    let (tx, mut rx) = mpsc::unbounded_channel();
    retain_apple_words_at_exit(&mut state, &tx, 2, &words, &current, "overlap_refused");
    let receipts = retention_receipts(&drain(&mut rx));
    assert_eq!(receipts[0]["words"], 2);
    assert_eq!(receipts[0]["admitted_into"][0]["count"], 2);
    assert_eq!(receipts[0]["dropped_by_slot_rules"], 0);
    assert_eq!(document(&state), "heard overlap novel");
}

#[test]
fn retained_unmatched_words_enter_later_qualified_owner_once() {
    let mut state = physical_state("retain-later-owner", 2.0, &[(0.0, 2.0)]);
    let current = vec![segment("alpha", 0.25, 0.5), segment("beta", 1.0, 1.25)];
    let words = retention_words(&current);
    let (tx, mut rx) = mpsc::unbounded_channel();
    retain_apple_words_at_exit(&mut state, &tx, 2, &words, &current, "overlap_refused");
    let receipts = retention_receipts(&drain(&mut rx));
    assert_eq!(receipts[0]["unmatched"], 2);
    assert_eq!(receipts[0]["dropped_by_slot_rules"], 0);
    assert_eq!(state.unmatched_silero_words.len(), 2);
    let owner = qualify(&mut state, 0.0, 2.0);
    state.reconciled_silero.insert(1);
    let physical = state.fusion.as_ref().unwrap().ledger().clone();
    // Current evidence authenticates slots; no fresh slice means unanchored
    // evidence instead, which has its own acceptance test below.
    for _ in 0..2 {
        reconcile_silero_ledger(&mut state, &tx, &physical, &current);
        assert!(state.unmatched_silero_words.is_empty());
        assert_eq!(document(&state), "alpha beta");
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .len(),
            2
        );
    }
    assert!(retention_receipts(&drain(&mut rx)).is_empty());
}

#[test]
fn qualification_exits_retain_words_without_retrying_qualification() {
    for evicted in [false, true] {
        let mut state = physical_state("retain-qualification", 2.0, &[(0.0, 1.0)]);
        state.audio = super::super::live_audio_buffer::LiveAudioBuffer::new(RATE, 1.0);
        if evicted {
            state.audio.push(&vec![0.25; sample(2.0) as usize]);
        } else {
            // Actual silent PCM fails the configured energy qualification.
            state.audio.push(&vec![0.0; sample(1.0) as usize]);
        }
        let current = vec![segment("Iwo", 0.25, 0.5)];
        let physical = state.fusion.as_ref().unwrap().ledger().clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        reconcile_silero_ledger(&mut state, &tx, &physical, &current);
        let receipts = retention_receipts(&drain(&mut rx));
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0]["exit"],
            if evicted {
                "qualification_failed_pcm_unavailable"
            } else {
                "qualification_failed_refused"
            }
        );
        assert_eq!(receipts[0]["words"], 1);
        assert_eq!(receipts[0]["unmatched"], 1);
        assert!(state.reconciled_silero.contains(&1));
        // Make qualification possible; the next tick must not attempt it.
        state.audio = super::super::live_audio_buffer::LiveAudioBuffer::new(RATE, 2.0);
        state.audio.push(&vec![0.25; sample(2.0) as usize]);
        reconcile_silero_ledger(&mut state, &tx, &physical, &[]);
        assert!(retention_receipts(&drain(&mut rx)).is_empty());
        assert_eq!(state.unmatched_silero_words.len(), 1);
        assert_eq!(state.unmatched_silero_words[0].text, "Iwo");
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .qualified_occurrences()
                .count(),
            0
        );
    }
}

#[test]
fn whisper_omission_does_not_consume_retained_words() {
    let mut state = physical_state("retain-omission", 3.0, &[(0.0, 2.0)]);
    let current = vec![segment("alpha", 0.25, 0.5), segment("beta", 1.0, 1.25)];
    let words = retention_words(&current);
    let (tx, mut rx) = mpsc::unbounded_channel();
    retain_apple_words_at_exit(&mut state, &tx, 2, &words, &current, "overlap_refused");
    let owner = qualify(&mut state, 0.0, 2.0);
    let whisper =
        LedgerObservationIdentity::new(LedgerObservationProducer::Whisper, 9, 0, owner.clone());
    let receipt = state.acoustic_ledger.lock().unwrap().admit_word_slots(
        &whisper,
        &[crate::pipeline::acoustic_ledger::WordPin::new(
            sample(1.5),
            sample(1.75),
            "other",
        )],
    );
    assert!(receipt.grants_mutation());
    assert_eq!(state.unmatched_silero_words.len(), 2);
    state.reconciled_silero.insert(1);
    let physical = state.fusion.as_ref().unwrap().ledger().clone();
    reconcile_silero_ledger(&mut state, &tx, &physical, &current);
    assert_eq!(document(&state), "alpha beta other");
    assert!(state.unmatched_silero_words.is_empty());
    let mut whisper = state.acoustic_ledger.lock().unwrap().next_word_observation(
        LedgerObservationProducer::Whisper,
        10,
        &owner,
    );
    let pins =
        [
            crate::pipeline::acoustic_ledger::WordPin::new(sample(1.5), sample(1.75), "revised")
                .with_decode_window(0, sample(2.0)),
        ];
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    ledger.admit_word_slots(&whisper, &pins);
    let trial = ledger
        .next_word_trial(true)
        .expect("changed word needs confirmation");
    whisper = ledger.next_word_observation(LedgerObservationProducer::Whisper, 11, &owner);
    let witness = pins
        .iter()
        .cloned()
        .map(|pin| pin.with_decode_window(0, sample(2.5)))
        .collect::<Vec<_>>();
    assert!(sample(2.5) < state.audio.session_sample_end());
    assert!(
        ledger
            .admit_word_trial(&trial, &whisper, &witness, &witness)
            .grants_mutation()
    );
    drop(ledger);
    let retained_document = document(&state);
    assert_eq!(retained_document, "alpha beta revised");
    assert_eq!(
        retained_document
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
    assert_eq!(retention_receipts(&drain(&mut rx)).len(), 1);
}

#[test]
fn late_noncurrent_words_keep_their_pins() {
    for labels in [vec!["alpha", "beta", "gamma"], vec!["Iwo"; 5]] {
        let mut state = state("late-own-pins", 6.0);
        let owner = qualify(&mut state, 0.0, 6.0);
        let words = labels
            .iter()
            .enumerate()
            .map(|(i, text)| FusionWord {
                confidence: None,
                text: (*text).into(),
                sample_start: sample(i as f32 + 0.25),
                sample_end: sample(i as f32 + 0.5),
            })
            .collect::<Vec<_>>();
        let (tx, mut rx) = mpsc::unbounded_channel();
        retain_apple_words_at_exit(&mut state, &tx, 9, &words, &[], "overlap_refused");
        let events = drain(&mut rx);
        let receipts = retention_receipts(&events);
        assert_eq!(receipts[0]["kept_unanchored"], labels.len());
        assert_eq!(receipts[0]["dropped_by_slot_rules"], 0);
        let evidence = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation,
                    receipt,
                    ..
                } => {
                    let MutationReceipt::KeepVisibleUnanchored {
                        occurrence,
                        label,
                        reason,
                    } = receipt
                    else {
                        panic!("late words must be evidence: {receipt:?}");
                    };
                    assert_eq!(*reason, NoAuthorityReason::LateAppleWordNotCurrent);
                    assert_eq!(&observation.occurrence, occurrence);
                    Some((
                        occurrence.sample_start,
                        occurrence.sample_end,
                        label.as_str(),
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            evidence,
            words
                .iter()
                .map(|word| { (word.sample_start, word.sample_end, word.text.as_str()) })
                .collect::<Vec<_>>()
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.qualified_occurrences().collect::<Vec<_>>(),
            vec![&owner]
        );
        assert!(ledger.is_empty(), "evidence must not mint occurrences");
        assert!(ledger.slots_of(&owner).is_none());
    }
}

#[test]
fn retained_noncurrent_word_is_receipted_as_unanchored_not_a_slot_or_drop() {
    let mut state = state("retain-unanchored", 2.0);
    let owner = qualify(&mut state, 0.0, 2.0);
    let words = retention_words(&[segment("Iwo", 0.25, 0.5)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    retain_apple_words_at_exit(&mut state, &tx, 2, &words, &[], "overlap_refused");
    let events = drain(&mut rx);
    let receipts = retention_receipts(&events);
    assert_eq!(receipts[0]["admitted_into"], serde_json::json!([]));
    assert_eq!(receipts[0]["kept_unanchored"], 1);
    assert_eq!(receipts[0]["unmatched"], 0);
    assert_eq!(receipts[0]["dropped_by_slot_rules"], 0);
    assert!(events.iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation {
            receipt: MutationReceipt::KeepVisibleUnanchored { label, .. }, ..
        } if label == "Iwo"
    )));
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&owner)
            .is_none()
    );
}

#[test]
fn qualification_exit_keeps_five_disjoint_iwo_words_until_late_admission() {
    let mut state = physical_state("retain-five-iwo", 5.0, &[(0.0, 5.0)]);
    let calibration = state.energy_calibration.take();
    let current = (0..5)
        .map(|i| segment("Iwo", i as f32 + 0.25, i as f32 + 0.5))
        .collect::<Vec<_>>();
    let physical = state.fusion.as_ref().unwrap().ledger().clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    reconcile_silero_ledger(&mut state, &tx, &physical, &current);
    let receipts = retention_receipts(&drain(&mut rx));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["exit"], "qualification_failed_refused");
    assert_eq!(receipts[0]["words"], 5);
    assert_eq!(receipts[0]["unmatched"], 5);
    assert_eq!(receipts[0]["dropped_by_slot_rules"], 0);
    assert_eq!(state.unmatched_silero_words.len(), 5);
    // A qualified owner arrives independently; the failed utterance itself
    // remains reconciled and must use late admission on subsequent ticks.
    state.energy_calibration = calibration;
    let owner = qualify(&mut state, 0.0, 5.0);
    for _ in 0..2 {
        reconcile_silero_ledger(&mut state, &tx, &physical, &current);
        assert_eq!(count_iwo(&document(&state)), 5);
        assert!(state.unmatched_silero_words.is_empty());
    }
    assert!(retention_receipts(&drain(&mut rx)).is_empty());
    let ledger = state.acoustic_ledger.lock().unwrap();
    let slots = ledger.slots_of(&owner).unwrap();
    assert_eq!(slots.len(), 5);
    assert!(
        slots
            .windows(2)
            .all(|pair| pair[0].sample_end < pair[1].sample_start)
    );
}

/// Existing cumulative-restatement fixture shapes: alpha/beta and five Iwo.
/// Replay must not fire a retention exit or alter a slot or seal.
#[test]
fn existing_cumulative_replay_fixtures_emit_no_retention_receipts() {
    for labels in [vec!["alpha", "beta"], vec!["Iwo"; 5]] {
        let ranges = (0..labels.len())
            .map(|i| (i as f32, (i + 1) as f32))
            .collect::<Vec<_>>();
        let mut state = physical_state("retain-replay", labels.len() as f32, &ranges);
        let current = labels
            .iter()
            .zip(&ranges)
            .map(|(text, &(start, end))| segment(text, start, end))
            .collect::<Vec<_>>();
        let (tx, mut rx) = mpsc::unbounded_channel();
        emit(&mut state, &tx, current.clone());
        assert_eq!(document(&state), labels.join(" "));
        let before = {
            let ledger = state.acoustic_ledger.lock().unwrap();
            ledger
                .occurrences()
                .map(|owner| {
                    (
                        owner.clone(),
                        ledger.slots_of(owner).unwrap().to_vec(),
                        ledger.is_sealed(owner),
                    )
                })
                .collect::<Vec<_>>()
        };
        emit(&mut state, &tx, current);
        let after = {
            let ledger = state.acoustic_ledger.lock().unwrap();
            ledger
                .occurrences()
                .map(|owner| {
                    (
                        owner.clone(),
                        ledger.slots_of(owner).unwrap().to_vec(),
                        ledger.is_sealed(owner),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(before, after);
        assert_eq!(document(&state), labels.join(" "));
        assert!(retention_receipts(&drain(&mut rx)).is_empty());
    }
}

#[test]
fn checked_in_five_iwo_replay_has_no_retention_exit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("p0_b_five_iwo_manifest.json")).unwrap(),
    )
    .unwrap();
    let pcm = hound::WavReader::open(root.join("p0_b_five_iwo.wav"))
        .unwrap()
        .samples::<i16>()
        .map(|value| f32::from(value.unwrap()) / 32768.0)
        .collect::<Vec<_>>();
    let ranges = manifest["bursts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|burst| {
            (
                burst["sample_start"].as_u64().unwrap() as f32 / RATE as f32,
                burst["sample_end"].as_u64().unwrap() as f32 / RATE as f32,
            )
        })
        .collect::<Vec<_>>();
    let mut state = physical_state("retain-fixture", pcm.len() as f32 / RATE as f32, &ranges);
    state.audio = super::super::live_audio_buffer::LiveAudioBuffer::new(RATE, 5.0);
    state.audio.push(&pcm);
    let current = ranges
        .iter()
        .map(|&(start, end)| segment("Iwo", start, end))
        .collect::<Vec<_>>();
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit(&mut state, &tx, current.clone());
    assert_eq!(document(&state), "Iwo Iwo Iwo Iwo Iwo");
    let before = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .occurrences()
        .cloned()
        .collect::<Vec<_>>();
    let slots = {
        let ledger = state.acoustic_ledger.lock().unwrap();
        before
            .iter()
            .map(|owner| {
                (
                    ledger.slots_of(owner).unwrap().to_vec(),
                    ledger.is_sealed(owner),
                )
            })
            .collect::<Vec<_>>()
    };
    emit(&mut state, &tx, current);
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.rendered_text(), "Iwo Iwo Iwo Iwo Iwo");
    assert_eq!(ledger.occurrences().cloned().collect::<Vec<_>>(), before);
    for (owner, (prior_slots, sealed)) in before.iter().zip(slots) {
        assert_eq!(ledger.slots_of(owner).unwrap(), prior_slots.as_slice());
        assert_eq!(ledger.is_sealed(owner), sealed);
    }
    assert!(retention_receipts(&drain(&mut rx)).is_empty());
}

/// Boundary acceptance requirement: expected RED by source inspection.
/// W1 leaves qualification policy unchanged and does not execute this test.
#[test]
fn qualification_refusal_must_keep_timed_apple_words_in_mirror() {
    let mut state = physical_state("mirror-qualification-refusal", 1.0, &[]);
    state.energy_calibration = None;
    state
        .fusion
        .as_mut()
        .unwrap()
        .observe(Some((sample(0.0), sample(1.0))), false, sample(1.0));
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit(&mut state, &tx, vec![segment("Iwo", 0.0, 0.8)]);
    assert!(state.published_unadmitted_words.iter().any(|word| {
        word.text == "Iwo" && matches!(word.source, UnadmittedAppleWordSource::Pending { .. })
    }));
    drain(&mut rx);

    state
        .fusion
        .as_mut()
        .unwrap()
        .observe(None, true, sample(1.0));
    seal_sliced_by_silero(&mut state, &tx, &[]);
    let events = drain(&mut rx);
    let kept_in_ledger = events.iter().any(|event| {
        matches!(event,
            EngineEvent::LedgerMutation {
                receipt: MutationReceipt::KeepVisibleUnanchored { label, .. }, ..
            } if label.split_whitespace().any(|word| word == "Iwo")
        )
    });
    assert!(
        state
            .published_unadmitted_words
            .iter()
            .any(|word| word.text == "Iwo")
            || document(&state)
                .split_whitespace()
                .any(|word| word == "Iwo")
            || kept_in_ledger,
        "qualification refusal must retain the pending Apple word in paint state"
    );
}

#[test]
fn forced_close_clears_partial_and_records_its_own_admission() {
    let mut state = physical_state("mirror-forced-close", 1.0, &[(0.0, 1.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Iwo".into(),
            segments: vec![segment("Iwo", 0.0, 0.8)],
        }],
        &tx,
        &mut state,
        1.0,
    );
    drain(&mut rx);
    seal_open_partial(&mut state, &tx, 1.0);
    assert!(state.open_partial.is_empty());
    assert!(state.open_partial_segments.is_empty());
    assert_eq!(state.phrase_id, 2);
    assert_eq!(state.finishing_phrase_id, Some(1));
    assert_eq!(
        state.closed_phrases[&1].outcomes,
        BTreeMap::from([(ApplePhraseOutcome::Admitted, 1)])
    );
    assert_eq!(document(&state), "Iwo");
    assert!(state.published_unadmitted_words.is_empty());
}

#[test]
fn untimed_final_survives_unrelated_commit_without_a_summary() {
    let mut state = physical_state("mirror-untimed", 1.0, &[(0.0, 1.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Iwo".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        1.0,
    );
    drain(&mut rx);
    emit_stream_events(
        vec![LiveStreamEvent::PhraseFinal {
            text: "Iwo".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        1.0,
    );
    let events = drain(&mut rx);
    let words = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::UnadmittedAppleWords { words, .. } => Some(words),
            _ => None,
        })
        .expect("untimed mirror");
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].sample_start, words[0].sample_end);
    assert!(matches!(
        words[0].source,
        UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 }
    ));
    assert!(document(&state).is_empty());
    // A different phrase with a committed range containing the old pin
    // cannot consume phrase 1's untimed words.
    emit(&mut state, &tx, vec![segment("earlier", 0.0, 0.8)]);
    assert_eq!(state.refused_untimed_words.len(), 1);
    assert_eq!(
        state.closed_phrases[&1].outcomes,
        BTreeMap::from([(ApplePhraseOutcome::Untimed, 1)])
    );
    assert!(
        state
            .published_unadmitted_words
            .iter()
            .any(|word| word.text == "Iwo")
    );
    drain(&mut rx);
}

#[test]
fn timed_summary_replaces_untimed_phrase_with_the_same_id() {
    let mut state = physical_state("mirror-timed-summary", 63.0, &[(60.2, 62.0)]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.open_partial_pin = sample(0.8);
    emit_stream_events(
        vec![LiveStreamEvent::PhraseFinal {
            text: "Iwo Iwo".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        63.0,
    );
    assert_eq!(state.phrase_id, 2);
    assert_eq!(state.refused_untimed_words[&1].len(), 2);
    state.stop_trailing_finish = true;
    emit_stream_events(
        vec![LiveStreamEvent::Summary {
            text: "Iwo Iwo".into(),
            segments: vec![segment("Iwo", 60.2, 61.0), segment("Iwo", 61.1, 61.9)],
            ok: true,
            error: None,
        }],
        &tx,
        &mut state,
        63.0,
    );
    assert!(state.refused_untimed_words.is_empty());
    assert_eq!(state.phrase_id, 2, "a re-seal keeps the phrase id");
    assert_eq!(state.closed_phrases.len(), 1);
    assert_eq!(
        state.closed_phrases[&1].outcomes,
        BTreeMap::from([(ApplePhraseOutcome::Admitted, 2)])
    );
    assert!(state.closed_phrases[&1].was_untimed);
    assert_eq!(state.closed_phrases[&1].arrival_index, 0);
    assert_eq!(count_iwo(&document(&state)), 2);
    assert!(state.published_unadmitted_words.is_empty());
    drain(&mut rx);
}

#[test]
fn untimed_final_and_summary_replace_one_phrase_without_deduplicating_words() {
    let mut state = physical_state("untimed-summary", 1.0, &[]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    emit_stream_events(
        vec![
            LiveStreamEvent::Partial {
                text: "Iwo Iwo".into(),
                segments: Vec::new(),
            },
            LiveStreamEvent::PhraseFinal {
                text: "Iwo Iwo".into(),
                segments: Vec::new(),
            },
            LiveStreamEvent::Summary {
                text: "Iwo Iwo".into(),
                segments: Vec::new(),
                ok: true,
                error: None,
            },
        ],
        &tx,
        &mut state,
        1.0,
    );
    let mirrors = drain(&mut rx)
        .into_iter()
        .filter_map(|event| match event {
            EngineEvent::UnadmittedAppleWords { words, .. } => Some(words),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(mirrors.len(), 1, "summary leaves the held phrase unchanged");
    assert_eq!(
        mirrors[0].len(),
        2,
        "two spoken words survive at one untimed pin"
    );
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
    assert_eq!(
        state.refused_untimed_words.len(),
        2,
        "new phrase keeps its own words"
    );
    assert_eq!(state.published_unadmitted_words.len(), 3);
}

#[test]
fn armed_epoch_close_summary_does_not_readmit_retimed_words() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, _tail_rx) = mpsc::channel(1);
    let mut state = physical_state("epoch-summary", 3.0, &[(0.0, 2.0)]);
    state.tail_patch = Some(tail_tx);
    let owner = OccurrenceIdentity::new("epoch-summary", 7, 0, sample(2.0));
    emit(
        &mut state,
        &tx,
        vec![segment("alpha", 0.0, 0.5), segment("beta", 1.0, 1.5)],
    );
    assert!(state.utterance_id > 0);
    assert!(!state.stop_trailing_finish);
    {
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(!ledger.is_sealed(&owner));
        assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
    }
    drain(&mut rx);

    let mut epoch = EpochGate::armed(RATE, 1.0);
    assert!(matches!(
        epoch.feed_pcm(&[0.25; 480], sample(2.0), true),
        EpochDecision::Wake { .. }
    ));
    assert!(matches!(
        epoch.feed_pcm(&vec![0.0; RATE as usize], sample(3.0), false),
        EpochDecision::Sleep { .. }
    ));
    // The Sleep arm delivers finish events before sealing its residue.
    // Retiming alpha off its slot would create a duplicate on this open
    // owner if arming alone admitted the restating summary.
    emit_stream_events(
        vec![LiveStreamEvent::Summary {
            text: "alpha beta".into(),
            segments: vec![segment("alpha", 0.6, 0.9), segment("beta", 1.0, 1.5)],
            ok: true,
            error: None,
        }],
        &tx,
        &mut state,
        3.0,
    );
    assert!(drain(&mut rx).is_empty(), "summary must emit no mutation");
    assert!(!state.stop_trailing_finish);
    seal_open_partial(&mut state, &tx, 3.0);
    state.close_admission_horizon(&tx, sample(3.0));
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.rendered_text(), "alpha beta");
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
    ledger.assert_slot_labels();
}

#[test]
fn stop_finish_error_clears_summary_window() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
    let mut state = physical_state("stop-error", 1.0, &[(0.0, 1.0)]);
    state.stop_trailing_finish = true;
    let result = finish_capture_after_seal(
        &mut state,
        &tx,
        1.0,
        ack_tx,
        false,
        || Err(anyhow::anyhow!("finish failed")),
        |_| panic!("finish failure must not close residue"),
    );
    assert!(result.is_err());
    assert!(!state.stop_trailing_finish);
}

#[test]
fn unarmed_stop_ack_follows_finish_and_partial_seal() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut state = state("unarmed-stop", 2.0);
    state.fusion_seal_armed = false;
    qualify(&mut state, 0.0, 1.0);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "alpha".into(),
            segments: vec![segment("alpha", 0.0, 1.0)],
        }],
        &tx,
        &mut state,
        2.0,
    );
    let ledger = Arc::clone(&state.acoustic_ledger);
    finish_capture_after_seal(
        &mut state,
        &tx,
        2.0,
        ack_tx,
        false,
        || {
            assert_eq!(
                ack_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            assert!(ledger.lock().unwrap().rendered_text().is_empty());
            Ok(vec![LiveStreamEvent::Summary {
                text: "alpha".into(),
                segments: vec![segment("alpha", 0.0, 1.0)],
                ok: true,
                error: None,
            }])
        },
        |state| close_stop_residue(state, &tx, 2.0),
    )
    .unwrap();
    assert_eq!(ack_rx.try_recv(), Ok(()));
    assert_eq!(document(&state), "alpha");
    assert!(drain(&mut rx).iter().any(|event| matches!(
        event,
        EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
    )));
}

#[test]
fn armed_short_take_ack_waits_for_first_finish_text_and_residue() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut state = physical_state("short-stop", 1.0, &[(0.0, 1.0)]);
    assert!(!state.has_stop_canvas_text());
    assert_eq!(state.sealed_count, 0);
    assert!(state.open_partial.is_empty());
    let ack_before_finish = state.has_stop_canvas_text();
    seal_sliced_by_silero(&mut state, &tx, &[]);
    seal_open_partial(&mut state, &tx, 1.0);
    state.stop_trailing_finish = true;
    finish_capture_after_seal(
        &mut state,
        &tx,
        1.0,
        ack_tx,
        ack_before_finish,
        || {
            assert_eq!(
                ack_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            Ok(vec![LiveStreamEvent::Partial {
                text: "Tak".into(),
                segments: vec![segment("Tak", 0.0, 0.8)],
            }])
        },
        |state| close_stop_residue(state, &tx, 1.0),
    )
    .unwrap();
    assert_eq!(ack_rx.try_recv(), Ok(()));
    assert_eq!(document(&state), "Tak");
    assert!(state.has_stop_canvas_text());
    assert!(state.open_partial.is_empty());
    assert!(state.unmatched_silero_words.is_empty());
    assert!(drain(&mut rx).iter().any(|event| matches!(
        event,
        EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
    )));
}

#[test]
fn rejected_raw_callback_does_not_release_empty_stop_before_valid_finish() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut state = physical_state("rejected-stop", 1.0, &[(0.0, 1.0)]);
    emit_stream_events(
        vec![LiveStreamEvent::PhraseFinal {
            text: "Tak".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        1.0,
    );
    assert!(drain(&mut rx).iter().any(|event| matches!(
        event,
        EngineEvent::Warning { code, .. } if code == "apple_final_without_pcm_timing"
    )));
    assert!(document(&state).is_empty());
    let ack_before_finish = state.has_stop_canvas_text();
    assert!(!ack_before_finish);
    seal_sliced_by_silero(&mut state, &tx, &[]);
    seal_open_partial(&mut state, &tx, 1.0);
    state.stop_trailing_finish = true;
    finish_capture_after_seal(
        &mut state,
        &tx,
        1.0,
        ack_tx,
        ack_before_finish,
        || {
            assert_eq!(
                ack_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            Ok(vec![LiveStreamEvent::Summary {
                text: "Tak".into(),
                segments: vec![segment("Tak", 0.0, 0.8)],
                ok: true,
                error: None,
            }])
        },
        |state| close_stop_residue(state, &tx, 1.0),
    )
    .unwrap();
    assert_eq!(ack_rx.try_recv(), Ok(()));
    assert_eq!(document(&state), "Tak");
    assert!(state.unmatched_silero_words.is_empty());
}

#[test]
fn internal_seal_ack_precedes_finish_and_partial_is_already_committed() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut state = physical_state("stop-order", 2.0, &[(0.0, 2.0)]);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "alpha beta".into(),
            segments: vec![segment("alpha", 0.0, 0.5), segment("beta", 1.0, 1.5)],
        }],
        &tx,
        &mut state,
        2.0,
    );
    if let Some(fusion) = state.fusion.as_mut() {
        fusion.flush(sample(2.0));
    }
    seal_sliced_by_silero(&mut state, &tx, &[]);
    seal_open_partial(&mut state, &tx, 2.0);
    state.stop_trailing_finish = true;
    assert!(state.has_stop_canvas_text());
    let ledger = Arc::clone(&state.acoustic_ledger);
    let mut finish_called = false;
    finish_capture_after_seal(
        &mut state,
        &tx,
        2.0,
        ack_tx,
        true,
        || {
            assert_eq!(ack_rx.try_recv(), Ok(()));
            assert_eq!(ledger.lock().unwrap().rendered_text(), "alpha beta");
            let before_finish = drain(&mut rx);
            assert!(before_finish.iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
            )));
            finish_called = true;
            Ok(vec![LiveStreamEvent::Error {
                message: "finish-observed".into(),
            }])
        },
        |state| close_stop_residue(state, &tx, 2.0),
    )
    .unwrap();
    assert!(finish_called);
    assert!(drain(&mut rx).iter().any(|event| matches!(
        event,
        EngineEvent::NoSpeech { reason } if reason.contains("finish-observed")
    )));
    assert!(state.open_partial.is_empty());
}

#[test]
fn trailing_final_after_stop_ack_keeps_owner_receipts() {
    for observer_open in [false, true] {
        for summary in [false, true] {
            for case in 0..3 {
                let (tx, mut rx) = mpsc::unbounded_channel();
                let (tail_tx, _tail_rx) = mpsc::channel(1);
                let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
                let mut state = physical_state("stop-final", 2.0, &[(0.0, 2.0)]);
                if observer_open {
                    state.tail_patch = Some(tail_tx);
                }
                let owner = OccurrenceIdentity::new("stop-final", 7, 0, sample(2.0));
                let mut words = vec![segment("alpha", 0.0, 0.5), segment("beta", 1.0, 1.5)];
                emit_stream_events(
                    vec![LiveStreamEvent::Partial {
                        text: "alpha beta".into(),
                        segments: words.clone(),
                    }],
                    &tx,
                    &mut state,
                    2.0,
                );
                seal_sliced_by_silero(&mut state, &tx, &[]);
                seal_open_partial(&mut state, &tx, 2.0);
                assert!(state.utterance_id > 0);
                state.stop_trailing_finish = true;
                assert_eq!(document(&state), "alpha beta");
                assert_eq!(
                    state.acoustic_ledger.lock().unwrap().is_sealed(&owner),
                    !observer_open
                );
                let (seal_before, slots_before) = {
                    let ledger = state.acoustic_ledger.lock().unwrap();
                    (
                        ledger.seal_of(&owner).cloned(),
                        ledger.slots_of(&owner).unwrap().to_vec(),
                    )
                };
                drain(&mut rx);
                if case == 1 {
                    words[1].text = "changed".into();
                } else if case == 2 {
                    words.push(segment("gamma", 1.6, 1.9));
                }
                let text = words
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let event = if summary {
                    LiveStreamEvent::Summary {
                        text,
                        segments: words,
                        ok: true,
                        error: None,
                    }
                } else {
                    LiveStreamEvent::PhraseFinal {
                        text,
                        segments: words,
                    }
                };
                finish_capture_after_seal(
                    &mut state,
                    &tx,
                    2.0,
                    ack_tx,
                    true,
                    || {
                        assert_eq!(ack_rx.try_recv(), Ok(()));
                        Ok(vec![event])
                    },
                    |state| close_stop_residue(state, &tx, 2.0),
                )
                .unwrap();
                assert!(!state.stop_trailing_finish);
                seal_sliced_by_silero(&mut state, &tx, &[]);
                let events = drain(&mut rx);
                match case {
                    0 => {
                        assert!(
                            events.is_empty(),
                            "identical callback is already reconciled"
                        );
                    }
                    1 => {
                        if observer_open {
                            assert_eq!(document(&state), "alpha changed");
                            assert!(events.iter().any(|event| matches!(event,
                                    EngineEvent::LedgerMutation { label, receipt, .. }
                                        if label == "alpha changed" && receipt.grants_mutation())));
                            let ledger = state.acoustic_ledger.lock().unwrap();
                            assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
                            assert!(ledger.layer_trail_for(&owner).any(|entry| {
                                entry.candidate_label == "alpha beta"
                                    && entry.decision.grants_mutation()
                            }));
                        } else {
                            assert!(events.iter().any(|event| matches!(event,
                                    EngineEvent::LedgerMutation { label,
                                        receipt: MutationReceipt::KeepVisibleUnanchored {
                                            reason: NoAuthorityReason::LateAppleWordSealedOwner, .. }, .. }
                                        if label == "alpha changed")));
                            assert_eq!(document(&state), "alpha beta");
                            let ledger = state.acoustic_ledger.lock().unwrap();
                            assert_eq!(ledger.seal_of(&owner), seal_before.as_ref());
                            assert_eq!(ledger.slots_of(&owner).unwrap(), slots_before.as_slice());
                            drop(ledger);
                            assert_sealed_apple_refusal(&state, &owner);
                        }
                    }
                    _ => {
                        if observer_open {
                            for word in ["alpha", "beta"] {
                                assert!(events.iter().any(|event| matches!(
                                    event,
                                    EngineEvent::LedgerMutation {
                                        label,
                                        receipt: MutationReceipt::Refuse {
                                            reason: RefuseReason::ReplayedRangeIdentity,
                                            ..
                                        },
                                        ..
                                    } if label == word
                                )));
                            }
                            assert_eq!(document(&state), "alpha beta gamma");
                            assert_eq!(
                                state
                                    .acoustic_ledger
                                    .lock()
                                    .unwrap()
                                    .slots_of(&owner)
                                    .unwrap()
                                    .len(),
                                3
                            );
                            assert!(events.iter().any(|event| matches!(
                                event,
                                EngineEvent::LedgerMutation { label, receipt, .. }
                                    if label == "alpha beta gamma" && receipt.grants_mutation()
                            )));
                        } else {
                            assert_eq!(document(&state), "alpha beta");
                            assert!(events.iter().any(|event| matches!(
                                event,
                                EngineEvent::LedgerMutation {
                                    label,
                                    receipt: MutationReceipt::KeepVisibleUnanchored {
                                        reason: NoAuthorityReason::LateAppleWordSealedOwner,
                                        ..
                                    },
                                    ..
                                } if label == "alpha beta gamma"
                            )));
                        }
                    }
                }
                assert!(state.unmatched_silero_words.is_empty());
                assert!(state.open_partial.is_empty());
                state.acoustic_ledger.lock().unwrap().assert_slot_labels();
            }
        }
    }
}

#[test]
fn fusion_lexicon_admits_bundled_canonical_after_apple() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut state = physical_state("bundled-lexicon", 2.0, &[(0.0, 1.0)]);
    state.lexicon_custom_path = dir.path().join("lexicon.custom.jsonl");
    emit(&mut state, &tx, vec![segment("accepromazyna", 0.0, 1.0)]);
    assert_eq!(document(&state), "Acepromazyna");
    let events = drain(&mut rx);
    assert!(events.iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation { observation, label, receipt }
        if observation.producer == LedgerObservationProducer::Lexicon
            && label == "Acepromazyna" && receipt.grants_mutation()
    )));
}

#[test]
fn fusion_lexicon_registered_merge_conserves_disjoint_source_pcm() {
    use crate::pipeline::acoustic_ledger::SlotOperationKind;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut state = physical_state("embedded-phrase", 2.0, &[(0.0, 1.0)]);
    state.lexicon_custom_path = dir.path().join("absent.jsonl");
    emit(
        &mut state,
        &tx,
        vec![segment("code", 0.1, 0.3), segment("scribe", 0.5, 0.8)],
    );
    assert_eq!(document(&state), "Codescribe");
    let ledger = state.acoustic_ledger.lock().unwrap();
    let operation = ledger
        .slot_operations()
        .iter()
        .find(|op| op.kind == SlotOperationKind::Merge && op.outputs[0].text == "Codescribe")
        .expect("registered phrase must use the explicit source-conserving merge");
    assert_eq!(operation.sources.len(), 2);
    assert_eq!(
        operation.source_ranges,
        vec![
            OccurrenceIdentity::new(
                "embedded-phrase",
                state.capture_epoch,
                sample(0.1),
                sample(0.3)
            ),
            OccurrenceIdentity::new(
                "embedded-phrase",
                state.capture_epoch,
                sample(0.5),
                sample(0.8)
            ),
        ]
    );
    assert_eq!(
        ledger.slot_source_ranges(&operation.outputs[0]),
        operation.source_ranges
    );
    assert_eq!(ledger.conservation().residue(), 0);
    drop(ledger);
    assert!(drain(&mut rx).iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation { observation, label, receipt }
        if observation.producer == LedgerObservationProducer::Lexicon
            && label == "Codescribe" && receipt.grants_mutation()
    )));
}

#[test]
fn fusion_lexicon_fixture_custom_entry_wins_over_bundled_variant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lexicon.custom.jsonl");
    std::fs::write(
        &path,
        "{\"term\":\"CustomDrug\",\"mispronunciations\":[\"accepromazyna\"]}\n",
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut state = physical_state("custom-lexicon", 2.0, &[(0.0, 1.0)]);
    state.lexicon_custom_path = path;
    emit(&mut state, &tx, vec![segment("accepromazyna", 0.0, 1.0)]);
    assert_eq!(document(&state), "CustomDrug");
}

#[test]
fn fusion_dictation_preserves_ordinary_polish_command_words() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut state = physical_state("ordinary-polish", 2.0, &[(0.0, 1.0)]);
    state.lexicon_custom_path = dir.path().join("lexicon.custom.jsonl");
    emit(
        &mut state,
        &tx,
        vec![segment("schowek i zaznaczenie", 0.0, 1.0)],
    );
    assert_eq!(document(&state), "schowek i zaznaczenie");
}

#[test]
fn whisper_label_is_rewritten_before_ledger_admission() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut state = state("whisper-lexicon", 2.0);
    state.lexicon_custom_path = dir.path().join("lexicon.custom.jsonl");
    let occurrence = qualify(&mut state, 0.0, 1.0);
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .schedule_frontier(occurrence.clone(), [LedgerObservationProducer::Whisper]);
    let receipt = admit_ledger_label(
        &mut state,
        &tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                1,
                0,
                occurrence,
            ),
            label: "accepromazyna",
            energy: EnergyAdmission::RequireExistingQualification,
        },
    );
    assert!(receipt.is_some_and(|receipt| receipt.grants_mutation()));
    assert_eq!(document(&state), "Acepromazyna");
    assert!(drain(&mut rx).iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation { observation, label, .. }
        if observation.producer == LedgerObservationProducer::Whisper
            && label == "Acepromazyna"
    )));
}

#[test]
fn emit_maps_partial_and_two_phrase_finals() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("two-finals", 2.0, &[(0.0, 1.0), (1.0, 2.0)]);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "hello".into(),
            segments: vec![segment("hello", 0.0, 0.5)],
        }],
        &tx,
        &mut state,
        0.5,
    );
    assert!(document(&state).is_empty(), "preview cannot admit words");
    emit(&mut state, &tx, vec![segment("hello world", 0.0, 1.0)]);
    emit(&mut state, &tx, vec![segment("second", 1.0, 2.0)]);
    let events = drain(&mut rx);
    assert!(matches!(&events[0], EngineEvent::Preview { rev: 1, text, .. } if text == "hello"));
    let ids = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::UtteranceFinal { utterance_id, .. } => Some(*utterance_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(state.sealed_count, 2);
    assert_eq!(document(&state), "hello world second");
}

/// The old test synthesized a new window from stale timing and novel text.
/// Current contract: novelty alone is not PCM identity. A later exact
/// observation preserves the novel phrase without rewriting the first one.
#[test]
fn boundary_consumed_final_with_novel_text_requires_exact_pcm() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("stale-final", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
    emit(&mut state, &tx, vec![segment("prior", 0.0, 1.0)]);
    drain(&mut rx);
    emit(&mut state, &tx, vec![segment("novel phrase", 0.0, 1.0)]);
    assert_eq!(document(&state), "prior");
    assert!(raw_finals(&drain(&mut rx)).is_empty());
    emit(&mut state, &tx, vec![segment("novel phrase", 2.0, 3.0)]);
    assert_eq!(document(&state), "prior novel phrase");
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        2
    );
}

/// Untimed preview cannot be promoted into committed speech. Preserve the
/// refusal and prove that the same words with actual timing can still land.
#[test]
fn untimed_partial_revision_requires_exact_pcm_without_synthetic_final() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("untimed-freeze", 3.0, &[(0.0, 2.0)]);
    let prior = "whole prior utterance retained for exact observation";
    emit_stream_events(
        vec![
            LiveStreamEvent::Partial {
                text: prior.into(),
                segments: Vec::new(),
            },
            LiveStreamEvent::Partial {
                text: "Next".into(),
                segments: Vec::new(),
            },
        ],
        &tx,
        &mut state,
        2.0,
    );
    let events = drain(&mut rx);
    assert!(!events.iter().any(|event| matches!(event,
        EngineEvent::Warning { code, .. } if code == "apple_final_without_pcm_timing"
    )));
    assert!(raw_finals(&events).is_empty());
    assert!(document(&state).is_empty());
    emit(&mut state, &tx, vec![segment(prior, 0.0, 2.0)]);
    assert_eq!(document(&state), prior);
}

/// Retention now preserves the bounded tail for terminal gap repair; the
/// old immediate-release assertion would destroy required recovery PCM.
#[test]
fn seals_resolve_their_audio_window_from_retained_pcm() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("retained-seals", 6.0);
    qualify(&mut state, 0.5, 2.0);
    qualify(&mut state, 2.5, 4.0);
    emit(&mut state, &tx, vec![segment("first", 0.5, 2.0)]);
    emit(&mut state, &tx, vec![segment("second", 2.5, 4.0)]);
    assert_eq!(state.sealed_count, 2);
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["first", "second"]);
    assert_eq!(state.unresolved_windows, 0);
    assert_eq!(
        state
            .audio
            .window_by_samples(0, u64::from(RATE))
            .unwrap()
            .samples,
        vec![0.25; RATE as usize]
    );
    let second = state
        .audio
        .window_by_samples(sample(2.5), sample(4.0))
        .unwrap();
    assert_eq!(
        (second.sample_start, second.sample_end),
        (sample(2.5), sample(4.0))
    );
    assert_eq!(
        second.samples,
        vec![0.25; (sample(4.0) - sample(2.5)) as usize]
    );
}

#[test]
fn cumulative_apple_final_commits_only_segments_after_last_boundary() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("cumulative-segments", 4.0);
    qualify(&mut state, 0.0, 2.0);
    qualify(&mut state, 2.0, 3.0);
    emit(
        &mut state,
        &tx,
        vec![segment("alpha", 0.0, 1.0), segment("beta", 1.0, 2.0)],
    );
    emit(
        &mut state,
        &tx,
        vec![
            segment("alpha", 0.0, 1.0),
            segment("beta", 1.0, 2.0),
            segment("gamma", 2.0, 3.0),
        ],
    );
    let events = drain(&mut rx);
    let finals = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::UtteranceFinal {
                raw_text,
                start_ts,
                end_ts,
                ..
            } => Some((raw_text.as_str(), *start_ts, *end_ts)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finals, vec![("alpha beta", 0.0, 2.0), ("gamma", 2.0, 3.0)]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::Warning { code, .. } if code == APPLE_FINAL_OVERLAP_WARNING_CODE
            ))
            .count(),
        1
    );
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        2
    );
}

#[test]
fn cumulative_final_commits_only_its_exact_novel_suffix() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("novel-suffix", 3.0, &[(0.0, 2.0), (2.0, 3.0)]);
    emit(&mut state, &tx, vec![segment("alpha beta", 0.0, 2.0)]);
    emit(
        &mut state,
        &tx,
        vec![segment("alpha beta revised", 0.0, 2.0)],
    );
    assert_eq!(
        document(&state),
        "alpha beta",
        "stale span cannot authenticate a suffix"
    );
    emit(
        &mut state,
        &tx,
        vec![
            segment("alpha beta", 0.0, 2.0),
            segment("revised", 2.0, 3.0),
        ],
    );
    assert_eq!(document(&state), "alpha beta revised");
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["alpha beta", "revised"]);
}

#[test]
fn five_spoken_occurrences_survive_a_cumulative_restatement_end_to_end() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let ranges = (0..5)
        .map(|i| (i as f32, (i + 1) as f32))
        .collect::<Vec<_>>();
    let mut state = physical_state("five-restated", 5.0, &ranges);
    let words = ranges
        .iter()
        .map(|&(start, end)| segment("Iwo", start, end))
        .collect::<Vec<_>>();
    emit(&mut state, &tx, words[..4].to_vec());
    assert_eq!(count_iwo(&document(&state)), 4);
    emit(&mut state, &tx, words);
    assert_eq!(count_iwo(&document(&state)), 5);
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["Iwo"; 5]);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        5
    );
}

/// Prefix identity is acoustic even after Lexicon changes its spelling.
/// Drive the configured seal path, then replay Apple against the sealed
/// prefix: no text matcher is permitted to mint another occurrence.
#[test]
fn cumulative_final_prefix_survives_words_the_lexicon_rewrites() {
    with_lexicon_fixture(
        "cumulative_final_prefix_survives_words_the_lexicon_rewrites",
        || {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut state = state("rewritten-prefix", 3.0);
            let occurrence = qualify(&mut state, 0.0, 2.0);
            emit(&mut state, &tx, vec![segment("uruchom doker", 0.0, 2.0)]);
            assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
            assert_eq!(document(&state), "uruchom Docker");
            state.fusion_seal_armed = true;
            let mut fusion =
                SileroIngress::new(RATE, state.session_id.clone(), state.capture_epoch);
            fusion.observe(Some((0, sample(2.0))), true, sample(2.0));
            fusion.observe(Some((sample(2.0), sample(3.0))), true, sample(3.0));
            state.fusion = Some(fusion);
            emit(
                &mut state,
                &tx,
                vec![
                    segment("uruchom doker", 0.0, 2.0),
                    segment("i restart", 2.0, 3.0),
                ],
            );
            assert_eq!(document(&state), "uruchom Docker i restart");
            assert_eq!(
                state.acoustic_ledger.lock().unwrap().occurrences().count(),
                2
            );
            assert!(drain(&mut rx).iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation {
                    receipt: MutationReceipt::KeepVisibleUnanchored {
                        reason: NoAuthorityReason::LateAppleWordSealedOwner,
                        ..
                    },
                    ..
                }
            )));
        },
    );
}

#[test]
fn late_apple_restatement_over_lexicon_respelled_open_slot_is_refused_by_range() {
    with_lexicon_fixture(
        "late_apple_restatement_over_lexicon_respelled_open_slot_is_refused_by_range",
        || {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (sender, _requests) = mpsc::channel(8);
            let mut state = physical_state("rewritten-open", 3.0, &[(0.0, 2.0)]);
            state.tail_patch = Some(sender);
            let words = vec![segment("uruchom doker", 0.0, 2.0)];
            emit(&mut state, &tx, words.clone());
            let owner = OccurrenceIdentity::new("rewritten-open", 7, 0, sample(2.0));
            let before = {
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert!(!ledger.is_sealed(&owner));
                assert_eq!(ledger.text_of(&owner), Some("uruchom Docker"));
                ledger.slots_of(&owner).unwrap().to_vec()
            };
            drain(&mut rx);
            let replay = FusionWord {
                confidence: None,
                text: "uruchom doker".into(),
                sample_start: 0,
                sample_end: sample(2.0),
            };
            admit_late_apple_words(&mut state, &tx, 2, &[replay], &words);
            assert!(drain(&mut rx).iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation {
                    receipt: MutationReceipt::Refuse {
                        reason: RefuseReason::SlotAdmissionRejected,
                        ..
                    },
                    ..
                }
            )));
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
            assert_eq!(ledger.text_of(&owner), Some("uruchom Docker"));
            assert!(!ledger.is_sealed(&owner));
        },
    );
}

/// The old explicit empty Preview event belongs to presentation. Here the
/// current owner clears its volatile state and emits no duplicate final.
#[test]
fn fully_reheard_cumulative_final_clears_preview_instead_of_repeating_canvas() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("reheard", 3.0, &[(0.0, 2.0)]);
    emit(&mut state, &tx, vec![segment("heard phrase", 0.0, 2.0)]);
    drain(&mut rx);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "heard phrase".into(),
            segments: vec![segment("heard phrase", 0.0, 2.0)],
        }],
        &tx,
        &mut state,
        3.0,
    );
    assert_eq!(state.open_partial, "heard phrase");
    drain(&mut rx);
    emit(&mut state, &tx, vec![segment("heard phrase", 0.0, 2.0)]);
    let events = drain(&mut rx);
    assert!(state.open_partial.is_empty());
    assert!(state.open_partial_segments.is_empty());
    assert!(raw_finals(&events).is_empty());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::Preview { text, .. } if !text.is_empty()))
    );
    assert_eq!(document(&state), "heard phrase");
}

#[test]
fn legitimate_repeated_words_survive_disjoint_apple_windows() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("repeated-tak", 3.0, &[(0.0, 1.0), (1.0, 2.0)]);
    emit(&mut state, &tx, vec![segment("tak", 0.0, 1.0)]);
    emit(&mut state, &tx, vec![segment("tak", 1.0, 2.0)]);
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["tak", "tak"]);
    assert_eq!(document(&state), "tak tak");
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        2
    );
}

#[test]
fn refused_future_callback_does_not_advance_accepted_apple_cursor() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("future-cursor", 2.0);
    emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
    assert!(document(&state).is_empty());
    assert!(raw_finals(&drain(&mut rx)).is_empty());
    assert_eq!(state.last_apple_segment_end, 0.0);
}

#[test]
fn refused_future_callback_preserves_later_in_capture_apple_final() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("future-then-owned", 2.0);
    let owner = qualify(&mut state, 0.0, 1.5);
    emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
    drain(&mut rx);
    emit(&mut state, &tx, vec![segment("owned phrase", 0.0, 1.5)]);
    assert_eq!(document(&state), "owned phrase");
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["owned phrase"]);
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.occurrences().count(), 1);
    assert_eq!(ledger.text_of(&owner), Some("owned phrase"));
    assert!(ledger.is_sealed(&owner));
    assert_eq!(state.last_apple_segment_end, 1.5);
}

#[test]
fn refused_future_callback_cannot_suppress_available_sibling() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("mixed-future-owned", 2.0);
    qualify(&mut state, 0.0, 1.5);
    emit(
        &mut state,
        &tx,
        vec![
            segment("future phrase", 8.0, 9.0),
            segment("owned phrase", 0.0, 1.5),
        ],
    );
    assert_eq!(document(&state), "owned phrase");
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["owned phrase"]);
    assert_eq!(state.last_apple_segment_end, 1.5);
}

#[test]
fn available_apple_callback_consumes_boundary_and_rejects_replay() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("owned-cursor-control", 2.0);
    qualify(&mut state, 0.0, 1.5);
    emit(&mut state, &tx, vec![segment("owned phrase", 0.0, 1.5)]);
    assert_eq!(state.last_apple_segment_end, 1.5);
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["owned phrase"]);
    emit(&mut state, &tx, vec![segment("owned phrase", 0.0, 1.5)]);
    assert!(raw_finals(&drain(&mut rx)).is_empty());
    assert_eq!(document(&state), "owned phrase");
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        1
    );
}

/// A refused future callback is not a planned PCM window failure.
#[test]
fn future_callback_refusal_does_not_count_as_unresolved_planned_window() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("future-window", 2.0);
    emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
    assert_eq!(state.unresolved_windows, 0);
    assert_eq!(state.last_apple_segment_end, 0.0);
    assert_eq!(state.sealed_count, 0);
    assert!(raw_finals(&drain(&mut rx)).is_empty());
    assert!(document(&state).is_empty());
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        0
    );
}

/// Real event producer and real channel, with sender still open at assert.
/// No hand-written select loop masquerading as the session implementation.
#[tokio::test]
async fn live_previews_surface_before_audio_eof() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("preview-before-eof", 1.0);
    for text in ["a", "ab"] {
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: text.into(),
                segments: vec![segment(text, 0.0, 0.5)],
            }],
            &tx,
            &mut state,
            0.5,
        );
    }
    for (expected_rev, expected) in [(1, "a"), (2, "ab")] {
        // Each open partial is also mirrored as unadmitted words; this
        // test reads only the preview stream.
        let event = loop {
            let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("preview before EOF")
                .expect("producer stays open");
            if !matches!(event, EngineEvent::UnadmittedAppleWords { .. }) {
                break event;
            }
        };
        assert!(
            matches!(event, EngineEvent::Preview { rev, text, .. } if rev == expected_rev && text == expected)
        );
    }
    assert!(!rx.is_closed());
    assert!(document(&state).is_empty());
    drop(tx);
}

fn only_preview(events: &[EngineEvent]) -> serde_json::Value {
    let previews = events
        .iter()
        .filter(|event| matches!(event, EngineEvent::Preview { .. }))
        .collect::<Vec<_>>();
    assert_eq!(previews.len(), 1, "one partial paints one preview");
    serde_json::to_value(previews[0]).unwrap()
}

/// Counterexample B (Roman, 2026-09-24): a partial with text and word
/// segments became a Preview with no PCM range — the pins were dropped
/// before the emitter painted. The preview must carry the range those
/// segments occupy on the capture counter, at word grain.
#[test]
fn partial_with_segments_paints_its_capture_range() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("preview-pins", 2.0);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "dzień dobry".into(),
            segments: vec![segment("dzień", 0.25, 0.5), segment("dobry", 0.5, 1.0)],
        }],
        &tx,
        &mut state,
        1.0,
    );
    let preview = only_preview(&drain(&mut rx));
    assert_eq!(preview["text"], "dzień dobry");
    assert_eq!(
        preview["pin"]["range"]["sample_start"],
        sample(0.25),
        "the preview paint carries no PCM range: {preview}"
    );
    assert_eq!(preview["pin"]["range"]["sample_end"], sample(1.0));
    assert_eq!(preview["pin"]["range"]["session"], "preview-pins");
    assert_eq!(preview["pin"]["range"]["capture_epoch"], 7);
    assert_eq!(preview["pin"]["grain"], "word");
    assert!(document(&state).is_empty(), "a preview never admits words");
}

/// Counterexample B, second half: Swift may send a partial with text and
/// no segments after filtering. That preview was painted with no receipt.
/// It must paint the open occurrence's capture range as utterance grain
/// with one named receipt — never invented per-word ranges.
#[test]
fn partial_without_segments_paints_the_open_occurrence_at_utterance_grain() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = state("preview-unpinned", 2.0);
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "bez pinów".into(),
            segments: Vec::new(),
        }],
        &tx,
        &mut state,
        1.5,
    );
    let preview = only_preview(&drain(&mut rx));
    assert_eq!(preview["text"], "bez pinów");
    assert_eq!(
        preview["pin"]["grain"], "utterance",
        "a segment-less partial was painted with no grain or receipt: {preview}"
    );
    assert_eq!(preview["pin"]["receipt"], "partial_without_segments");
    assert_eq!(preview["pin"]["range"]["sample_start"], 0);
    assert_eq!(preview["pin"]["range"]["sample_end"], sample(1.5));
    assert!(document(&state).is_empty(), "a preview never admits words");
}

/// Run the same named test in an isolated process with a real custom table.
/// Command::env changes only the child's launch environment. Other tests
/// keep their own config, and the test never edits the Founder's dictionary.
/// The marker chooses the child arm; it is private to this fresh tempdir.
fn with_lexicon_fixture(name: &str, check: impl FnOnce()) {
    if let Some(root) = std::env::var_os("CODESCRIBE_DATA_DIR") {
        let root = std::path::PathBuf::from(root);
        if std::fs::read(root.join("rc-w2-lexicon-case"))
            .ok()
            .as_deref()
            == Some(name.as_bytes())
        {
            assert_eq!(
                crate::config::Config::config_dir(),
                root.canonicalize().unwrap()
            );
            assert_eq!(
                crate::quality::overlay_quality::apply_custom_lexicon("doker"),
                "Docker",
                "positive control: the configured rule must change the input",
            );
            check();
            return;
        }
    }
    let directory = tempfile::tempdir().expect("isolated lexicon directory");
    std::fs::write(directory.path().join("rc-w2-lexicon-case"), name)
        .expect("child fixture marker");
    std::fs::write(
        directory.path().join("lexicon.custom.jsonl"),
        "{\"term\":\"Docker\",\"mispronunciations\":[\"doker\"],\"source\":\"manual\"}\n",
    )
    .expect("deterministic custom rule");
    let output = std::process::Command::new(std::env::current_exe().expect("unit test binary"))
        .arg("--exact")
        .arg(format!(
            "{}::{name}",
            module_path!().split_once("::").unwrap().1
        ))
        .arg("--nocapture")
        .env("CODESCRIBE_DATA_DIR", directory.path())
        .output()
        .expect("isolated lexicon test process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "child test failed: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("running 1 test\n"),
        "exact test must exist: {stdout}"
    );
    assert!(
        stdout.contains("1 passed; 0 failed"),
        "no skipped execution: {stdout}"
    );
}

/// Positive configured rewrite through the real PhraseFinal consumer.
/// Light+ capitalization/periods belong to PresentationEmitter, so they
/// are deliberately absent here; raw Apple evidence remains unchanged.
#[test]
fn apple_seal_lexicon_corrects_sealed_final() {
    with_lexicon_fixture("apple_seal_lexicon_corrects_sealed_final", || {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("lexicon-phrase", 1.0);
        let occurrence = qualify(&mut state, 0.0, 1.0);
        emit(
            &mut state,
            &tx,
            vec![segment("uruchom doker teraz", 0.0, 1.0)],
        );
        let events = drain(&mut rx);
        assert_eq!(raw_finals(&events), vec!["uruchom doker teraz"]);
        assert_eq!(document(&state), "uruchom Docker teraz");
        assert_eq!(state.sealed_count, 1);
        let observations = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation, label, ..
                } => {
                    assert_eq!(observation.occurrence, occurrence);
                    Some((observation.producer, label.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observations,
            vec![
                (LedgerObservationProducer::Apple, "uruchom doker teraz"),
                (LedgerObservationProducer::Lexicon, "uruchom Docker teraz"),
            ]
        );
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::UtteranceFinal { text, .. } if text == "uruchom Docker teraz"
        )));
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
    });
}

#[test]
fn apple_seal_lexicon_leaves_preview_and_mirror_raw() {
    with_lexicon_fixture("apple_seal_lexicon_leaves_preview_and_mirror_raw", || {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("raw-lexicon-preview", 1.0);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.0, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        assert!(
            matches!(rx.try_recv().unwrap(), EngineEvent::Preview { text, .. } if text == "uruchom doker")
        );
        // The mirror carries the open partial word by word, still raw.
        assert!(matches!(rx.try_recv().unwrap(),
            EngineEvent::UnadmittedAppleWords { words, .. }
                if words.iter().map(|word| word.text.as_str()).eq(["uruchom", "doker"])
        ));
        assert!(rx.try_recv().is_err());
        assert!(document(&state).is_empty());
        assert_eq!(state.sealed_count, 0);
    });
}

/// The old `:D` string filter has no live worker owner. Blank observations
/// still cannot emit blank finals; a nonempty acoustically qualified label
/// must not disappear merely because its spelling resembles an artifact.
#[test]
fn apple_seal_lexicon_empty_text_never_emits_a_blank_final() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("blank-final", 1.0, &[(0.0, 1.0)]);
    emit(&mut state, &tx, vec![segment("   ", 0.0, 1.0)]);
    assert!(document(&state).is_empty());
    assert_eq!(state.sealed_count, 0);
    assert!(raw_finals(&drain(&mut rx)).is_empty());
    // Independent positive control: text classification must not substitute
    // for evidence. A blank callback has already reconciled its own slice.
    let mut state = physical_state("spoken-symbol", 1.0, &[(0.0, 1.0)]);
    emit(&mut state, &tx, vec![segment(":D", 0.0, 1.0)]);
    assert_eq!(document(&state), ":D");
    assert_eq!(raw_finals(&drain(&mut rx)), vec![":D"]);
    assert_eq!(state.sealed_count, 1);
}

/// Positive configured rewrite through the real summary fallback. A later
/// summary cannot bypass the occurrence owner to publish the document twice.
#[test]
fn apple_seal_lexicon_corrects_summary_fallback_seal() {
    with_lexicon_fixture("apple_seal_lexicon_corrects_summary_fallback_seal", || {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("summary-observers", 2.0);
        let occurrence = qualify(&mut state, 0.0, 2.0);
        let summary = LiveStreamEvent::Summary {
            text: "zbuduj obraz doker".into(),
            segments: vec![segment("zbuduj obraz doker", 0.0, 2.0)],
            ok: true,
            error: None,
        };
        emit_stream_events(vec![summary.clone()], &tx, &mut state, 2.0);
        let events = drain(&mut rx);
        let producers = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation, label, ..
                } => {
                    assert_eq!(observation.occurrence, occurrence);
                    Some((observation.producer, label.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            producers,
            vec![
                (LedgerObservationProducer::Apple, "zbuduj obraz doker"),
                (LedgerObservationProducer::Lexicon, "zbuduj obraz Docker"),
            ]
        );
        assert_eq!(document(&state), "zbuduj obraz Docker");
        assert_eq!(raw_finals(&events), vec!["zbuduj obraz doker"]);
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
        assert!(state.open_partial.is_empty());
        emit_stream_events(vec![summary], &tx, &mut state, 2.0);
        assert!(drain(&mut rx).is_empty());
        assert_eq!(state.sealed_count, 1);
    });
}

#[test]
fn fusion_lexicon_corrects_before_seal_and_refuses_late_replay() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut state = physical_state("apple-raw", 1.0, &[(0.0, 1.0)]);
    state.lexicon_custom_path = dir.path().join("lexicon.custom.jsonl");
    emit(
        &mut state,
        &tx,
        vec![segment("uruchom doker teraz", 0.0, 1.0)],
    );
    assert_eq!(document(&state), "uruchom Docker teraz");
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker teraz"]);
    let occurrence = OccurrenceIdentity::new("apple-raw", 7, 0, sample(1.0));
    let late = admit_ledger_label(
        &mut state,
        &tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Lexicon,
                99,
                0,
                occurrence,
            ),
            label: "uruchom Docker teraz",
            energy: EnergyAdmission::RequireExistingQualification,
        },
    )
    .unwrap();
    assert!(matches!(
        late,
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    assert_eq!(document(&state), "uruchom Docker teraz");
}

#[test]
fn apple_tail_patch_seal_enqueues_audio_window_for_the_sealed_utterance() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
    let mut state = physical_state("exact-tail", 6.0, &[(0.5, 2.0)]);
    state.tail_patch = Some(tail_tx);
    emit(&mut state, &tx, vec![segment("uruchom doker", 0.5, 2.0)]);
    assert!(
        raw_finals(&drain(&mut rx)).is_empty(),
        "Whisper still owns an open frontier"
    );
    state.capture_stopping = true;
    state.pump_capture_windows(&tx);
    let request = tail_rx.try_recv().expect("owned PCM request");
    assert!(
        tail_rx.try_recv().is_err(),
        "a 6 s head is one stopping partial"
    );
    assert_eq!(request.utterance_id, 1);
    assert_eq!(
        request.committed_text, "uruchom doker",
        "sliced lane preserves raw observed baseline"
    );
    assert_eq!(request.provider_request.identity.range.sample_start, 0);
    assert_eq!(
        request.provider_request.identity.range.sample_end,
        sample(6.0)
    );
    assert_eq!(request.audio, vec![0.25; sample(6.0) as usize]);
    assert_eq!(
        request.provider_request.identity.request_id,
        request.utterance_id
    );
    assert_eq!(
        request.member_occurrences,
        vec![(
            1,
            OccurrenceIdentity::new("exact-tail", 7, 2_624, sample(2.0))
        )]
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    state.complete_whisper_window(
        &tx,
        TailPatchCompletion {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity),
            member_occurrences: request.member_occurrences,
            payload: None,
        },
        6.0,
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.tail_patch_jobs_skipped, 1);
    // The stopping partial already finished the plan. Closing the finished
    // horizon releases the raw baseline; it does not invent a padded end.
    state.close_admission_horizon(&tx, state.window_plan.admission_horizon());
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker"]);
}

#[test]
fn seal_window_clamps_start_after_retention_eviction() {
    let state = state("evicted-head", 200.0);
    let retained_start = state.audio.retained_start_secs();
    assert_eq!(retained_start, 80.0);
    let retained_samples = (f64::from(retained_start) * f64::from(RATE)) as u64;
    let window = state
        .window_by_samples(retained_samples, sample(150.0))
        .expect("retained window");
    assert_eq!(window.sample_start, retained_samples);
    assert_eq!(window.sample_end, sample(150.0));
    let next = state
        .window_by_samples(sample(150.0), sample(180.0))
        .expect("next explicit slice");
    assert_eq!(next.sample_start, sample(150.0));
    assert_eq!(next.sample_end, sample(180.0));
    assert!(
        state
            .window_by_samples(sample(70.0), sample(75.0))
            .is_none(),
        "a slice wholly before retention is not a window"
    );
    assert!(
        state
            .window_by_samples(sample(79.0), sample(85.0))
            .is_none(),
        "an evicted prefix refuses the entire requested slice"
    );
    assert_eq!(
        state.unresolved_windows, 0,
        "reading the buffer does not itself count an unresolved offer"
    );
}

#[test]
fn apple_tail_patch_unresolved_window_enqueues_nothing() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
    let mut state = state("unresolved-tail", 2.0);
    state.tail_patch = Some(tail_tx);
    emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
    assert!(
        state.unsealed_word_owners().is_empty(),
        "the future phrase never became a qualified occurrence"
    );
    state.capture_stopping = true;
    state.pump_capture_windows(&tx);
    let offered = std::iter::from_fn(|| tail_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        offered
            .iter()
            .all(|request| request.member_occurrences.is_empty()),
        "the future phrase is not a member of any offer"
    );
    assert!(
        offered
            .iter()
            .all(|request| request.admit_sample_end <= sample(2.0)),
        "an offer stays inside the captured head"
    );
    for request in &offered {
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity.clone()),
                payload: None,
                member_occurrences: request.member_occurrences.clone(),
            },
            2.0,
        );
    }
    assert_eq!(state.unresolved_windows, 0);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.sealed_count, 0, "unqualified text cannot seal");
    assert!(document(&state).is_empty());
    assert!(raw_finals(&drain(&mut rx)).is_empty());
}

#[test]
fn retention_miss_counts_planned_window_once_without_queueing() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
    let mut state = state("retention-miss", 0.0);
    state.audio = LiveAudioBuffer::new(RATE, 1.0);
    state.audio.push(&vec![0.25; sample(9.0) as usize]);
    assert_eq!(state.audio.session_sample_end(), sample(9.0));
    assert_eq!(state.audio.retained_start_sample(), sample(8.0));
    assert!(state.window_by_samples(0, sample(9.0)).is_none());
    state.tail_patch = Some(tail_tx);
    state.capture_stopping = true;
    state.pump_capture_windows(&tx);
    assert_eq!(state.unresolved_windows, 1);
    assert_eq!(state.window_plan.admission_horizon(), u64::MAX);
    assert!(tail_rx.try_recv().is_err());
    state.pump_capture_windows(&tx);
    assert_eq!(state.unresolved_windows, 1);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.sealed_count, 0);
    assert!(document(&state).is_empty());
    assert!(raw_finals(&drain(&mut rx)).is_empty());
}

#[test]
fn apple_only_occurrence_seals_on_apple_return_with_whisper_armed_but_unscheduled() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (tail_tx, mut requests) = mpsc::channel(1);
    let mut state = state("armed-unscheduled", 2.0);
    state.tail_patch = Some(tail_tx);
    let owner = qualify(&mut state, 0.0, 1.0);
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .schedule_frontier(owner.clone(), [LedgerObservationProducer::Apple]);
    assert!(
        admit_ledger_label(
            &mut state,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    1,
                    0,
                    owner.clone(),
                ),
                label: "Iwo",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .is_some_and(|receipt| receipt.grants_mutation())
    );
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
    assert_eq!(ledger.conservation().residue(), 0);
    drop(ledger);
    assert_eq!(state.admission_horizon, 0);
    assert!(state.tail_patch.is_some());
    assert!(requests.try_recv().is_err());
    assert_eq!(
        drain(&mut rx)
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        1
    );
}

fn assert_sealed_apple_refusal(state: &AppleSealState, owner: &OccurrenceIdentity) {
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    let seal = ledger
        .seal_of(owner)
        .expect("genuinely sealed fixture")
        .clone();
    let slots = ledger.slots_of(owner).unwrap().to_vec();
    let label = ledger.text_of(owner).unwrap().to_string();
    let observation = ledger.next_word_observation(LedgerObservationProducer::Apple, 99, owner);
    assert!(matches!(
        ledger.admit(&observation, "forbidden sealed rewrite"),
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    assert_eq!(ledger.seal_of(owner), Some(&seal));
    assert_eq!(ledger.slots_of(owner).unwrap(), slots.as_slice());
    assert_eq!(ledger.text_of(owner), Some(label.as_str()));
    assert!(ledger.layer_trail_for(owner).any(|entry| {
        entry.observation == observation
            && matches!(
                entry.decision,
                MutationReceipt::Refuse {
                    reason: RefuseReason::SealedReplay,
                    ..
                }
            )
    }));
}

#[test]
fn changed_apple_span_cannot_append_a_cumulative_suffix() {
    for armed in [false, true] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, _requests) = mpsc::channel(1);
        let mut state = physical_state("stale-apple-slot", 2.0, &[(0.0, 2.0)]);
        if armed {
            state.tail_patch = Some(tail_tx);
        }
        let owner = OccurrenceIdentity::new("stale-apple-slot", 7, 0, sample(2.0));
        emit(&mut state, &tx, vec![segment("alpha beta", 0.0, 2.0)]);
        // Returning the observer and closing the horizon, rather than merely
        // closing Silero, establishes finality in both configurations.
        state.return_outstanding_whisper_without_label(&tx);
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
        let seal = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_of(&owner)
            .unwrap()
            .clone();
        let before = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&owner)
            .unwrap()
            .to_vec();
        drain(&mut rx);
        emit(
            &mut state,
            &tx,
            vec![segment("alpha beta revised", 0.0, 2.0)],
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("alpha beta"));
        assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
        assert_eq!(ledger.conservation().residue(), 0);
        assert_eq!(ledger.seal_of(&owner), Some(&seal));
        drop(ledger);
        assert!(drain(&mut rx).iter().any(|event| match event {
            EngineEvent::LedgerMutation { label, receipt, .. } if label == "alpha beta revised" => {
                matches!(
                    receipt,
                    MutationReceipt::KeepVisibleUnanchored {
                        reason: NoAuthorityReason::LateAppleWordSealedOwner,
                        ..
                    }
                )
            }
            _ => false,
        }));
        assert_sealed_apple_refusal(&state, &owner);
    }
}

#[test]
fn cumulative_apple_restatement_is_a_replay_not_a_late_slot() {
    for armed in [false, true] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, _requests) = mpsc::channel(1);
        let mut state = physical_state("restatement-slot", 2.0, &[(0.0, 2.0)]);
        if armed {
            state.tail_patch = Some(tail_tx);
        }
        let owner = OccurrenceIdentity::new("restatement-slot", 7, 0, sample(2.0));
        let mut words = vec![segment("alpha", 0.0, 0.5), segment("beta", 1.0, 1.5)];
        emit(&mut state, &tx, words.clone());
        let before = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&owner)
            .unwrap()
            .to_vec();
        drain(&mut rx);
        let receipts_before = state.acoustic_ledger.lock().unwrap().layer_trail().len();
        emit(&mut state, &tx, words.clone());
        assert!(drain(&mut rx).is_empty());
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.layer_trail().len(), receipts_before);
        assert_eq!(ledger.text_of(&owner), Some("alpha beta"));
        assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
        assert_eq!(
            ledger
                .layer_trail_for(&owner)
                .filter(|entry| matches!(
                    entry.decision,
                    MutationReceipt::Refuse {
                        reason: RefuseReason::ReplayedRangeIdentity,
                        ..
                    }
                ))
                .count(),
            0
        );
        assert_eq!(ledger.conservation().residue(), 0);
        assert_eq!(ledger.is_sealed(&owner), !armed);
        drop(ledger);

        // A new callback crosses the callback fence. Its consumed ranges
        // receive replay receipts; only the novel range may add a slot.
        words.push(segment("gamma", 1.6, 1.9));
        emit(&mut state, &tx, words);
        let events = drain(&mut rx);
        let ledger = state.acoustic_ledger.lock().unwrap();
        if armed {
            for label in ["alpha", "beta"] {
                assert_eq!(
                    ledger
                        .layer_trail_for(&owner)
                        .filter(|entry| {
                            entry.candidate_label == label
                                && matches!(
                                    entry.decision,
                                    MutationReceipt::Refuse {
                                        reason: RefuseReason::ReplayedRangeIdentity,
                                        ..
                                    }
                                )
                        })
                        .count(),
                    1
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(
                            event,
                            EngineEvent::LedgerMutation {
                                observation,
                                label: emitted,
                                receipt: MutationReceipt::Refuse {
                                    reason: RefuseReason::ReplayedRangeIdentity,
                                    ..
                                },
                            } if observation.occurrence == owner && emitted == label
                        ))
                        .count(),
                    1
                );
            }
        } else {
            let kept = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::LedgerMutation {
                        observation,
                        label,
                        receipt:
                            MutationReceipt::KeepVisibleUnanchored {
                                reason: NoAuthorityReason::LateAppleWordSealedOwner,
                                ..
                            },
                    } if observation.occurrence == owner => Some(label.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(kept, ["alpha", "alpha beta", "alpha beta gamma"]);
        }
        let slots = ledger.slots_of(&owner).unwrap();
        assert_eq!(&slots[..before.len()], before.as_slice());
        if armed {
            assert_eq!(slots.len(), before.len() + 1);
            assert_eq!(slots.last().unwrap().text, "gamma");
            assert_eq!(ledger.text_of(&owner), Some("alpha beta gamma"));
            assert!(events.iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation { label, receipt, .. }
                    if label == "alpha beta gamma" && receipt.grants_mutation()
            )));
        } else {
            assert_eq!(slots, before.as_slice());
            assert_eq!(ledger.text_of(&owner), Some("alpha beta"));
            assert!(events.iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation {
                    label,
                    receipt: MutationReceipt::KeepVisibleUnanchored {
                        reason: NoAuthorityReason::LateAppleWordSealedOwner,
                        ..
                    },
                    ..
                } if label == "alpha beta gamma"
            )));
        }
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
        assert_eq!(ledger.is_sealed(&owner), !armed);
        assert!(state.unmatched_silero_words.is_empty());
    }
}

#[test]
fn apple_tail_patch_off_by_default_enqueues_no_jobs() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("tail-off", 4.0, &[(0.5, 2.0)]);
    assert!(
        state.tail_patch.is_none(),
        "arming is injected by session owner"
    );
    emit(&mut state, &tx, vec![segment("uruchom doker", 0.5, 2.0)]);
    state.pump_capture_windows(&tx);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.windows_admitted, 0);
    assert!(state.refinement_pending.is_empty());
    assert_eq!(state.sealed_count, 1);
    assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker"]);
}

#[test]
fn apple_tail_patch_backpressure_retains_until_capacity_without_stalling_capture() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (tail_tx, mut tail_rx) = mpsc::channel(1);
    let ranges = (0..10)
        .map(|i| (i as f32 * 0.5, i as f32 * 0.5 + 0.4))
        .collect::<Vec<_>>();
    let mut state = physical_state("full-tail", 10.0, &ranges);
    state.tail_patch = Some(tail_tx);
    for (i, &(start, end)) in ranges.iter().enumerate() {
        emit(
            &mut state,
            &tx,
            vec![segment(&format!("segment {i}"), start, end)],
        );
    }
    // 10 s covers one 9 s window. The next step needs a 12 s head, so the
    // channel of 1 can retain that exact second offer.
    state.audio.push(&vec![0.25; 32_000]);
    state.pump_capture_windows(&tx);
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    assert_eq!(state.refinement_pending.len(), 1);
    let held = state
        .refinement_pending
        .front()
        .unwrap()
        .provider_request
        .identity
        .clone();
    assert_eq!(
        (held.range.sample_start, held.range.sample_end),
        (48_000, 192_000)
    );
    assert_eq!(
        state.sealed_count, 0,
        "a full queue retains the offer and does not recover speech"
    );
    state.audio.push(&[0.25; 512]);
    state.pump_capture_windows(&tx);
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(
        state
            .refinement_pending
            .front()
            .unwrap()
            .provider_request
            .identity,
        held
    );
    let first = tail_rx.try_recv().expect("the accepted window");
    let mut submitted = BTreeSet::new();
    assert!(submitted.insert(first.submission_sequence));
    first.provider_request.validate_pcm(&first.audio).unwrap();
    state.complete_whisper_window(
        &tx,
        TailPatchCompletion {
            submission_sequence: first.submission_sequence,
            utterance_id: first.utterance_id,
            request_identity: Some(first.provider_request.identity),
            member_occurrences: first.member_occurrences,
            payload: None,
        },
        12.0,
    );
    state.pump_capture_windows(&tx);
    let second = tail_rx.try_recv().expect("the retained offer");
    assert!(submitted.insert(second.submission_sequence));
    assert_eq!(second.provider_request.identity, held);
    assert!(tail_rx.try_recv().is_err());
    assert_eq!(submitted.len(), 2);
    assert!(state.refinement_pending.is_empty());
    {
        let ledger = state.acoustic_ledger.lock().unwrap();
        for (i, &(start, end)) in ranges.iter().enumerate() {
            // Onset context is clamped to the previous closed owner.
            let owner_start = if i == 0 {
                sample(start)
            } else {
                sample(ranges[i - 1].1)
            };
            let occurrence = OccurrenceIdentity::new("full-tail", 7, owner_start, sample(end));
            assert_eq!(
                ledger.text_of(&occurrence),
                Some(format!("segment {i}").as_str()),
                "all physical labels remain visible while the queue is saturated"
            );
        }
    }
    let before = state.audio.session_sample_end();
    state.audio.push(&[0.25; 512]);
    assert_eq!(state.audio.session_sample_end(), before + 512);
}

#[test]
fn fusion_slices_admit_disjoint_ledger_occurrences_before_raw_finals() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("slice-order", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
    emit(
        &mut state,
        &tx,
        vec![segment("Iwo", 0.0, 1.0), segment("Iwo", 2.0, 3.0)],
    );
    let events = drain(&mut rx);
    assert_eq!(raw_finals(&events), vec!["Iwo", "Iwo"]);
    for (id, start, end) in [(1, 0, 16_000), (2, 26_624, 48_000)] {
        let occurrence = OccurrenceIdentity::new("slice-order", 7, start, end);
        let admission = events.iter().position(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, receipt, label }
                    if observation.producer == LedgerObservationProducer::Apple
                        && observation.occurrence == occurrence && receipt.grants_mutation() && label == "Iwo"
            )).expect("accepted slice-local Apple observation");
        let seal = events
            .iter()
            .position(|event| {
                matches!(event,
                    EngineEvent::LedgerSeal { receipt } if receipt.coverage == occurrence
                )
            })
            .expect("same occurrence seal");
        let final_index = events
            .iter()
            .position(|event| {
                matches!(event,
                    EngineEvent::UtteranceFinal { utterance_id, .. } if *utterance_id == id
                )
            })
            .expect("raw final");
        assert!(admission < seal && seal < final_index);
    }
    assert_eq!(document(&state), "Iwo Iwo");
}

/// Sliced replay is stopped by reconciled physical identity before a new
/// admission. Also exercise the ledger's independent replay fence directly.
#[test]
fn fusion_slice_replay_reaches_ledger_identity_refusal() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("slice-replay", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
    let words = vec![segment("Iwo", 0.0, 1.0), segment("Iwo", 2.0, 3.0)];
    emit(&mut state, &tx, words.clone());
    drain(&mut rx);
    emit(&mut state, &tx, words);
    assert!(
        drain(&mut rx).is_empty(),
        "reconciled slice must not re-emit events"
    );
    for (id, start, end) in [(1, 0, 16_000), (2, 26_624, 48_000)] {
        let receipt = admit_ledger_label(
            &mut state,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    id,
                    0,
                    OccurrenceIdentity::new("slice-replay", 7, start, end),
                ),
                label: "changed replay",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .unwrap();
        assert!(!receipt.grants_mutation());
    }
    assert_eq!(document(&state), "Iwo Iwo");
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        2
    );
}

#[test]
fn five_disjoint_iwo_segments_all_reach_the_final() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("five-words", 3.0, &[(0.0, 2.0)]);
    emit(
        &mut state,
        &tx,
        (0..5)
            .map(|i| {
                let start = i as f32 * 0.4;
                segment("Iwo", start, start + 0.3)
            })
            .collect(),
    );
    let events = drain(&mut rx);
    let finals = raw_finals(&events);
    assert_eq!(
        finals.len(),
        1,
        "one physical occurrence contains five words"
    );
    assert_eq!(count_iwo(finals[0]), 5);
    assert_eq!(count_iwo(&document(&state)), 5);
}

#[test]
fn cumulative_fifth_iwo_is_not_absorbed_as_a_revision() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = physical_state("fifth-iwo", 3.0, &[(0.0, 1.5), (1.6, 1.9)]);
    let first = (0..4)
        .map(|i| {
            let start = i as f32 * 0.4;
            segment("Iwo", start, start + 0.3)
        })
        .collect::<Vec<_>>();
    emit(&mut state, &tx, first.clone());
    assert_eq!(count_iwo(&document(&state)), 4);
    let mut cumulative = first;
    cumulative.push(segment("Iwo", 1.6, 1.9));
    emit(&mut state, &tx, cumulative);
    assert_eq!(count_iwo(&document(&state)), 5);
    assert_eq!(
        raw_finals(&drain(&mut rx))
            .iter()
            .map(|text| count_iwo(text))
            .sum::<usize>(),
        5
    );
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().occurrences().count(),
        2
    );
}

fn cloud_notice_final(
    text: &str,
    words: &[(&str, u64, u64)],
    sample_start: u64,
    sample_end: u64,
) -> crate::asr_session::events::TranscriptEvent {
    use crate::asr_session::events::{
        CaptureWord, CommitMatchPath, FinalCommit, FinalGrain, SessionId, TranscriptEvent,
    };
    TranscriptEvent {
        session_id: SessionId::new("cloud-final").expect("session id"),
        utterance_id: 1,
        sequence_id: 1,
        text: text.to_string(),
        range: None,
        commit: Some(FinalCommit {
            commit_id: "commit-1".to_string(),
            sample_start,
            sample_end,
            capture_rate_hz: RATE,
            grain: if words.is_empty() {
                FinalGrain::Phrase
            } else {
                FinalGrain::Word
            },
            words: words
                .iter()
                .map(|(word, start, end)| CaptureWord {
                    word: (*word).to_string(),
                    sample_start: *start,
                    sample_end: *end,
                    probability: None,
                })
                .collect(),
            word_time_clamped: 0,
            phrase_fallback: false,
            match_path: CommitMatchPath::Echo,
            range_mismatch: None,
            clock_unreliable: false,
        }),
    }
}

fn silence_close(secs: f32) -> super::super::silero_fusion::UtteranceCloseReceipt {
    super::super::silero_fusion::UtteranceCloseReceipt {
        decision_sample: sample(secs),
        cause: super::super::silero_fusion::UtteranceCloseCause::SilenceFence,
    }
}

fn cloud_owner(
    state: &mut AppleSealState,
    tx: &mpsc::UnboundedSender<EngineEvent>,
    id: u64,
    start: f32,
    end: f32,
) -> OccurrenceIdentity {
    let owner = qualify(state, start, end);
    for producer in [
        LedgerObservationProducer::Apple,
        LedgerObservationProducer::Lexicon,
    ] {
        admit_ledger_label(
            state,
            tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(producer, id, 0, owner.clone()),
                label: "zostaje",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .expect("qualified fixture label");
    }
    owner
}

fn cloud_open(state: &AppleSealState, owner: &OccurrenceIdentity) -> bool {
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .frontier_of(owner)
        .is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::CloudLive)
        })
}

#[test]
fn live_silero_receipt_drives_the_hub_commit_without_changing_owned_pcm() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, mut commit_rx) = mpsc::channel(4);
    let mut state = state("live-cloud-receipt", 0.0);
    state.cloud_commit_tx = Some(commit_tx);
    state.fusion_seal_armed = true;
    state.fusion = Some(SileroIngress::new(
        RATE,
        &state.session_id,
        state.capture_epoch,
    ));
    let mut cursor = 0;
    let mut closed = None;
    for frame in 0..100 {
        let speech = frame < 20;
        let pcm = vec![if speech { 0.25 } else { 0.0 }; 512];
        state.audio.push(&pcm);
        cursor += pcm.len() as u64;
        let fusion = state.fusion.as_mut().unwrap();
        fusion.push_scripted_speech_prob_for_test(if speech { 0.9 } else { 0.01 });
        let ingest = fusion.ingest(&pcm, &pcm, cursor);
        let receipt = ingest
            .closed
            .first()
            .and_then(|id| fusion.ledger().close_receipt(*id));
        seal_sliced_by_silero(&mut state, &tx, &[]);
        if let Some(receipt) = receipt {
            closed = Some(receipt);
            break;
        }
    }
    let receipt = closed.expect("scripted VAD must reach a silence fence");
    assert_eq!(commit_rx.try_recv().unwrap(), receipt.decision_sample);
    assert_eq!(state.cloud_inflight.len(), 1);
    assert_eq!(state.cloud_inflight[0].occurrences.len(), 1);
    let owner = state.cloud_inflight[0].occurrences[0].clone();
    assert_eq!(owner.sample_end, receipt.decision_sample);
    assert!(cloud_open(&state, &owner));
    let before = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .qualified_occurrences()
        .cloned()
        .collect::<Vec<_>>();
    state.commit_cloud_close(&tx, receipt);
    assert!(commit_rx.try_recv().is_err());
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualified_occurrences()
            .cloned()
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn phrase_final_never_requeues_its_returned_observer() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("phrase-return", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let owner = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let mut final_event = cloud_notice_final("zostaje", &[], 0, sample(1.5));
    final_event.commit.as_mut().unwrap().phrase_fallback = true;
    state.admit_cloud_final(&tx, final_event);
    assert!(!cloud_open(&state, &owner));
    assert!(state.cloud_uncommitted.is_empty());
    state.commit_cloud_close(&tx, silence_close(2.0));
    assert!(state.cloud_inflight[0].occurrences.is_empty());
    state.release_cloud_live_behind(&tx, sample(5.0));
    assert_eq!(state.cloud_live_timed_out, 0);
}

#[test]
fn lane_loss_returns_tracked_and_inflight_and_disarms_future_scheduling() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("cloud-loss-all", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let open = cloud_owner(&mut state, &tx, 2, 2.0, 3.0);
    state.handle_cloud_notice(&tx, super::CloudWorkerNotice::LaneLost);
    assert_eq!(state.cloud_live_lane_lost, 2);
    assert!(state.cloud_commit_tx.is_none());
    assert!(state.cloud_uncommitted.is_empty());
    assert!(state.cloud_inflight.is_empty());
    assert!(!cloud_open(&state, &first));
    assert!(!cloud_open(&state, &open));
    let next = cloud_owner(&mut state, &tx, 3, 3.0, 4.0);
    assert!(!cloud_open(&state, &next));
    state.handle_cloud_notice(&tx, super::CloudWorkerNotice::LaneLost);
    assert_eq!(state.cloud_live_lane_lost, 2);
}

#[test]
fn split_waits_for_silence_then_commits_both_occurrences_at_decision_sample() {
    use super::super::silero_fusion::{UtteranceCloseCause, UtteranceCloseReceipt};
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, mut commit_rx) = mpsc::channel(4);
    let mut state = state("split-commit", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(
        &tx,
        UtteranceCloseReceipt {
            decision_sample: sample(1.0),
            cause: UtteranceCloseCause::MaxUtteranceSplit,
        },
    );
    assert!(commit_rx.try_recv().is_err());
    assert!(state.cloud_inflight.is_empty());
    let second = cloud_owner(&mut state, &tx, 2, 1.0, 2.0);
    state.commit_cloud_close(&tx, silence_close(2.6));
    assert_eq!(commit_rx.try_recv().unwrap(), sample(2.6));
    assert_eq!(
        state.cloud_inflight[0].occurrences,
        vec![first.clone(), second.clone()]
    );
    assert!(state.cloud_uncommitted.is_empty());
    state.commit_cloud_close(&tx, silence_close(2.6));
    assert!(commit_rx.try_recv().is_err());
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(2.6)));
    assert!(!cloud_open(&state, &first));
    assert!(!cloud_open(&state, &second));
}

#[test]
fn just_closed_owner_past_the_decision_is_released_by_its_own_final() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("callback-past-decision", 3.0);
    state.cloud_commit_tx = Some(commit_tx);
    // S is 1.5 s; close_open at the callback head owns PCM through 1.6 s.
    let mut boundaries = super::super::silero_fusion::UtteranceLedger::new();
    boundaries.open_or_extend(&state.session_id, state.capture_epoch, 0, sample(1.0));
    boundaries.close_open(sample(1.6));
    assert!(reconcile_silero_ledger(&mut state, &tx, &boundaries, &[]));
    let owner = OccurrenceIdentity::new(&state.session_id, 7, 0, sample(1.6));
    assert!(state.cloud_uncommitted.contains(&owner));
    state.commit_cloud_close(&tx, silence_close(1.5));
    assert_eq!(state.cloud_inflight[0].occurrences, vec![owner.clone()]);
    assert!(!state.cloud_uncommitted.contains(&owner));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.5)));
    assert!(!cloud_open(&state, &owner));
    assert!(state.cloud_inflight.is_empty());
}

#[test]
fn open_padded_owner_before_s_waits_for_its_own_close_and_final() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("padded-open-owner", 3.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.6);
    // B's pre-pad begins before S=1.5, but its physical extent is open.
    let second = OccurrenceIdentity::new(&state.session_id, 7, sample(1.4), sample(2.4));
    let calibration = state.energy_calibration.as_ref().unwrap();
    let mut evidence = AcousticEvidence {
        occurrence: second.clone(),
        duration_ms: 1_000.0,
        energy_integral: second.sample_len() as f64 * 0.25_f64.powi(2),
        mean_rms_dbfs: 20.0 * 0.25_f64.log10(),
        peak_dbfs: 20.0 * 0.25_f64.log10(),
        vad_open_sample: Some(second.sample_start),
        vad_close_sample: None,
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualify(&evidence, calibration)
            .is_qualified()
    );
    state.track_cloud_occurrence(&second);
    state.commit_cloud_close(&tx, silence_close(1.5));
    assert_eq!(state.cloud_inflight[0].occurrences, vec![first.clone()]);
    assert!(state.cloud_uncommitted.contains(&second));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.5)));
    assert!(!cloud_open(&state, &first));
    assert!(cloud_open(&state, &second));
    // A later physical close qualifies the same identity, then its own
    // silence commit takes the observer. Text never decides membership.
    evidence.vad_close_sample = Some(second.sample_end);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualify(&evidence, state.energy_calibration.as_ref().unwrap())
            .is_qualified()
    );
    state.commit_cloud_close(&tx, silence_close(2.5));
    assert_eq!(state.cloud_inflight[0].occurrences, vec![second.clone()]);
    assert!(state.cloud_uncommitted.is_empty());
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], sample(1.5), sample(2.5)));
    assert!(!cloud_open(&state, &second));
}

#[test]
fn reminted_utterance_id_tracks_the_new_occurrence_until_its_next_final() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("remint", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 7, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.5)));
    let reminted = cloud_owner(&mut state, &tx, 7, 2.0, 3.0);
    assert!(state.cloud_uncommitted.contains(&reminted));
    state.commit_cloud_close(&tx, silence_close(3.5));
    assert_eq!(state.cloud_inflight[0].occurrences, vec![reminted.clone()]);
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], sample(1.5), sample(3.5)));
    assert!(!cloud_open(&state, &first));
    assert!(!cloud_open(&state, &reminted));
    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&reminted));
}

#[test]
fn session_end_returns_committed_and_open_uncommitted_observers_once() {
    use super::super::silero_fusion::{UtteranceCloseCause, UtteranceCloseReceipt};
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (commit_tx, mut commit_rx) = mpsc::channel(4);
    let mut state = state("cloud-stop-all", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let open = cloud_owner(&mut state, &tx, 2, 2.0, 4.0);
    state.commit_cloud_close(
        &tx,
        UtteranceCloseReceipt {
            decision_sample: sample(4.0),
            cause: UtteranceCloseCause::EndOfCapture,
        },
    );
    assert_eq!(commit_rx.try_recv().unwrap(), sample(1.5));
    assert!(
        commit_rx.try_recv().is_err(),
        "stop uses the existing lane end"
    );
    drain(&mut rx);
    state.return_outstanding_cloud(&tx);
    assert!(state.cloud_inflight.is_empty());
    assert!(state.cloud_uncommitted.is_empty());
    for owner in [&first, &open] {
        assert!(!cloud_open(&state, owner));
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(owner));
    }
    let seals = drain(&mut rx)
        .iter()
        .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
        .count();
    assert_eq!(seals, 2);
    state.return_outstanding_cloud(&tx);
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn fluent_next_utterance_does_not_expire_a_final_inside_capture_grace() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("fluent-cloud", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let next = cloud_owner(&mut state, &tx, 2, 1.7, 2.2);
    state.close_admission_horizon(&tx, sample(1.7));
    state.release_cloud_live_behind(&tx, sample(2.7));
    assert!(cloud_open(&state, &first));
    assert!(!state.acoustic_ledger.lock().unwrap().is_sealed(&first));
    state.admit_cloud_final(
        &tx,
        cloud_notice_final(
            "cloud",
            &[("cloud", sample(0.2), sample(0.6))],
            0,
            sample(1.5),
        ),
    );
    assert_eq!(state.cloud_live_refused_sealed, 0);
    assert_eq!(state.cloud_live_admitted, 1);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&first)
            .unwrap()
            .iter()
            .any(|slot| slot.producer == LedgerObservationProducer::CloudLive)
    );
    assert!(cloud_open(&state, &next));
}

#[test]
fn reverse_finals_release_only_their_matching_commit_and_unmatched_text_is_routed() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("reverse-cloud", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let second = cloud_owner(&mut state, &tx, 2, 2.0, 3.0);
    state.commit_cloud_close(&tx, silence_close(3.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], sample(1.5), sample(3.5)));
    assert!(cloud_open(&state, &first));
    assert!(!cloud_open(&state, &second));
    assert_eq!(state.cloud_inflight[0].sample_end, sample(1.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.5)));
    assert!(!cloud_open(&state, &first));
    assert!(state.cloud_inflight.is_empty());
    state.admit_cloud_final(
        &tx,
        cloud_notice_final(
            "kept",
            &[("kept", sample(0.2), sample(0.6))],
            0,
            sample(1.5),
        ),
    );
    assert_eq!(state.cloud_live_unmatched_final, 1);
    assert!(state.acoustic_ledger.lock().unwrap().layer_trail().iter().any(|entry| {
            matches!(&entry.decision, MutationReceipt::KeepVisibleUnanchored { label, .. } if label == "kept")
        }));
}

#[test]
fn late_commit_end_selects_the_first_eligible_range() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("late-commit-end", 4.0);
    state.cloud_commit_tx = Some(commit_tx);
    let first = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    let second = cloud_owner(&mut state, &tx, 2, 2.0, 3.0);
    state.commit_cloud_close(&tx, silence_close(3.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.8)));
    assert!(!cloud_open(&state, &first));
    assert!(cloud_open(&state, &second));
}

#[test]
fn cloud_return_waits_for_whisper_on_the_same_frontier() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("cloud-and-whisper", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let owner = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .schedule_observer(owner.clone(), LedgerObservationProducer::Whisper);
    state.commit_cloud_close(&tx, silence_close(1.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], 0, sample(1.5)));
    assert!(!cloud_open(&state, &owner));
    assert!(!state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
    state.finish_whisper_frontier(&tx, &owner);
    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
}

#[test]
fn split_only_monologue_records_the_longest_uncommitted_capture_span() {
    use super::super::silero_fusion::{UtteranceCloseCause, UtteranceCloseReceipt};
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, mut commit_rx) = mpsc::channel(4);
    let mut state = state("long-monologue", 40.0);
    state.cloud_commit_tx = Some(commit_tx);
    for second in [12.0, 24.0, 36.0, 40.0] {
        state.observe_cloud_capture_head(sample(second));
        state.commit_cloud_close(
            &tx,
            UtteranceCloseReceipt {
                decision_sample: sample(second),
                cause: UtteranceCloseCause::MaxUtteranceSplit,
            },
        );
    }
    assert!(commit_rx.try_recv().is_err());
    assert_eq!(state.cloud_uncommitted_span_ms, 40_000);
    state.commit_cloud_close(&tx, silence_close(40.0));
    state.observe_cloud_capture_head(sample(42.0));
    assert_eq!(state.cloud_uncommitted_span_ms, 40_000);
}

#[test]
fn cloud_commit_does_not_mint_or_resize_an_occurrence() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("commit-identity", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let occurrence = qualify(&mut state, 0.0, 1.0);
    let before = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .qualified_occurrences()
        .cloned()
        .collect::<Vec<_>>();
    state.track_cloud_occurrence(&occurrence);
    state.commit_cloud_close(&tx, silence_close(1.6));
    let after = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .qualified_occurrences()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(before, after);
    assert_eq!(after[0].sample_start, occurrence.sample_start);
    assert_eq!(after[0].sample_end, occurrence.sample_end);
    assert_eq!(state.cloud_inflight.len(), 1);
    assert_eq!(state.cloud_inflight[0].sample_end, sample(1.6));
}

#[test]
fn local_power_without_a_cloud_lane_leaves_the_ledger_unchanged() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut state = state("local-power", 2.0);
    let occurrence = qualify(&mut state, 0.0, 1.0);
    let observation =
        LedgerObservationIdentity::new(LedgerObservationProducer::Apple, 1, 0, occurrence.clone());
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .admit(&observation, "zostaje");
    let before = state.acoustic_ledger.lock().unwrap().layer_trail().len();
    let text_before = document(&state);
    state.track_cloud_occurrence(&occurrence);
    state.commit_cloud_close(&tx, silence_close(1.6));
    assert!(state.cloud_inflight.is_empty());
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().layer_trail().len(),
        before
    );
    assert_eq!(document(&state), text_before);
    assert_eq!(state.cloud_live_admitted, 0);
    assert_eq!(state.cloud_live_timed_out, 0);
}

#[test]
fn empty_cloud_final_returns_the_scheduled_observer() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let mut state = state("empty-final", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let occurrence = qualify(&mut state, 0.0, 1.0);
    state.track_cloud_occurrence(&occurrence);
    state.commit_cloud_close(&tx, silence_close(1.5));
    state.admit_cloud_final(&tx, cloud_notice_final("", &[], sample(0.0), sample(1.5)));
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        !ledger
            .frontier_of(&occurrence)
            .unwrap()
            .open_producers()
            .contains(&LedgerObservationProducer::CloudLive)
    );
}

#[test]
fn cloud_lane_loss_lets_the_covered_occurrence_seal() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut state = state("lane-lost", 2.0);
    let occurrence = qualify(&mut state, 0.0, 1.0);
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        ledger.schedule_frontier(
            occurrence.clone(),
            [
                LedgerObservationProducer::Apple,
                LedgerObservationProducer::CloudLive,
            ],
        );
        assert!(
            ledger
                .admit(
                    &LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        1,
                        0,
                        occurrence.clone(),
                    ),
                    "zostaje",
                )
                .is_insert()
        );
        ledger.note_frontier_return(&occurrence, LedgerObservationProducer::Apple);
    }
    state.cloud_inflight.push_back(super::PendingCloudCommit {
        sample_end: sample(1.5),
        occurrences: vec![occurrence.clone()],
    });
    state.handle_cloud_notice(&tx, super::CloudWorkerNotice::LaneLost);
    assert_eq!(state.cloud_live_lane_lost, 1);
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(ledger.is_sealed(&occurrence));
    assert_eq!(ledger.text_of(&occurrence), Some("zostaje"));
}

#[test]
fn stalled_cloud_final_seals_only_after_capture_clock_grace() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut state = state("cloud-stall", 2.0);
    let occurrence = qualify(&mut state, 0.0, 1.0);
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        ledger.schedule_frontier(occurrence.clone(), [LedgerObservationProducer::CloudLive]);
        assert!(
            ledger
                .admit(
                    &LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        1,
                        0,
                        occurrence.clone(),
                    ),
                    "zostaje",
                )
                .is_insert()
        );
    }
    let commit_end = sample(1.5);
    state.cloud_commit_tx = Some(mpsc::channel(1).0);
    state.cloud_inflight.push_back(super::PendingCloudCommit {
        sample_end: commit_end,
        occurrences: vec![occurrence.clone()],
    });
    state.close_admission_horizon(&tx, commit_end);
    assert!(!state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
    state.close_admission_horizon(&tx, commit_end + sample(0.2));
    assert_eq!(state.cloud_live_timed_out, 0);
    state.release_cloud_live_behind(&tx, commit_end + sample(3.0) - 1);
    assert_eq!(state.cloud_live_timed_out, 0);
    state.release_cloud_live_behind(&tx, commit_end + sample(3.0));
    assert_eq!(state.cloud_live_timed_out, 1);
    {
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_sealed(&occurrence));
    }
    state.admit_cloud_final(
        &tx,
        cloud_notice_final(
            "pozno",
            &[("pozno", sample(0.2), sample(0.5))],
            0,
            commit_end,
        ),
    );
    assert_eq!(state.cloud_live_refused_sealed, 1);
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&occurrence), Some("zostaje"));
    assert!(ledger.layer_trail().iter().any(|entry| {
        matches!(
            entry.decision,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::LateCloudLiveWordSealedOwner,
                ref label,
                ..
            } if label.contains("pozno")
        )
    }));
}

#[test]
fn cloud_word_final_keeps_two_slots_on_one_owner() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut state = state("two-slots", 2.0);
    let occurrence = qualify(&mut state, 0.0, 2.0);
    state.cloud_inflight.push_back(super::PendingCloudCommit {
        sample_end: sample(2.0),
        occurrences: vec![occurrence.clone()],
    });
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .schedule_frontier(occurrence.clone(), [LedgerObservationProducer::CloudLive]);
    state.admit_cloud_final(
        &tx,
        cloud_notice_final(
            "dwa slowa",
            &[
                ("dwa", sample(0.2), sample(0.6)),
                ("slowa", sample(1.0), sample(1.4)),
            ],
            0,
            sample(2.0),
        ),
    );
    let slots = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .slots_of(&occurrence)
        .unwrap()
        .to_vec();
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0].text, "dwa");
    assert_eq!(slots[1].text, "slowa");
    assert!(
        slots
            .iter()
            .all(|slot| slot.producer == LedgerObservationProducer::CloudLive)
    );
}

#[test]
fn cloud_word_outside_every_owner_uses_the_whisper_gap_path() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut cloud = state("cloud-gap", 1.0);
    let mut whisper = state("whisper-gap", 1.0);
    let _ = qualify(&mut cloud, 0.0, 0.4);
    let _ = qualify(&mut whisper, 0.0, 0.4);
    let pin = TimedTailSegment {
        confidence: None,
        text: "obok".into(),
        range: TailSampleRange {
            session: cloud.session_id.clone(),
            capture_epoch: cloud.capture_epoch,
            sample_start: sample(0.6),
            sample_end: sample(0.8),
        },
        grain: crate::stt::tail_provider::TailSegmentGrain::Word,
    };
    let owners = cloud.word_owners_overlapping(0, u64::MAX);
    cloud.route_overlap_pins(
        &tx,
        AdmitWindow {
            request_id: 1,
            sample_start: 0,
            sample_end: sample(1.0),
        },
        &owners,
        std::slice::from_ref(&pin),
        LedgerObservationProducer::CloudLive,
    );
    let whisper_pin = TimedTailSegment {
        confidence: None,
        range: TailSampleRange {
            session: whisper.session_id.clone(),
            ..pin.range.clone()
        },
        ..pin
    };
    whisper.route_overlap_pins(
        &tx,
        AdmitWindow {
            request_id: 1,
            sample_start: 0,
            sample_end: sample(1.0),
        },
        &whisper.word_owners_overlapping(0, u64::MAX),
        &[whisper_pin],
        LedgerObservationProducer::Whisper,
    );
    let cloud_reason = cloud
        .acoustic_ledger
        .lock()
        .unwrap()
        .layer_trail()
        .iter()
        .rev()
        .find_map(|entry| match &entry.decision {
            MutationReceipt::KeepVisibleUnanchored { reason, .. } => Some(*reason),
            _ => None,
        });
    let whisper_reason = whisper
        .acoustic_ledger
        .lock()
        .unwrap()
        .layer_trail()
        .iter()
        .rev()
        .find_map(|entry| match &entry.decision {
            MutationReceipt::KeepVisibleUnanchored { reason, .. } => Some(*reason),
            _ => None,
        });
    assert_eq!(cloud_reason, whisper_reason);
    assert!(cloud_reason.is_some());
    assert_eq!(cloud.cloud_live_unowned_routed, 1);
}

#[test]
fn apple_partial_or_error_never_claims_a_live_final() {
    assert!(!apple_stop_final_received(&[]));
    assert!(!apple_stop_final_received(&[LiveStreamEvent::Partial {
        text: "visible words".into(),
        segments: Vec::new(),
    }]));
    assert!(!apple_stop_final_received(&[LiveStreamEvent::Summary {
        text: "visible words".into(),
        segments: Vec::new(),
        ok: false,
        error: Some("lane lost".into()),
    }]));
    assert!(apple_stop_final_received(&[LiveStreamEvent::Summary {
        text: String::new(),
        segments: Vec::new(),
        ok: true,
        error: None,
    }]));
    let final_event = LiveStreamEvent::PhraseFinal {
        text: "complete words".into(),
        segments: Vec::new(),
    };
    assert!(apple_stop_final_received(std::slice::from_ref(
        &final_event
    )));
    assert!(!apple_stop_final_received(&[
        final_event,
        LiveStreamEvent::Error {
            message: "lane lost".into(),
        },
    ]));
}

#[test]
fn live_final_receipt_follows_paint_even_when_the_lane_is_lost() {
    struct PaintSink {
        completion: Mutex<tokio::sync::oneshot::Receiver<()>>,
        painted: Mutex<Vec<EngineEvent>>,
    }
    impl EventSink for PaintSink {
        fn on_event(&self, event: &EngineEvent) {
            assert_eq!(
                self.completion.lock().unwrap().try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty),
                "the stop receipt cannot precede painting its final events",
            );
            self.painted.lock().unwrap().push(event.clone());
        }
    }
    for succeeded in [true, false] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (completion, receiver) = tokio::sync::oneshot::channel();
        let sink = PaintSink {
            completion: Mutex::new(receiver),
            painted: Mutex::new(Vec::new()),
        };
        tx.send(EngineEvent::NoSpeech {
            reason: "last admitted event".into(),
        })
        .unwrap();
        let mut completion = Some(completion);
        forward_live_finals_admitted(succeeded, &mut rx, &sink, None, &mut completion);
        assert_eq!(sink.painted.lock().unwrap().len(), 1);
        assert!(completion.is_none());
        let received = sink.completion.lock().unwrap().try_recv();
        if succeeded {
            assert_eq!(received, Ok(()));
        } else {
            assert_eq!(
                received,
                Err(tokio::sync::oneshot::error::TryRecvError::Closed),
            );
        }
    }
}

#[test]
fn actual_lane_stop_retains_end_finals_in_the_forward_queue_once() {
    use crate::asr_session::{
        AsrSessionEvent, FakeAsrSessionProvider, Layer1Decision, RefinerMode, TranscriptEvent,
    };
    let input = Layer1SessionInput {
        session_id: Layer1SessionId::new("stop-forward-queue").unwrap(),
        locale: None,
        sample_rate: RATE,
    };
    let mut first = cloud_notice_final("first", &[], 0, 500);
    first.session_id = input.session_id.clone();
    let mut last = cloud_notice_final("last", &[], 500, 1_000);
    last.session_id = input.session_id.clone();
    last.utterance_id = 2;
    last.sequence_id = 3;
    let interim = TranscriptEvent {
        session_id: input.session_id.clone(),
        utterance_id: 2,
        sequence_id: 2,
        text: "unfinished".into(),
        range: None,
        commit: None,
    };
    let provider = FakeAsrSessionProvider::with_script(
        RefinerMode::CloudSession,
        vec![
            AsrSessionEvent::Final(first.clone()),
            AsrSessionEvent::Partial(interim),
            AsrSessionEvent::Final(last.clone()),
        ],
    );
    let mut lane = RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input);
    let _ = lane.offer_pcm(&vec![0.25; 500]);
    let _ = lane.flush_holdback();
    lane.poll();
    assert_eq!(lane.take_unforwarded_finals(), vec![first.clone()]);
    let _ = lane.offer_pcm(&vec![0.25; 500]);
    // The second push releases only the partial; the final is released
    // by the real lane.stop → provider.close → route_event path.
    let outcome = lane.stop();
    assert!(outcome.degrade_reason().is_none());
    assert_eq!(outcome.finals(), &[first, last.clone()]);
    assert!(cloud_stop_finals_cover_capture(
        outcome.finals(),
        lane.pushed_samples()
    ));
    assert_eq!(lane.take_unforwarded_finals(), vec![last]);
    assert!(lane.take_unforwarded_finals().is_empty());
}

#[test]
fn clean_cloud_transport_stop_without_an_end_final_is_not_complete() {
    use crate::asr_session::{FakeAsrSessionProvider, Layer1Decision, RefinerMode};
    let input = Layer1SessionInput {
        session_id: Layer1SessionId::new("stop-missing-final").unwrap(),
        locale: None,
        sample_rate: RATE,
    };
    for captured_samples in [0, 1_000] {
        let provider = FakeAsrSessionProvider::with_script(RefinerMode::CloudSession, Vec::new());
        let mut lane = RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input);
        let _ = lane.offer_pcm(&vec![0.25; captured_samples]);
        let outcome = lane.stop();
        assert!(outcome.degrade_reason().is_none());
        assert_eq!(
            cloud_stop_finals_cover_capture(outcome.finals(), lane.pushed_samples()),
            captured_samples == 0,
        );
    }
    let end_only = cloud_notice_final("end", &[], 500, 1_000);
    assert!(!cloud_stop_finals_cover_capture(&[end_only], 1_000));
}

#[test]
fn cloud_end_final_is_admitted_before_live_final_completion() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let (notice_tx, notice_rx) = std_mpsc::channel();
    let mut state = state("cloud-stop-final", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let owner = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    notice_tx
        .send(CloudWorkerNotice::Final(Box::new(cloud_notice_final(
            "cloud",
            &[("cloud", sample(0.2), sample(0.6))],
            0,
            sample(1.5),
        ))))
        .unwrap();
    notice_tx.send(CloudWorkerNotice::EndSettled).unwrap();
    assert!(drain_cloud_stop_finals(&mut state, &tx, &notice_rx));
    assert_eq!(state.cloud_live_admitted, 1);
    assert!(!cloud_open(&state, &owner));
    assert!(state.cloud_inflight.is_empty());
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&owner)
            .unwrap()
            .iter()
            .any(|slot| slot.producer == LedgerObservationProducer::CloudLive)
    );
}

#[test]
fn cloud_lane_loss_releases_stop_without_a_success_receipt() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (commit_tx, _commit_rx) = mpsc::channel(4);
    let (notice_tx, notice_rx) = std_mpsc::channel();
    let mut state = state("cloud-stop-lost", 2.0);
    state.cloud_commit_tx = Some(commit_tx);
    let owner = cloud_owner(&mut state, &tx, 1, 0.0, 1.0);
    state.commit_cloud_close(&tx, silence_close(1.5));
    // Keep the notice sender alive and send no end receipt: lane loss is
    // enough to release this wait, even with outstanding commit debt.
    notice_tx.send(CloudWorkerNotice::LaneLost).unwrap();
    assert!(!drain_cloud_stop_finals(&mut state, &tx, &notice_rx));
    assert!(!cloud_open(&state, &owner));
    assert!(state.cloud_inflight.is_empty());
    let (completion, mut receiver) = tokio::sync::oneshot::channel();
    forward_live_finals_admitted(
        false,
        &mut rx,
        &RecordingSink::default(),
        None,
        &mut Some(completion),
    );
    assert_eq!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed),
    );
}

#[test]
fn internal_seal_ack_does_not_release_a_pending_cloud_final() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let (_live_final_tx, mut live_final_rx) = tokio::sync::oneshot::channel::<()>();
    let mut state = state("stop-ack", 1.0);
    let occurrence = qualify(&mut state, 0.0, 0.5);
    state.cloud_inflight.push_back(super::PendingCloudCommit {
        sample_end: sample(1.0),
        occurrences: vec![occurrence],
    });
    finish_capture_after_seal(
        &mut state,
        &tx,
        1.0,
        ack_tx,
        false,
        || Ok(Vec::new()),
        |_| {},
    )
    .expect("finish");
    assert!(ack_rx.try_recv().is_ok());
    assert_eq!(state.cloud_inflight.len(), 1);
    assert_eq!(
        live_final_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty),
    );
}
