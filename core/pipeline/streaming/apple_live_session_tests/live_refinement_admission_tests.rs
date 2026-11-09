/// W2 contracts: virtual time, owned synthetic PCM, no audio/model invocation.
use super::super::silero_fusion::UtteranceLedger;
use super::*;

const RATE: u32 = 1_000;
/// Calibrated existence floor of the fixture's capture device. Fixture
/// speech is 0.25 RMS, so a word over measured fixture PCM clears it.
const EXISTENCE_FLOOR_RMS: f32 = 0.01;

fn fixture(
    capacity: usize,
) -> (
    AppleSealState,
    mpsc::UnboundedSender<EngineEvent>,
    mpsc::UnboundedReceiver<EngineEvent>,
    mpsc::Receiver<TailPatchRequest>,
) {
    let (events, receiver) = mpsc::unbounded_channel();
    let (sender, requests) = mpsc::channel(capacity);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        RATE,
        "live-admission".into(),
        7,
        sender,
        Arc::new(Mutex::new(AcousticLedger::new())),
        Some(EnergyCalibration {
            version: "owned-fixture".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        }),
    );
    state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
    state.audio.push(&vec![0.25; 20_000]);
    (state, events, receiver, requests)
}

fn closed(count: u64) -> UtteranceLedger {
    let mut ledger = UtteranceLedger::new();
    for id in 0..count {
        ledger.open_or_extend("live-admission", 7, id * 1_000, id * 1_000 + 400);
        ledger.close_open(id * 1_000 + 400);
    }
    ledger
}

/// Stop freezes the head. A head shorter than one 9 s window is that one partial.
fn pump_stopped(state: &mut AppleSealState, events: &mpsc::UnboundedSender<EngineEvent>) {
    state.capture_stopping = true;
    state.pump_capture_windows(events);
}

fn finish(request: &TailPatchRequest) -> TailPatchCompletion {
    TailPatchCompletion {
        submission_sequence: request.submission_sequence,
        utterance_id: request.utterance_id,
        request_identity: Some(request.provider_request.identity.clone()),
        payload: None,
        member_occurrences: request.member_occurrences.clone(),
    }
}

pub(super) fn refinement_log(run: impl FnOnce()) -> String {
    let buffer = Arc::new(Mutex::new(String::new()));
    let capture = RefinementCapture {
        buffer: Arc::clone(&buffer),
        next_span: std::sync::atomic::AtomicU64::new(1),
    };
    tracing::subscriber::with_default(capture, run);
    buffer.lock().expect("refinement log").clone()
}

struct RefinementCapture {
    buffer: Arc<Mutex<String>>,
    next_span: std::sync::atomic::AtomicU64,
}

impl tracing::Subscriber for RefinementCapture {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let id = self
            .next_span
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::span::Id::from_u64(id.max(1))
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = FieldVisitor(String::new());
        event.record(&mut visitor);
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push_str(&visitor.0);
            buffer.push('\n');
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

struct FieldVisitor(String);

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={} ", field.name(), value);
    }
}

fn warnings(receiver: &mut mpsc::UnboundedReceiver<EngineEvent>, code: &str) -> usize {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| matches!(event, EngineEvent::Warning { code: found, .. } if found == code))
        .count()
}

fn labelled_completion(request: &TailPatchRequest) -> TailPatchCompletion {
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailTimingQuality,
    };
    let occurrence = &request.member_occurrences[0].1;
    let mut completion = finish(request);
    completion.payload = Some(TailProviderPayload {
        identity: request.provider_request.identity.clone(),
        text: "hello".into(),
        segments: vec![TimedTailSegment {
            confidence: None,
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            text: "hello".into(),
            range: TailSampleRange {
                session: occurrence.session.clone(),
                capture_epoch: occurrence.capture_epoch,
                sample_start: occurrence.sample_start,
                sample_end: occurrence.sample_end,
            },
        }],
        avg_logprob: None,
        compression_ratio: None,
        provider_id: TailProviderId::Fake,
        elapsed_ms: 0,
        evidence: TailProviderEvidence {
            segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            source: TailEvidenceSource::Whisper,
            revision: Some("synthetic-live-admission".into()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::Synthetic,
            avg_logprob: None,
            vad_selected_samples: None,
        },
    });
    completion
}

// Root-owned transport controls: real fixture energy, supplied decoder words.
fn forensic_live_transport_capture(state: &mut AppleSealState) {
    let pcm = vec![0.25; 400];
    state.audio = LiveAudioBuffer::new(RATE, 20.0);
    state.audio.push(&pcm);
    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    writer.push_samples(&pcm);
    let speech = coverage_speech_evidence(state);
    assert_eq!(
        speech.producer(),
        crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER
    );
    assert!(speech.observed_speech());
    assert_eq!(speech.availability().observed_samples(), Some(400));
    assert_eq!(state.audio.session_sample_end(), 400);
}

fn forensic_live_transport_word_completion(request: &TailPatchRequest) -> TailPatchCompletion {
    let mut completion = labelled_completion(request);
    let payload = completion.payload.as_mut().unwrap();
    payload.segments[0].grain = crate::stt::tail_provider::TailSegmentGrain::Word;
    payload.segments[0].range.sample_start = 50;
    payload.segments[0].range.sample_end = 350;
    payload.evidence.segment_grain = crate::stt::tail_provider::TailSegmentGrain::Word;
    payload.evidence.timing_quality =
        crate::stt::tail_provider::TailTimingQuality::ExactSampleRange;
    completion
}

fn forensic_live_transport_words_completion(
    request: &TailPatchRequest,
    text: &str,
) -> TailPatchCompletion {
    request
        .provider_request
        .validate_pcm(&request.audio)
        .unwrap();
    assert_eq!(request.audio, vec![0.25; 400]);
    let mut completion = forensic_live_transport_word_completion(request);
    let payload = completion.payload.as_mut().unwrap();
    payload.text = text.into();
    let template = payload.segments[0].clone();
    let words = text.split_whitespace().collect::<Vec<_>>();
    payload.segments = words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let mut segment = template.clone();
            segment.text = (*word).into();
            segment.range.sample_start = 50 + (300 * i / words.len()) as u64;
            segment.range.sample_end = 50 + (300 * (i + 1) / words.len()) as u64;
            segment
        })
        .collect();
    completion
}

#[test]
fn word_dispute_uses_grid_witnesses_then_at_most_one_bounded_trial() {
    for word_start in [6_000, 6_200] {
        let (mut state, events, _receiver, mut requests) = fixture(8);
        let pcm = vec![0.25; 15_000];
        state.audio = LiveAudioBuffer::new(RATE, 20.0);
        state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&pcm);
        let mut utterances = UtteranceLedger::new();
        utterances.open_or_extend("live-admission", 7, word_start, word_start + 400);
        utterances.close_open(word_start + 400);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &utterances,
            &[TranscriptSegment {
                confidence: None,
                text: "1286".into(),
                start_ts: word_start as f32 / RATE as f32,
                end_ts: (word_start + 400) as f32 / RATE as f32,
            }],
        );
        state.pump_capture_windows(&events);
        let windows = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(windows.len(), 3);
        for (request, bounds) in windows
            .iter()
            .zip([(0, 9_000), (3_000, 12_000), (6_000, 15_000)])
        {
            let range = &request.provider_request.identity.range;
            assert_eq!((range.sample_start, range.sample_end), bounds);
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        let owner = windows[0].member_occurrences[0].1.clone();
        assert!(
            windows
                .iter()
                .all(|request| request.member_occurrences == vec![(1, owner.clone())])
        );
        let witness = |request: &TailPatchRequest| {
            let mut completion = forensic_live_transport_word_completion(request);
            let payload = completion.payload.as_mut().unwrap();
            payload.text = "56".into();
            payload.segments[0].text = "56".into();
            payload.segments[0].range.sample_start = word_start;
            payload.segments[0].range.sample_end = word_start + 400;
            completion
        };
        state.complete_whisper_window(&events, witness(&windows[0]), 15.0);
        assert_eq!(state.tail_patch_awaiting_completion(), 2);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("1286")
        );
        // A repeated return of the same original frame cannot become an independent vote.
        state.complete_whisper_window(&events, witness(&windows[0]), 15.1);
        assert_eq!(state.tail_patch_awaiting_completion(), 2);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("1286")
        );
        for request in &windows[1..] {
            state.complete_whisper_window(&events, witness(request), 15.2);
        }
        let needs_trial = false;
        assert!(!state.acoustic_ledger.lock().unwrap().has_word_conflicts());
        assert_eq!(
            state.tail_patch_awaiting_completion(),
            u64::from(needs_trial)
        );
        if needs_trial {
            let trial = requests
                .try_recv()
                .expect("grid left an unresolved lexical dispute");
            let range = &trial.provider_request.identity.range;
            assert!(trial.provider_request.identity.request_id >= (1_u64 << 63));
            assert!(range.sample_start < word_start && range.sample_end > word_start + 400);
            assert!(range.sample_end - range.sample_start <= 9_000);
            assert_eq!(
                trial.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
            state.complete_whisper_window(&events, witness(&trial), 15.3);
        }
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        state.pump_capture_windows(&events);
        assert!(
            requests.try_recv().is_err(),
            "resolved dispute must not dispatch another inference"
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("56"));
        assert!(!ledger.has_word_conflicts());
        assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
        assert!(
            ledger
                .layer_trail_for(&owner)
                .any(|entry| entry.candidate_label == "1286")
        );
        assert_eq!(ledger.conservation().residue(), 0);
        assert!(ledger.word_deletions().is_empty());
    }
}

#[test]
fn forensic_live_transport_foreign_envelope_waits_for_exact_word_completion() {
    for context in ["utterance-only", "symmetric-pad"] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        forensic_live_transport_capture(&mut state);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        pump_stopped(&mut state, &events);
        let request = requests.try_recv().expect("real submitted job");
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        assert_eq!(request.audio, vec![0.25; 400]);
        let owner = &request.member_occurrences[0].1;
        assert_eq!(owner, &OccurrenceIdentity::new("live-admission", 7, 0, 400));
        while receiver.try_recv().is_ok() {}
        for defect in [
            "sequence",
            "request",
            "session",
            "epoch",
            "start",
            "end",
            "member",
            "member_span",
            "members",
            "utterance",
            "missing",
        ] {
            let mut completion = forensic_live_transport_word_completion(&request);
            let identity = completion.request_identity.as_mut().unwrap();
            match defect {
                "sequence" => completion.submission_sequence += 1,
                "request" => identity.request_id += 1,
                "session" => identity.range.session = "foreign".into(),
                "epoch" => identity.range.capture_epoch += 1,
                "start" => identity.range.sample_start += 1,
                "end" => identity.range.sample_end += 1,
                "member" => completion.member_occurrences[0].0 += 1,
                "member_span" => completion.member_occurrences[0].1.sample_end += 1,
                "members" => completion.member_occurrences.clear(),
                "utterance" => completion.utterance_id += 1,
                "missing" => completion.request_identity = None,
                _ => unreachable!(),
            }
            state.complete_whisper_window(&events, completion, 20.0);
            state.close_admission_horizon(&events, owner.sample_end);
            assert_eq!(
                state.tail_patch_awaiting_completion(),
                1,
                "{context:?}/{defect}"
            );
            assert_eq!(state.refinement_submitted.len(), 1);
            assert!(state.pending_events.contains_key(&request.utterance_id));
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(owner), None);
            assert!(ledger.text_recovery_pending(owner));
            assert!(!ledger.is_sealed(owner));
            assert_eq!(
                ledger.seal_terminal("live-admission", 7),
                Err(SealRefusal::TextRecoveryPending)
            );
            assert!(
                ledger
                    .frontier_of(owner)
                    .unwrap()
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            );
            drop(ledger);
            assert!(receiver.try_recv().is_err());
        }
        state.complete_whisper_window(
            &events,
            forensic_live_transport_word_completion(&request),
            20.0,
        );
        state.close_admission_horizon(&events, owner.sample_end);
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        assert!(state.refinement_submitted.is_empty());
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(owner), Some("hello"));
        assert!(!ledger.text_recovery_pending(owner));
        assert!(ledger.is_sealed(owner));
        let slots = ledger.slots_of(owner).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!((slots[0].sample_start, slots[0].sample_end), (50, 350));
        assert_eq!(slots[0].producer, LedgerObservationProducer::Whisper);
        let coverage = ledger.assess_seal_coverage(
            "live-admission",
            7,
            &coverage_speech_evidence(&state),
            250,
        );
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert!(ledger.record_seal_coverage(coverage));
        assert!(ledger.seal_terminal("live-admission", 7).is_ok());
        assert_eq!(ledger.conservation().residue(), 0);
        drop(ledger);
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            1
        );
        assert_eq!(
            emitted
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::UtteranceFinal { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec!["hello"]
        );
        state.complete_whisper_window(
            &events,
            forensic_live_transport_word_completion(&request),
            20.0,
        );
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn forensic_live_transport_phrase_and_invalid_payload_cannot_seal_recovery() {
    for context in ["utterance-only", "symmetric-pad"] {
        for defect in [
            "phrase",
            "segmentless",
            "session",
            "epoch",
            "outside",
            "empty",
            "reversed",
            "payload_identity",
            "no_payload",
        ] {
            let (mut state, events, mut receiver, mut requests) = fixture(1);
            forensic_live_transport_capture(&mut state);
            reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
            pump_stopped(&mut state, &events);
            let request = requests.try_recv().unwrap();
            let owner = &request.member_occurrences[0].1;
            let mut completion = labelled_completion(&request);
            let payload = completion.payload.as_mut().unwrap();
            match defect {
                "phrase" => {}
                "segmentless" => payload.segments.clear(),
                "session" => payload.segments[0].range.session = "foreign".into(),
                "epoch" => payload.segments[0].range.capture_epoch += 1,
                "outside" => payload.segments[0].range.sample_end += 1,
                "empty" => payload.segments[0].range.sample_end = 0,
                "reversed" => payload.segments[0].range.sample_start = 401,
                "payload_identity" => payload.identity.range.capture_epoch += 1,
                "no_payload" => completion.payload = None,
                _ => unreachable!(),
            }
            while receiver.try_recv().is_ok() {}
            state.complete_whisper_window(&events, completion, 20.0);
            state.close_admission_horizon(&events, owner.sample_end);
            assert_eq!(state.tail_patch_awaiting_completion(), 0);
            assert!(state.refinement_submitted.is_empty());
            let expected = None;
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(owner), expected, "{context:?}/{defect}");
            assert!(ledger.text_recovery_pending(owner), "{context:?}/{defect}");
            assert!(!ledger.is_sealed(owner));
            assert!(
                ledger
                    .frontier_of(owner)
                    .unwrap()
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper),
                "{context:?}/{defect}: a refused payload does not return Whisper"
            );
            assert_eq!(ledger.seal(owner), Err(SealRefusal::TextRecoveryPending));
            assert_eq!(
                ledger.seal_terminal("live-admission", 7),
                Err(SealRefusal::TextRecoveryPending)
            );
            assert_eq!(ledger.conservation().residue(), 0);
            drop(ledger);
            let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
            assert!(!emitted.iter().any(|event| matches!(
                event,
                EngineEvent::LedgerSeal { .. } | EngineEvent::UtteranceFinal { .. }
            )));
            state.complete_whisper_window(
                &events,
                forensic_live_transport_word_completion(&request),
                20.0,
            );
            assert!(
                receiver.try_recv().is_err(),
                "a consumed request cannot admit a replay"
            );
            assert_eq!(
                state.acoustic_ledger.lock().unwrap().text_of(owner),
                expected
            );
        }
    }
}

fn owner_reservation_fixture(
    state: &mut AppleSealState,
    events: &mpsc::UnboundedSender<EngineEvent>,
    id: u64,
    occurrence: &OccurrenceIdentity,
    label: &str,
) {
    assert!(qualify_owned_occurrence(state, occurrence));
    state.pending_events.insert(
        id,
        PendingAppleSeal {
            occurrence: occurrence.clone(),
            raw_text: label.into(),
            layer1_baseline: label.into(),
            start_ts: occurrence.sample_start as f32 / RATE as f32,
            end_ts: occurrence.sample_end as f32 / RATE as f32,
            segments: Vec::new(),
        },
    );
    for producer in [
        LedgerObservationProducer::Apple,
        LedgerObservationProducer::Lexicon,
    ] {
        let observation = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .next_word_observation(producer, id, occurrence);
        assert!(
            admit_ledger_label(
                state,
                events,
                LabelAdmission {
                    observation,
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                }
            )
            .is_some()
        );
    }
    if state.tail_patch.is_some() {
        assert!(state.register_whisper_owner(events, id, occurrence));
    }
}

fn dictation_reservation_fixture(
    state: &mut AppleSealState,
    events: &mpsc::UnboundedSender<EngineEvent>,
) -> OccurrenceIdentity {
    forensic_live_transport_capture(state);
    let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    owner_reservation_fixture(state, events, 1, &occurrence, "Apple words");
    occurrence
}

#[test]
fn dictation_expected_whisper_blocks_formatter_before_delayed_enqueue() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        assert!(formatter_lane_is_armed(policy, true));
        for intent in [CaptureTurnIntent::HandsFree, CaptureTurnIntent::SingleTurn] {
            let (mut state, events, _receiver, _requests) = fixture(1);
            state.capture_turn = intent;
            let (formatter, mut formats) = mpsc::channel(1);
            state.formatter = Some(formatter);
            let occurrence = dictation_reservation_fixture(&mut state, &events);
            assert!(
                formats.try_recv().is_err(),
                "{intent:?}/{policy:?}: Whisper not settled"
            );
            state.close_admission_horizon(&events, occurrence.sample_end);
            assert!(
                formats.try_recv().is_err(),
                "PCM horizon is not a Whisper return"
            );
            assert_eq!(
                state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .frontier_of(&occurrence)
                    .unwrap()
                    .open_producers(),
                vec![LedgerObservationProducer::Whisper]
            );
        }
    }
}

#[test]
fn dictation_expected_whisper_stop_timeout_releases_formatter_with_receipt() {
    let (mut state, events, mut receiver, _requests) = fixture(1);
    let (formatter, mut formats) = mpsc::channel(1);
    state.formatter = Some(formatter);
    let occurrence = dictation_reservation_fixture(&mut state, &events);
    assert!(formats.try_recv().is_err());
    let deadline = state.refinement_clock + TAIL_PATCH_CLOSURE_TIMEOUT;
    assert!(!state.stop_refinements_tick(&events, deadline, deadline));
    assert_eq!(formats.try_recv().unwrap().occurrence, occurrence);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
        1
    );
    state.return_outstanding_whisper_without_label(&events);
    assert!(formats.try_recv().is_err(), "settlement is exactly once");
}

#[test]
fn dictation_expected_whisper_lane_loss_releases_unqueued_reservation() {
    let (mut state, events, mut receiver, requests) = fixture(1);
    let (formatter, mut formats) = mpsc::channel(1);
    state.formatter = Some(formatter);
    let occurrence = dictation_reservation_fixture(&mut state, &events);
    assert!(formats.try_recv().is_err());
    drop(requests);
    let now = state.refinement_clock;
    state.tick_refinements(&events, now);
    assert_eq!(formats.try_recv().unwrap().occurrence, occurrence);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::LaneGone.code()),
        1
    );
    state.tick_refinements(&events, now);
    assert!(formats.try_recv().is_err());
}

#[test]
fn dictation_expected_whisper_returns_and_overlap_refusals_release_formatter() {
    for disposition in ["label", "empty", "overlap", "invalid_window"] {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        let (formatter, mut formats) = mpsc::channel(1);
        state.formatter = Some(formatter);
        let occurrence = dictation_reservation_fixture(&mut state, &events);
        assert!(
            formats.try_recv().is_err(),
            "{disposition}: covering work is open"
        );
        state.capture_stopping = true;
        state.pump_capture_windows(&events);
        let request = requests.try_recv().unwrap();
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        assert_eq!(request.audio, vec![0.25; 400]);
        let mut completion = if disposition == "empty" {
            finish(&request)
        } else {
            labelled_completion(&request)
        };
        if disposition == "overlap" {
            completion.payload.as_mut().unwrap().segments[0]
                .range
                .sample_end += 1;
        } else if disposition == "invalid_window" {
            completion
                .request_identity
                .as_mut()
                .unwrap()
                .range
                .sample_end += 1;
        }
        state.complete_whisper_window(&events, completion, 0.4);
        if disposition == "invalid_window" {
            assert_eq!(state.tail_patch_awaiting_completion(), 1);
            assert!(
                formats.try_recv().is_err(),
                "foreign envelope cannot settle work"
            );
            state.complete_whisper_window(&events, finish(&request), 0.4);
        }
        state.close_admission_horizon(&events, u64::MAX);
        assert_eq!(
            formats.try_recv().unwrap().occurrence,
            occurrence,
            "{disposition}"
        );
        assert!(formats.try_recv().is_err());
        assert!(state.refinement_submitted.is_empty());
    }
}

#[test]
fn dictation_expected_whisper_stop_deadline_invalidates_late_work() {
    for enqueue in [false, true] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        let (formatter, mut formats) = mpsc::channel(1);
        state.formatter = Some(formatter);
        let occurrence = dictation_reservation_fixture(&mut state, &events);
        assert!(formats.try_recv().is_err());
        state.capture_stopping = true;
        let completion = if enqueue {
            state.pump_capture_windows(&events);
            Some(labelled_completion(&requests.try_recv().unwrap()))
        } else {
            None
        };
        let deadline = state.refinement_clock + TAIL_PATCH_CLOSURE_TIMEOUT;
        assert!(!state.stop_refinements_tick(&events, deadline, deadline));
        let request = formats
            .try_recv()
            .expect("absolute Stop deadline settles reservation");
        assert_eq!(request.occurrence, occurrence);
        assert_eq!(request.existing_label, "Apple words");
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
            1
        );
        if let Some(completion) = completion {
            state.complete_whisper_window(&events, completion, 0.4);
        }
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(&occurrence),
            Some("Apple words")
        );
        assert!(state.refinement_submitted.is_empty());
        assert!(state.refinement_pending.is_empty());
        assert!(state.window_plan.is_finished());
        assert!(formats.try_recv().is_err());
    }
}

#[test]
fn dictation_expected_whisper_session_end_settles_unqueued_reservation() {
    let (mut state, events, mut receiver, _requests) = fixture(1);
    let (formatter, mut formats) = mpsc::channel(1);
    state.formatter = Some(formatter);
    let occurrence = dictation_reservation_fixture(&mut state, &events);
    assert!(formats.try_recv().is_err());
    state.close_admission_horizon(&events, u64::MAX);
    assert!(
        formats.try_recv().is_err(),
        "an open PCM plan cannot be skipped"
    );
    state.capture_stopping = true;
    state.return_outstanding_whisper_without_label(&events);
    assert_eq!(formats.try_recv().unwrap().occurrence, occurrence);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
        1
    );
    assert!(state.refinement_pending.is_empty());
    assert!(state.refinement_submitted.is_empty());
    assert!(state.window_plan.is_finished());
    state.return_outstanding_whisper_without_label(&events);
    assert!(formats.try_recv().is_err());
}

#[test]
fn dictation_expected_whisper_return_preserves_disjoint_work() {
    let (mut state, events, _receiver, mut requests) = fixture(8);
    let pcm = vec![0.25; 16_000];
    state.audio = LiveAudioBuffer::new(RATE, 20.0);
    state.audio.push(&pcm);
    let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
    writer.push_samples(&pcm);
    let (formatter, mut formats) = mpsc::channel(2);
    state.formatter = Some(formatter);
    let first = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    let second = OccurrenceIdentity::new("live-admission", 7, 12_000, 12_400);
    owner_reservation_fixture(&mut state, &events, 1, &first, "first");
    owner_reservation_fixture(&mut state, &events, 2, &second, "second");
    state.pump_capture_windows(&events);
    let windows = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(windows.len(), 3);
    assert_eq!(windows[0].member_occurrences, vec![(1, first.clone())]);
    assert_eq!(windows[2].member_occurrences, vec![(2, second.clone())]);
    state.complete_whisper_window(&events, finish(&windows[0]), 16.0);
    assert_eq!(formats.try_recv().unwrap().occurrence, first);
    assert!(
        formats.try_recv().is_err(),
        "disjoint covering work remains open"
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 2);
    state.complete_whisper_window(&events, labelled_completion(&windows[0]), 16.0);
    assert_eq!(
        state.tail_patch_awaiting_completion(),
        2,
        "replay cannot consume another frame"
    );
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&first),
        Some("first")
    );
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&second),
        Some("second")
    );
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .frontier_of(&second)
            .unwrap()
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .conservation()
            .residue(),
        0
    );
}

#[test]
fn dictation_whisper_disabled_keeps_immediate_formatter_handoff() {
    let (mut state, events, _receiver, _requests) = fixture(1);
    state.tail_patch = None;
    let (formatter, mut formats) = mpsc::channel(1);
    state.formatter = Some(formatter);
    let occurrence = dictation_reservation_fixture(&mut state, &events);
    assert_eq!(formats.try_recv().unwrap().occurrence, occurrence);
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .frontier_of(&occurrence)
            .unwrap()
            .open_producers(),
        Vec::<LedgerObservationProducer>::new()
    );
}

#[test]
fn agent_channel_expected_whisper_blocks_frontier_before_enqueue() {
    let (mut state, events, mut receiver, _requests) = fixture(1);
    state.capture_turn = CaptureTurnIntent::AgentChannel;
    let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    assert!(qualify_owned_occurrence(&state, &occurrence));
    for producer in [
        LedgerObservationProducer::Apple,
        LedgerObservationProducer::Lexicon,
    ] {
        let observation =
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .next_word_observation(producer, 1, &occurrence);
        assert!(
            admit_ledger_label(
                &mut state,
                &events,
                LabelAdmission {
                    observation,
                    label: "Apple words",
                    energy: EnergyAdmission::RequireExistingQualification,
                }
            )
            .is_some()
        );
    }
    assert!(state.refinement_submitted.is_empty());
    assert!(state.refinement_pending.is_empty());
    assert_eq!(state.windows_admitted, 0);
    state.close_admission_horizon(&events, occurrence.sample_end);
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(
        ledger.frontier_of(&occurrence).unwrap().open_producers(),
        vec![LedgerObservationProducer::Whisper]
    );
    assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::FrontierOpen));
    assert!(
        !std::iter::from_fn(|| receiver.try_recv().ok())
            .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
    );
}

#[test]
fn relay_acceptance_hands_free_formatter_waits_for_expected_whisper() {
    let (mut state, events, _receiver, mut requests) = fixture(1);
    state.capture_turn = CaptureTurnIntent::HandsFree;
    let (formatter, mut formatted) = mpsc::channel(FORMATTER_QUEUE_CAP);
    state.formatter = Some(formatter);
    let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    assert!(qualify_owned_occurrence(&state, &occurrence));
    // Explicitly control the return order: Apple and Lexicon finish before
    // the coalescer submits any Whisper work. No private frontier mutation
    // in the fixture may manufacture the reservation being tested.
    for producer in [
        LedgerObservationProducer::Apple,
        LedgerObservationProducer::Lexicon,
    ] {
        let observation =
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .next_word_observation(producer, 1, &occurrence);
        admit_ledger_label(
            &mut state,
            &events,
            LabelAdmission {
                observation,
                label: "czy weryfikowałeś",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .expect("qualified acoustic owner");
    }
    assert!(state.refinement_submitted.is_empty());
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(
        matches!(formatted.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "formatter was dispatched before expected Whisper was even enqueued"
    );
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        ledger
            .frontier_of(&occurrence)
            .unwrap()
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );
    assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::FrontierOpen));
}

#[test]
fn relay_acceptance_formatter_receives_corrected_whisper_source() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        forensic_live_transport_capture(&mut state);
        state.capture_turn = CaptureTurnIntent::HandsFree;
        let (formatter, mut formatted) = mpsc::channel(FORMATTER_QUEUE_CAP);
        assert!(live_formatter_lane_is_armed(
            state.capture_turn,
            policy,
            || true
        ));
        state.formatter = Some(formatter);
        let lexicon = tempfile::tempdir().unwrap();
        state.lexicon_custom_path = lexicon.path().join("empty.jsonl");

        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(1),
            &[TranscriptSegment {
                confidence: None,
                text: "czy weryfikowałeś".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        assert!(state.refinement_submitted.is_empty());
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(
            matches!(formatted.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "{policy:?}: an empty Whisper queue must not authorize formatter work"
        );
        assert!(
            !std::iter::from_fn(|| receiver.try_recv().ok()).any(|event| matches!(
                event,
                EngineEvent::LedgerSeal { .. } | EngineEvent::UtteranceFinal { .. }
            )),
            "Apple must not be published as settled before expected Whisper"
        );

        // Positive control: waiting cannot be implemented by disabling the
        // formatter or freezing Apple. The real submitted completion must
        // release precisely one request with corrected, enriched input.
        pump_stopped(&mut state, &events);
        let request = requests.try_recv().expect("Whisper remains enabled");
        let completion =
            forensic_live_transport_words_completion(&request, "czy plan zweryfikowałeś");
        state.complete_whisper_window(&events, completion, 20.0);
        state.close_admission_horizon(&events, u64::MAX);
        let derived = formatted
            .try_recv()
            .expect("settled Raw releases formatter");
        assert_eq!(derived.existing_label, "czy plan zweryfikowałeś");
        let source = derived.source_observation.expect("source observation");
        assert_eq!(source.occurrence, request.member_occurrences[0].1);
        assert_ne!(source.producer, LedgerObservationProducer::Apple);
        assert!(matches!(
            formatted.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[test]
fn relay_acceptance_disabled_whisper_does_not_leave_phantom_debt() {
    let (mut state, events, _receiver, _requests) = fixture(1);
    state.capture_turn = CaptureTurnIntent::HandsFree;
    state.tail_patch = None;
    let (formatter, mut formatted) = mpsc::channel(FORMATTER_QUEUE_CAP);
    state.formatter = Some(formatter);
    let lexicon = tempfile::tempdir().unwrap();
    state.lexicon_custom_path = lexicon.path().join("empty.jsonl");
    reconcile_silero_ledger(
        &mut state,
        &events,
        &closed(1),
        &[TranscriptSegment {
            confidence: None,
            text: "czy weryfikowałeś".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    let request = formatted.try_recv().expect("disabled layer owes no work");
    assert_eq!(request.existing_label, "czy weryfikowałeś");
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        !ledger
            .frontier_of(&request.occurrence)
            .unwrap()
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );
}

#[test]
fn agent_channel_raw_waits_for_whisper_correction_and_omission() {
    for (apple, whisper) in [
        ("weryfikowałeś", "zweryfikowałeś"),
        ("weryfikowałeś", "zweryfikowałeś"),
    ] {
        let (mut state, events, mut receiver, mut requests) = fixture(8);
        let pcm = vec![0.25; 15_000];
        state.audio = LiveAudioBuffer::new(RATE, 20.0);
        state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&pcm);
        let mut utterances = UtteranceLedger::new();
        utterances.open_or_extend("live-admission", 7, 6_200, 6_600);
        utterances.close_open(6_600);
        state.capture_turn = CaptureTurnIntent::AgentChannel;
        reconcile_silero_ledger(
            &mut state,
            &events,
            &utterances,
            &[TranscriptSegment {
                confidence: None,
                text: apple.into(),
                start_ts: 6.2,
                end_ts: 6.6,
            }],
        );
        let occurrence = OccurrenceIdentity::new("live-admission", 7, 6_200, 6_600);
        {
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert!(!ledger.is_sealed(&occurrence));
            assert!(
                ledger
                    .frontier_of(&occurrence)
                    .unwrap()
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            );
            assert_eq!(ledger.text_of(&occurrence), Some(apple));
        }
        assert!(
            state.refinement_submitted.is_empty(),
            "no submitted job is not settlement"
        );
        assert!(
            !std::iter::from_fn(|| receiver.try_recv().ok()).any(|event| matches!(
                event,
                EngineEvent::UtteranceFinal { .. } | EngineEvent::LedgerSeal { .. }
            ))
        );
        state.pump_capture_windows(&events);
        let windows = std::iter::from_fn(|| requests.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(windows.len(), 3);
        for (request, bounds) in windows
            .iter()
            .zip([(0, 9_000), (3_000, 12_000), (6_000, 15_000)])
        {
            let range = &request.provider_request.identity.range;
            assert_eq!((range.sample_start, range.sample_end), bounds);
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let mut completion = forensic_live_transport_word_completion(request);
            let payload = completion.payload.as_mut().unwrap();
            payload.text = whisper.into();
            let template = payload.segments[0].clone();
            let words = whisper.split_whitespace().collect::<Vec<_>>();
            payload.segments = words
                .iter()
                .enumerate()
                .map(|(i, word)| {
                    let mut segment = template.clone();
                    segment.text = (*word).into();
                    segment.range.sample_start = 6_200 + (400 * i / words.len()) as u64;
                    segment.range.sample_end = 6_200 + (400 * (i + 1) / words.len()) as u64;
                    segment
                })
                .collect();
            state.complete_whisper_window(&events, completion, 15.0);
        }
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        state.capture_stopping = true;
        state.close_admission_horizon(&events, u64::MAX);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some(whisper));
        assert!(ledger.is_sealed(&occurrence));
        assert!(
            ledger
                .layer_trail()
                .iter()
                .all(|decision| decision.producer() != LedgerObservationProducer::Formatter)
        );
        let finals = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(finals, vec![whisper.to_string()]);
    }
}

#[test]
fn agent_channel_timeout_invalidates_submissions_and_keeps_explicit_status() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    state.capture_turn = CaptureTurnIntent::AgentChannel;
    reconcile_silero_ledger(
        &mut state,
        &events,
        &closed(1),
        &[TranscriptSegment {
            confidence: None,
            text: "Apple words".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    forensic_live_transport_capture(&mut state);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let deadline = state.refinement_clock + TAIL_PATCH_CLOSURE_TIMEOUT;
    assert!(!state.stop_refinements_tick(&events, deadline, deadline));
    let warnings = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter_map(|event| match event {
            EngineEvent::Warning { code, message } => Some((code, message)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(warnings.iter().any(
        |(code, message)| code == RefinementFailure::StopDeadline.code()
            && message.contains("refinement_not_completed=true")
    ));
    assert!(state.refinement_submitted.is_empty());
    assert!(state.refinement_pending.is_empty());
    let occurrence = request.member_occurrences[0].1.clone();
    let before = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .text_of(&occurrence)
        .unwrap()
        .to_string();
    state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&occurrence),
        Some(before.as_str())
    );
    assert!(TAIL_PATCH_CLOSURE_TIMEOUT > Duration::from_secs(2));
}

/// Synthetic boundary-driven integration witness, not a microphone proof.
/// Exercise production reconciliation and admission with partial Apple text.
#[test]
fn synthetic_silero_guardian_keeps_debt_for_phrase_without_exact_word_targets() {
    use super::super::silero_fusion::SileroIngress;
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
    for (succeeds, resumes) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        let mut fusion = SileroIngress::new(RATE, "live-admission", 7);
        fusion.note_observed_pcm(200, 200);
        fusion.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 0,
            speech_probability: 0.95,
        }]);
        state
            .speech_progress
            .observe_speech(&fusion.acoustic_speech_evidence(), true, RATE);
        state.speech_progress.observe_apple("partial words", 200);
        fusion.note_observed_pcm(2_400, 2_600);
        fusion.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechEnd,
            sample: 2_600,
            speech_probability: 0.05,
        }]);
        let speech = fusion.acoustic_speech_evidence();
        state.speech_progress.observe_speech(&speech, false, RATE);
        assert_eq!(state.speech_progress.debt_ms(RATE), 2_400);
        assert_eq!(
            state.speech_progress.phase(RATE),
            crate::pipeline::contracts::SpeechIntegrityPhase::Stalled
        );
        if resumes {
            state
                .speech_progress
                .observe_apple("partial words and latest words", 2_600);
            assert_eq!(state.speech_progress.debt_ms(RATE), 0);
        }
        state.fusion = Some(fusion);
        let mut physical = UtteranceLedger::new();
        physical.open_or_extend("live-admission", 7, 0, 2_600);
        physical.close_open(2_600);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &physical,
            &[TranscriptSegment {
                confidence: None,
                text: "partial words".into(),
                start_ts: 0.0,
                end_ts: 0.2,
            }],
        );
        state.audio = LiveAudioBuffer::new(RATE, 20.0);
        state.audio.push(&vec![0.25; 2_600]);
        state.capture_stopping = true;
        state.pump_capture_windows(&events);
        let request = requests.try_recv().expect("debt submits at speech close");
        let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 2_600);
        assert_eq!(request.member_occurrences, vec![(1, occurrence.clone())]);
        assert_eq!(
            OccurrenceIdentity::from(&request.provider_request.identity.range),
            occurrence
        );
        assert_eq!(request.audio, vec![0.25; 2_600]);
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        assert!(state.refinement_pending.is_empty());
        assert_eq!(state.windows_admitted, 1);
        {
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&occurrence), Some("partial words"));
            assert!(ledger.text_recovery_pending(&occurrence));
            assert_eq!(
                ledger
                    .assess_seal_coverage("live-admission", 7, &speech, 250)
                    .coverage_ratio(),
                Some(0.0)
            );
        }
        while receiver.try_recv().is_ok() {}
        let mut completion = if succeeds {
            labelled_completion(&request)
        } else {
            finish(&request)
        };
        if let Some(payload) = completion.payload.as_mut() {
            payload.text = "partial words and the recovered remainder".into();
            payload.segments[0].text = payload.text.clone();
            payload.evidence.timing_quality =
                crate::stt::tail_provider::TailTimingQuality::ExactSampleRange;
            payload.evidence.stability = crate::stt::tail_provider::TailEvidenceStability::Final;
            payload.evidence.source = crate::stt::tail_provider::TailEvidenceSource::Whisper;
        }
        state.complete_whisper_window(&events, completion, 20.0);
        state.close_admission_horizon(&events, 2_600);
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.text_recovery_pending(&occurrence));
        assert!(!ledger.is_sealed(&occurrence));
        // A phrase covering the owner does not locate its new words on
        // the already pinned partial group. Retain text and recovery debt.
        assert_eq!(ledger.text_of(&occurrence), Some("partial words"));
        if succeeds {
            assert!(ledger.slot_alternatives().iter().any(
                |candidate| candidate.candidate == "partial words and the recovered remainder"
            ));
        }
        let coverage = ledger.assess_seal_coverage("live-admission", 7, &speech, 250);
        assert_eq!(coverage.coverage_ratio(), Some(0.0));
        assert!(ledger.record_seal_coverage(coverage));
        assert_eq!(
            ledger.seal_terminal("live-admission", 7),
            Err(SealRefusal::TextRecoveryPending)
        );
        drop(ledger);
        assert!(
            state
                .speech_progress
                .occurrence_has_debt(&occurrence, &speech, RATE)
        );
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(!emitted.iter().any(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Correct { .. }, label }
                    if observation.producer == LedgerObservationProducer::Whisper
                    && label == "partial words and the recovered remainder"
            )));
        assert!(
            !emitted
                .iter()
                .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
        );
        assert!(requests.try_recv().is_err());
    }
}

#[test]
fn whisper_segmentless_text_requires_exact_window_and_rejected_pins_never_fallback() {
    for context in ["utterance-only", "symmetric-pad"] {
        for defect in [
            "none", "word", "session", "epoch", "outside", "empty", "reversed",
        ] {
            let (mut state, events, mut receiver, mut requests) = fixture(1);
            forensic_live_transport_capture(&mut state);
            reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
            assert_eq!(state.windows_admitted, 0);
            assert!(state.refinement_pending.is_empty());
            pump_stopped(&mut state, &events);
            let request = requests.try_recv().unwrap();
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let occurrence = &request.member_occurrences[0].1;
            let mut completion = labelled_completion(&request);
            let payload = completion.payload.as_mut().unwrap();
            match defect {
                "none" => payload.segments.clear(),
                "word" => {
                    payload.segments[0].grain = crate::stt::tail_provider::TailSegmentGrain::Word;
                    payload.segments[0].range.sample_start = 50;
                    payload.segments[0].range.sample_end = 350;
                    payload.evidence.segment_grain =
                        crate::stt::tail_provider::TailSegmentGrain::Word;
                    payload.evidence.timing_quality =
                        crate::stt::tail_provider::TailTimingQuality::ExactSampleRange;
                }
                "session" => payload.segments[0].range.session = "foreign".into(),
                "epoch" => payload.segments[0].range.capture_epoch += 1,
                "outside" => payload.segments[0].range.sample_end += 1,
                "empty" => payload.segments[0].range.sample_end = 0,
                "reversed" => payload.segments[0].range.sample_start = 401,
                _ => unreachable!(),
            }
            while receiver.try_recv().is_ok() {}
            state.complete_whisper_window(&events, completion, 20.0);
            state.close_admission_horizon(&events, occurrence.sample_end);
            let accepted = defect == "word";
            let decoded_words = defect == "word";
            assert_eq!(
                OccurrenceIdentity::from(&request.provider_request.identity.range),
                *occurrence
            );
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.text_of(occurrence),
                accepted.then_some("hello"),
                "{context:?}/{defect}"
            );
            assert_eq!(ledger.is_sealed(occurrence), decoded_words);
            assert_eq!(ledger.text_recovery_pending(occurrence), !decoded_words);
            assert_eq!(ledger.conservation().residue(), 0);
            assert_eq!(
                ledger
                    .frontier_of(occurrence)
                    .unwrap()
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper),
                !decoded_words,
                "{context:?}/{defect}"
            );
            drop(ledger);
            assert_eq!(state.tail_patch_awaiting_completion(), 0);
            assert!(state.refinement_submitted.is_empty());
            let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
            assert_eq!(
                emitted
                    .iter()
                    .filter(|event| matches!(event,
                        EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
                    ))
                    .count(),
                usize::from(decoded_words)
            );
            assert_eq!(
                emitted
                    .iter()
                    .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                    .count(),
                usize::from(decoded_words)
            );
            // A corrected payload on a consumed request is still a replay.
            state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
            assert!(receiver.try_recv().is_err());
            assert_eq!(
                state.acoustic_ledger.lock().unwrap().text_of(occurrence),
                accepted.then_some("hello")
            );
        }
    }
}

#[test]
fn whisper_foreign_envelopes_preserve_the_submitted_job_until_exact_completion() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    assert_eq!(state.windows_admitted, 0);
    pump_stopped(&mut state, &events);
    assert!(state.refinement_pending.is_empty());
    let request = requests.try_recv().unwrap();
    let occurrence = &request.member_occurrences[0].1;
    while receiver.try_recv().is_ok() {}
    for defect in [
        "request",
        "session",
        "epoch",
        "start",
        "end",
        "member",
        "member_span",
        "members",
        "utterance",
        "missing",
    ] {
        let mut completion = labelled_completion(&request);
        let identity = completion.request_identity.as_mut().unwrap();
        match defect {
            "request" => identity.request_id += 1,
            "session" => identity.range.session = "foreign".into(),
            "epoch" => identity.range.capture_epoch += 1,
            "start" => identity.range.sample_start += 1,
            "end" => identity.range.sample_end += 1,
            "member" => completion.member_occurrences[0].0 += 1,
            "member_span" => completion.member_occurrences[0].1.sample_end += 1,
            "members" => completion.member_occurrences.clear(),
            "utterance" => completion.utterance_id += 1,
            "missing" => completion.request_identity = None,
            _ => unreachable!(),
        }
        state.complete_whisper_window(&events, completion, 20.0);
        assert_eq!(state.tail_patch_awaiting_completion(), 1, "{defect}");
        assert_eq!(state.refinement_submitted.len(), 1);
        assert!(state.pending_events.contains_key(&request.utterance_id));
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(occurrence), None);
        assert!(!ledger.is_sealed(occurrence));
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        drop(ledger);
        assert!(receiver.try_recv().is_err());
    }
    state.complete_whisper_window(
        &events,
        forensic_live_transport_word_completion(&request),
        20.0,
    );
    state.close_admission_horizon(&events, occurrence.sample_end);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(occurrence),
        Some("hello")
    );
    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(occurrence));
    assert!(
        !state
            .acoustic_ledger
            .lock()
            .unwrap()
            .text_recovery_pending(occurrence)
    );
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .conservation()
            .residue(),
        0
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(state.refinement_submitted.is_empty());
    let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
            ))
            .count(),
        1
    );
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        1
    );
    state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn whisper_unsolicited_completion_cannot_consume_a_held_occurrence() {
    let (mut source, source_events, _source_receiver, mut source_requests) = fixture(1);
    forensic_live_transport_capture(&mut source);
    reconcile_silero_ledger(&mut source, &source_events, &closed(1), &[]);
    pump_stopped(&mut source, &source_events);
    assert_eq!(source.windows_admitted, 1);
    assert!(source.refinement_pending.is_empty());
    let unsolicited = source_requests.try_recv().unwrap();
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(
        &mut state,
        &events,
        &closed(1),
        &[TranscriptSegment {
            confidence: None,
            text: "hello".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    while receiver.try_recv().is_ok() {}
    state.complete_whisper_window(&events, labelled_completion(&unsolicited), 20.0);
    assert!(receiver.try_recv().is_err());
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(state.refinement_submitted.is_empty());
    assert_eq!(state.windows_admitted, 0);
    assert!(state.refinement_pending.is_empty());
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&occurrence), Some("hello"));
    assert!(!ledger.is_sealed(&occurrence));
    assert!(
        ledger
            .frontier_of(&occurrence)
            .unwrap()
            .open_producers()
            .contains(&LedgerObservationProducer::Whisper)
    );
    drop(ledger);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    state.complete_whisper_window(
        &events,
        forensic_live_transport_word_completion(&request),
        20.0,
    );
    state.close_admission_horizon(&events, occurrence.sample_end);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&occurrence),
        Some("hello")
    );
    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
}

#[test]
fn whisper_foreign_payload_returns_only_the_owned_frontier_without_text() {
    for defect in ["request", "session", "epoch", "start", "end"] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        forensic_live_transport_capture(&mut state);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        pump_stopped(&mut state, &events);
        assert!(state.refinement_pending.is_empty());
        let request = requests.try_recv().unwrap();
        let occurrence = &request.member_occurrences[0].1;
        let mut completion = labelled_completion(&request);
        let identity = &mut completion.payload.as_mut().unwrap().identity;
        match defect {
            "request" => identity.request_id += 1,
            "session" => identity.range.session = "foreign".into(),
            "epoch" => identity.range.capture_epoch += 1,
            "start" => identity.range.sample_start += 1,
            "end" => identity.range.sample_end += 1,
            _ => unreachable!(),
        }
        while receiver.try_recv().is_ok() {}
        state.complete_whisper_window(&events, completion, 20.0);
        state.close_admission_horizon(&events, occurrence.sample_end);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(occurrence), None, "{defect}");
        assert!(!ledger.is_sealed(occurrence));
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper),
            "{defect}"
        );
        drop(ledger);
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        assert!(state.refinement_submitted.is_empty());
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(!emitted.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
        )));
        assert!(
            !emitted
                .iter()
                .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
        );
        assert!(emitted.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE
        )));
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn shared_decode_window_has_no_whitespace_labels_for_unresolved_owners() {
    for label in ["", "Iwo"] {
        let (mut state, events, _receiver, mut requests) = fixture(3);
        CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE)
            .push_samples(&vec![0.25; 20_000]);
        reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
        state.pending_events.get_mut(&2).unwrap().layer1_baseline = label.into();
        state.pump_capture_windows(&events);
        let request = requests
            .try_recv()
            .expect("one live window for three owners");
        assert_eq!(request.member_occurrences.len(), 3);
        assert_eq!(
            request.committed_text, label,
            "unresolved labels must not become padding around the real baseline"
        );
        assert!(!state.capture_stopping, "this is the live path before Stop");
    }
}

/// Fix d0670e5ac; kills M3b-08 (empty labels become synthetic separators).
#[test]
fn m3b_shared_decode_joins_only_literal_nonempty_labels_in_owner_order() {
    for (labels, expected) in [
        (["", "", ""], ""),
        (["Ala", "", "kot"], "Ala kot"),
        (["", "Iwo", ""], "Iwo"),
    ] {
        let (mut state, events, _receiver, mut requests) = fixture(3);
        CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE)
            .push_samples(&vec![0.25; 20_000]);
        reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
        for (id, label) in (1..=3).zip(labels) {
            state.pending_events.get_mut(&id).unwrap().layer1_baseline = label.into();
        }
        state.pump_capture_windows(&events);
        let request = requests
            .try_recv()
            .expect("live decode request for all three owners");
        assert_eq!(request.member_occurrences.len(), 3);
        assert_eq!(request.committed_text, expected);
        assert!(!state.capture_stopping);
    }
}

#[test]
fn closed_without_next_apple_piece_submits_immediately_in_both_capture_intents() {
    // The production tick has no intent input: neither SingleTurn nor
    // hands-free lifecycle may veto acoustic readiness.
    for silence in [None, Some(2.0)] {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        let _epoch = EpochGate::for_session(RATE, silence, true);
        let now = state.refinement_clock;
        forensic_live_transport_capture(&mut state);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        pump_stopped(&mut state, &events);
        let request = requests
            .try_recv()
            .expect("speech debt submits at close, without another Apple piece or tick");
        state.tick_refinements(&events, now + Duration::from_millis(1_199));
        assert!(requests.try_recv().is_err());
        state.audio.push(&[0.0; 40]);
        state.tick_refinements(&events, now + Duration::from_millis(1_200));
        assert!(requests.try_recv().is_err());
        assert_eq!(
            request.member_occurrences,
            vec![(1, OccurrenceIdentity::new("live-admission", 7, 0, 400))]
        );
        request
            .provider_request
            .validate_pcm(&request.audio)
            .expect("exact PCM");
        assert!(
            request.committed_text.is_empty(),
            "never manufacture Apple words"
        );
        for quantum in 1..10 {
            state.tick_refinements(
                &events,
                now + Duration::from_millis(1_200) + LIVE_WORKER_QUANTUM * quantum,
            );
        }
        assert!(
            requests.try_recv().is_err(),
            "idle ticks cannot replay admission"
        );
        assert_eq!(state.refinement_submitted.len(), 1);
    }
}

#[test]
fn blank_then_nonblank_apple_evidence_keeps_one_refinement_owner() {
    let (mut state, events, mut receiver, mut requests) = fixture(2);
    forensic_live_transport_capture(&mut state);
    let ledger = closed(1);
    reconcile_silero_ledger(
        &mut state,
        &events,
        &ledger,
        &[TranscriptSegment {
            confidence: None,
            text: " ".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    assert!(!state.reconciled_silero.contains(&1));
    reconcile_silero_ledger(
        &mut state,
        &events,
        &ledger,
        &[TranscriptSegment {
            confidence: None,
            text: "hello".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    assert!(state.reconciled_silero.contains(&1));
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().expect("one physical job");
    request
        .provider_request
        .validate_pcm(&request.audio)
        .expect("actual decoder PCM");
    assert_eq!(request.audio, vec![0.25; 400]);
    assert!(requests.try_recv().is_err());
    assert_eq!(request.member_occurrences.len(), 1);
    let occurrence = &request.member_occurrences[0].1;
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(occurrence),
        Some("hello")
    );
    state.complete_whisper_window(
        &events,
        forensic_live_transport_word_completion(&request),
        20.0,
    );
    state.close_admission_horizon(&events, occurrence.sample_end);
    assert!(state.acoustic_ledger.lock().unwrap().is_sealed(occurrence));
    let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::LedgerMutation { label, receipt: MutationReceipt::Insert { .. }, .. }
                    if label == "hello"
            ))
            .count(),
        1
    );
    assert!(!emitted.iter().any(|event| matches!(event,
        EngineEvent::LedgerMutation { label, receipt, .. }
            if label.trim().is_empty() && receipt.grants_mutation()
    )));
}

#[test]
fn apple_word_slots_are_pinned_before_the_admission_is_published() {
    let (mut state, events, mut receiver, _requests) = fixture(1);
    assert!(reconcile_silero_ledger(
        &mut state,
        &events,
        &closed(1),
        &[
            TranscriptSegment {
                confidence: None,
                text: "hello".into(),
                start_ts: 0.0,
                end_ts: 0.1
            },
            TranscriptSegment {
                confidence: None,
                text: "again".into(),
                start_ts: 0.2,
                end_ts: 0.4
            },
        ]
    ));
    let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 400);
    let ledger = state.acoustic_ledger.lock().unwrap();
    ledger.assert_slot_labels();
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 2);
    assert_eq!((slots[0].sample_start, slots[0].sample_end), (0, 100));
    assert_eq!((slots[1].sample_start, slots[1].sample_end), (200, 400));
    let tokens = ledger.compose(&occurrence).unwrap().tokens;
    assert_eq!(tokens[0].token_sample_end, Some(100));
    assert_eq!(tokens[1].token_sample_start, Some(200));
    assert!(
        std::iter::from_fn(|| receiver.try_recv().ok()).any(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Insert { .. }, .. }
                if observation.producer == LedgerObservationProducer::Apple
        ))
    );
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn whisper_first_completion_admits_owned_label_without_apple_words() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let occurrence = request.member_occurrences[0].1.clone();
    assert_eq!(occurrence.sample_end, 400);
    assert_eq!(request.provider_request.identity.range.sample_end, 400);
    request
        .provider_request
        .validate_pcm(&request.audio)
        .unwrap();
    let completion = forensic_live_transport_word_completion(&request);
    state.complete_whisper_window(&events, completion, 20.0);
    state.close_admission_horizon(&events, 400);
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&occurrence), Some("hello"));
    assert!(ledger.is_sealed(&occurrence));
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!((slots[0].sample_start, slots[0].sample_end), (50, 350));
    assert!(!ledger.text_recovery_pending(&occurrence));
    assert_eq!(state.tail_patch_jobs_applied, 1);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    drop(ledger);
    let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!emitted.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == RefinementFailure::NoLabel.code())));
    assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { .. } }
                if observation.producer == LedgerObservationProducer::Whisper && label == "hello"
        )).count(), 1);
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::UtteranceFinal { text, .. } if text == "hello"
            ))
            .count(),
        1
    );
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        1
    );

    // Independently executed real-reconcile controls retain the original
    // successful first-Whisper, once-only publication and late-Apple intent,
    // now with measured PCM and authentic successful word-scope evidence.
    for adaptive in [false, true] {
        super::relay_l1_overlap_admission_tests::first_whisper_measured_capture(
            adaptive, true, false,
        );
    }
}

#[test]
fn apple_first_emits_once_only_reducer_receipts_before_stop() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    let words = [TranscriptSegment {
        confidence: None,
        text: "hello".into(),
        start_ts: 0.0,
        end_ts: 0.4,
    }];
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &words);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &words);
    let mut emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        !emitted
            .iter()
            .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
    );
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    state.complete_whisper_window(&events, finish(&request), 20.0);
    state.complete_whisper_window(&events, finish(&request), 20.0);
    state.close_admission_horizon(&events, 400);
    emitted.extend(std::iter::from_fn(|| receiver.try_recv().ok()));
    assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { .. } }
                if observation.producer == LedgerObservationProducer::Apple && label == "hello"
        )).count(), 1);
    assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Preserve { .. }, .. }
                if observation.producer == LedgerObservationProducer::Lexicon
        )).count(), 1);
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        1
    );
    assert_eq!(state.sealed_count, 1);
    assert_eq!(state.tail_patch_jobs_skipped, 1);
    assert!(
        !state
            .acoustic_ledger
            .lock()
            .unwrap()
            .text_recovery_pending(&request.member_occurrences[0].1)
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(requests.try_recv().is_err());
}

/// Observer completion without a label leaves the physical occurrence
/// recoverable. Authored contract, deliberately unrun under W2.
#[test]
fn late_nonblank_apple_after_no_label_completion_remains_recoverable() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    let physical = closed(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &physical, &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let occurrence = request.member_occurrences[0].1.clone();
    let serial = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .serial_of(&occurrence)
        .unwrap()
        .clone();
    state.complete_whisper_window(&events, finish(&request), 20.0);
    state.complete_whisper_window(&events, finish(&request), 20.0);
    state.close_admission_horizon(&events, occurrence.sample_end);
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.seal(&occurrence),
            Err(SealRefusal::TextRecoveryPending)
        );
        assert!(
            ledger
                .frontier_of(&occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        assert_eq!(ledger.layer_trail_for(&occurrence).count(), 0);
    }
    assert_eq!(state.sealed_count, 0);
    reconcile_silero_ledger(
        &mut state,
        &events,
        &physical,
        &[TranscriptSegment {
            confidence: None,
            text: "late real words".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }],
    );
    {
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.text_of(&occurrence),
            Some("late real words"),
            "a failed refinement must not consume the only later real label"
        );
        assert_eq!(ledger.serial_of(&occurrence), Some(&serial));
        assert_eq!(
            ledger.qualified_occurrences().cloned().collect::<Vec<_>>(),
            vec![occurrence.clone()]
        );
        assert!(ledger.seal_of(&occurrence).is_none());
        assert!(ledger.text_recovery_pending(&occurrence));
        assert_eq!(ledger.layer_trail_for(&occurrence).count(), 2);
        assert!(
            ledger
                .frontier_of(&occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
    }
    let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { occurrence: held } }
                if observation.producer == LedgerObservationProducer::Apple
                    && observation.occurrence == occurrence && *held == occurrence
                    && label == "late real words"
        )).count(), 1);
    assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Preserve { .. }, .. }
                if observation.producer == LedgerObservationProducer::Lexicon
        )).count(), 1);
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            .count(),
        0
    );
    assert_eq!(
        emitted
            .iter()
            .filter(|event| matches!(event,
                EngineEvent::UtteranceFinal { text, .. } if text == "late real words"
            ))
            .count(),
        0
    );
    let later = state.refinement_clock + Duration::from_secs(3);
    state.tick_refinements(&events, later);
    assert!(
        requests.try_recv().is_err(),
        "same occurrence never decodes twice"
    );
    assert_eq!(state.tail_patch_jobs_skipped, 1);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(
        state.window_by_samples(0, 400).unwrap().samples,
        vec![0.25; 400]
    );
}

#[test]
fn late_apple_with_foreign_capture_cannot_qualify_or_replace_current_occurrence() {
    for (session, epoch) in [("other", 7), ("live-admission", 6)] {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        forensic_live_transport_capture(&mut state);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        pump_stopped(&mut state, &events);
        let request = requests.try_recv().unwrap();
        state.complete_whisper_window(&events, finish(&request), 20.0);
        let mut stale = UtteranceLedger::new();
        stale.open_or_extend(session, epoch, 0, 400);
        stale.close_open(400);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &stale,
            &[TranscriptSegment {
                confidence: None,
                text: "foreign".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.qualified_occurrences().cloned().collect::<Vec<_>>(),
            vec![request.member_occurrences[0].1.clone()]
        );
        assert!(ledger.is_empty());
        drop(ledger);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(1),
            &[TranscriptSegment {
                confidence: None,
                text: "current".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&request.member_occurrences[0].1),
            Some("current")
        );
        assert_eq!(
            state.window_by_samples(0, 400).unwrap().samples,
            vec![0.25; 400]
        );
    }
}

#[test]
fn oversized_owned_pcm_is_split_before_the_pending_cap() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    state.audio.push(&vec![0.25; 20_000]);
    let mut ledger = UtteranceLedger::new();
    ledger.open_or_extend("live-admission", 7, 0, 40_000);
    ledger.close_open(40_000);
    reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
    assert_eq!(state.windows_admitted, 0);
    assert!(state.refinement_pending.is_empty());
    state.pump_capture_windows(&events);
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(state.refinement_pending.len(), 1);
    let request = requests.try_recv().expect("one window fits the channel");
    assert_eq!(request.audio.len(), 9_000);
    state.pump_capture_windows(&events);
    assert!(
        state.windows_admitted >= 2,
        "40 s at 1 kHz is split into 9 s windows, admitted {}",
        state.windows_admitted
    );
    assert!(
        state.refinement_pending.len() <= 1,
        "a full channel retains one exact offer"
    );
    let backlog = warnings(&mut receiver, RefinementFailure::PcmUnavailable.code());
    assert_eq!(backlog, 0);
    assert!(
        state.windows_admitted >= 2,
        "the fragment is not one whole-fragment refusal"
    );
    assert_eq!(state.audio.session_sample_end(), 40_000);
}

#[test]
fn saturation_then_drain_preserves_exact_requests_and_once_only_submission() {
    let (mut state, events, _receiver, mut requests) = fixture(1);
    reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
    state.pump_capture_windows(&events);
    assert_eq!(state.refinement_pending.len(), 1);
    assert_eq!(state.windows_admitted, 1);
    let mut starts = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    for _ in 0..4 {
        let request = match requests.try_recv() {
            Ok(request) => request,
            Err(_) => {
                state.pump_capture_windows(&events);
                requests.try_recv().expect("next exact request")
            }
        };
        assert!(sequences.insert(request.submission_sequence));
        assert!(starts.insert(request.provider_request.identity.range.sample_start));
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        let completion = finish(&request);
        state.complete_whisper_window(&events, completion, 20.0);
        state.pump_capture_windows(&events);
    }
    for id in 0..3 {
        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(3),
            &[TranscriptSegment {
                confidence: None,
                text: "recovered".into(),
                start_ts: id as f32,
                end_ts: id as f32 + 0.4,
            }],
        );
        let occurrence = OccurrenceIdentity::new("live-admission", 7, id * 1_000, id * 1_000 + 400);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some("recovered"));
        assert!(!ledger.is_sealed(&occurrence));
        assert!(ledger.text_recovery_pending(&occurrence));
        assert_eq!(ledger.layer_trail_for(&occurrence).count(), 2);
        assert!(
            ledger
                .layer_trail_for(&occurrence)
                .all(|entry| entry.producer() != LedgerObservationProducer::Whisper),
            "failed work must not masquerade as an acoustic observation"
        );
    }
    assert!(requests.try_recv().is_err());
    assert_eq!(starts, BTreeSet::from([0, 3_000, 6_000, 9_000]));
    assert_eq!(sequences.len(), 4);
    assert!(state.refinement_pending.is_empty());
    assert!(state.refinement_submitted.is_empty());
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
}

#[test]
fn permanent_lane_loss_reports_every_pending_occurrence_once() {
    let (mut state, events, mut receiver, requests) = fixture(1);
    drop(requests);
    reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
    state.pump_capture_windows(&events);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::LaneGone.code()),
        3
    );
    state.retry_refinements(&events);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::LaneGone.code()),
        0
    );
    state.close_admission_horizon(&events, 2_400);
    for id in 0..3 {
        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(3),
            &[TranscriptSegment {
                confidence: None,
                text: "recovered".into(),
                start_ts: id as f32,
                end_ts: id as f32 + 0.4,
            }],
        );
        let occurrence = OccurrenceIdentity::new("live-admission", 7, id * 1_000, id * 1_000 + 400);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some("recovered"));
        assert!(!ledger.is_sealed(&occurrence));
        assert!(ledger.text_recovery_pending(&occurrence));
        assert_eq!(ledger.layer_trail_for(&occurrence).count(), 3);
    }
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::LaneGone.code()),
        0
    );
    assert!(state.refinement_pending.is_empty());
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
}

#[test]
fn retention_exhaustion_is_explicit_and_keeps_capture_pcm() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    state.audio = LiveAudioBuffer::new(RATE, 1.0);
    state.audio.push(&vec![0.25; 2_000]);
    let ledger = closed(1);
    reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::PcmUnavailable.code()),
        1
    );
    reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::PcmUnavailable.code()),
        0
    );
    assert!(requests.try_recv().is_err());
    assert_eq!(state.audio.session_sample_end(), 2_000);
    assert_eq!(
        state.window_by_samples(1_000, 2_000).unwrap().samples,
        vec![0.25; 1_000]
    );
}

#[test]
fn backlog_exhaustion_conserves_submitted_pending_and_failed_members() {
    let (mut state, events, mut receiver, _requests) = fixture(1);
    reconcile_silero_ledger(&mut state, &events, &closed(12), &[]);
    state.pump_capture_windows(&events);
    assert_eq!(state.refinement_submitted.len(), 1);
    assert_eq!(state.refinement_pending.len(), 1);
    assert_eq!(state.windows_admitted, 1);
    assert_eq!(state.audio.session_sample_end(), 20_000);
    let held = state.refinement_pending.front().unwrap();
    assert_eq!(
        (
            held.provider_request.identity.range.sample_start,
            held.provider_request.identity.range.sample_end
        ),
        (3_000, 12_000)
    );
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::PcmUnavailable.code()),
        0
    );
    assert_eq!(state.unsealed_word_owners().len(), 12);
}

// One physical owner, qualified on the 48 kHz capture clock.
/// Qualify the member from the PCM the capture holds under it.
fn tc3_stage_member(state: &mut AppleSealState, occurrence: &OccurrenceIdentity) {
    let calibration = state.energy_calibration.clone().unwrap();
    let window = state
        .window_by_samples(occurrence.sample_start, occurrence.sample_end)
        .unwrap();
    let energy_integral = window
        .samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    let peak = window
        .samples
        .iter()
        .fold(0.0_f32, |peak, x| peak.max(x.abs()));
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        ledger
            .qualify(
                &AcousticEvidence {
                    occurrence: occurrence.clone(),
                    duration_ms: occurrence.sample_len() as f64 / 48.0,
                    energy_integral,
                    mean_rms_dbfs: 10.0
                        * (energy_integral / occurrence.sample_len() as f64).log10(),
                    peak_dbfs: 20.0 * f64::from(peak).log10(),
                    vad_open_sample: Some(occurrence.sample_start),
                    vad_close_sample: Some(occurrence.sample_end),
                    evidence_calibration_version: calibration.version.clone(),
                },
                &calibration,
            )
            .is_qualified()
    );
    ledger.schedule_frontier(occurrence.clone(), vec![LedgerObservationProducer::Whisper]);
    drop(ledger);
    state.pending_events.insert(
        10,
        PendingAppleSeal {
            occurrence: occurrence.clone(),
            raw_text: String::new(),
            layer1_baseline: String::new(),
            start_ts: occurrence.sample_start as f32 / 48_000.0,
            end_ts: occurrence.sample_end as f32 / 48_000.0,
            segments: Vec::new(),
        },
    );
}

/// Three distinct 9 s windows on a stopped 15 s head. Request ids are the
/// capture-grid index, and the grid never re-offers a spent range.
fn tc3_duplicate_geometry(
    order: [usize; 3],
) -> (
    AppleSealState,
    mpsc::UnboundedSender<EngineEvent>,
    mpsc::UnboundedReceiver<EngineEvent>,
) {
    let (events, receiver) = mpsc::unbounded_channel();
    let (sender, mut requests) = mpsc::channel(3);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        48_000,
        "ef1fa240".into(),
        7,
        sender,
        Arc::new(Mutex::new(AcousticLedger::new())),
        Some(EnergyCalibration {
            version: "tc3-owned-pcm".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        }),
    );
    let head = 720_000_usize;
    // One 0.4 s spoken burst under the returned word, as in
    // `forensic_live_transport_capture`, then room tone below the floor:
    // speech that ran on through every window would leave the word at the
    // capture origin possibly cut, and the ledger would rightly wait.
    let mut pcm = vec![0.004; head];
    pcm[..19_200].fill(0.25);
    state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
    state.audio.push(&pcm);
    CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, 48_000).push_samples(&pcm);
    state.capture_stopping = true;
    let owner = OccurrenceIdentity::new("ef1fa240", 7, 0, head as u64);
    tc3_stage_member(&mut state, &owner);
    state.pump_capture_windows(&events);
    let mut submitted = Vec::new();
    for _ in 0..3 {
        submitted.push(requests.try_recv().expect("accepted window"));
    }
    assert!(requests.try_recv().is_err());
    assert!(state.window_plan.is_finished());
    assert_eq!(
        submitted
            .iter()
            .map(|request| request.provider_request.identity.request_id)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_ne!(
        submitted[0].provider_request.identity,
        submitted[2].provider_request.identity
    );
    assert!(submitted[0].submission_sequence < submitted[1].submission_sequence);
    assert!(submitted[1].submission_sequence < submitted[2].submission_sequence);
    assert_eq!(state.refinement_submitted.len(), 3);
    assert_eq!(state.tail_patch_awaiting_completion(), 3);
    for (finished, index) in order.into_iter().enumerate() {
        let completion = if index == 0 {
            forensic_live_transport_word_completion(&submitted[index])
        } else {
            finish(&submitted[index])
        };
        let logged = refinement_log(|| {
            state.complete_whisper_window(&events, completion, 50.0);
        });
        assert!(!logged.contains("unmatched_completion"), "{logged}");
        assert_eq!(
            state.tail_patch_awaiting_completion(),
            (2 - finished) as u64
        );
        assert_eq!(state.refinement_submitted.len(), 2 - finished);
        assert_eq!(
            state.tail_patch_jobs_applied + state.tail_patch_jobs_skipped,
            (finished + 1) as u64,
            "every matched decode must reach a terminal bucket"
        );
    }
    assert!(state.refinement_submitted.is_empty());
    assert!(state.refinement_pending.is_empty());
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&owner),
        Some("hello")
    );
    let receipt = tail_patch_receipt_after_stop(
        true,
        3,
        Some(TailPatchWorkerAccounting {
            applied_jobs: state.tail_patch_jobs_applied,
            skipped_jobs: state.tail_patch_jobs_skipped,
            timeout_residue: state.tail_patch_awaiting_completion(),
        }),
        SessionConservationReceipt::default(),
    );
    assert_eq!(receipt.timed_out, 0);
    assert_eq!(receipt.abandoned, 0);
    assert_eq!(receipt.drain, TailPatchDrainDisposition::Completed);
    (state, events, receiver)
}

#[test]
fn tc3_duplicate_identity_completes_in_submission_order() {
    tc3_duplicate_geometry([0, 1, 2]);
}

#[test]
fn tc3_duplicate_identity_completes_out_of_order() {
    tc3_duplicate_geometry([2, 0, 1]);
}

#[test]
fn tc3_empty_stop_drain_returns_on_first_tick() {
    let (mut state, events, mut receiver) = tc3_duplicate_geometry([0, 1, 2]);
    let now = Instant::now();
    let logged = refinement_log(|| {
        assert!(!state.stop_refinements_tick(&events, now, now + Duration::from_secs(5)));
        assert!(!state.stop_refinements_tick(&events, now, now));
    });
    assert!(
        !logged.contains("live_refinement_stop_deadline"),
        "{logged}"
    );
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
        0
    );
}

#[test]
fn local_power_stop_deadline_preserves_clock_residue_and_disposition() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let occurrence = request.member_occurrences[0].1.clone();
    assert!(state.cloud_commit_tx.is_none());
    assert!(state.cloud_uncommitted.is_empty());
    assert!(state.cloud_inflight.is_empty());
    let deadline = state.refinement_started + Duration::from_secs(5);
    let outstanding = state.tail_patch_awaiting_completion();
    assert_eq!(outstanding, 1);
    let mut residue = 0;
    let logged = refinement_log(|| {
        if !state.stop_refinements_tick(&events, deadline, deadline) {
            residue = outstanding;
        }
    });
    assert_eq!(state.refinement_clock, deadline);
    assert_eq!(residue, outstanding);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    let dispositions = logged
        .lines()
        .filter(|line| line.contains("disposition=live_refinement_stop_deadline"))
        .collect::<Vec<_>>();
    assert_eq!(dispositions.len(), 1, "{logged}");
    let disposition = dispositions[0];
    assert!(disposition.contains("elapsed_ms=5000 "), "{logged}");
    assert!(disposition.contains("session=live-admission "), "{logged}");
    assert!(disposition.contains("capture_epoch=7 "), "{logged}");
    assert!(
        disposition.contains(&format!("sample_start={} ", occurrence.sample_start)),
        "{logged}"
    );
    assert!(
        disposition.contains(&format!("sample_end={} ", occurrence.sample_end)),
        "{logged}"
    );
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
        1
    );
    let receipt = tail_patch_receipt_after_stop(
        true,
        1,
        Some(TailPatchWorkerAccounting {
            applied_jobs: state.tail_patch_jobs_applied,
            skipped_jobs: state.tail_patch_jobs_skipped,
            timeout_residue: residue,
        }),
        SessionConservationReceipt::default(),
    );
    assert_eq!(receipt.applied, 0);
    assert_eq!(receipt.skipped, 0);
    assert_eq!(receipt.timed_out, 1);
    assert_eq!(receipt.abandoned, 0);
    assert_eq!(receipt.drain, TailPatchDrainDisposition::TimedOut);
}

#[test]
fn tc3_stop_admits_real_inflight_job_before_deadline() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let occurrence = request.member_occurrences[0].1.clone();
    let now = Instant::now();
    let deadline = now + Duration::from_secs(5);
    assert!(state.stop_refinements_tick(&events, now, deadline));
    state.complete_whisper_window(
        &events,
        forensic_live_transport_word_completion(&request),
        20.0,
    );
    let outstanding = state.tail_patch_awaiting_completion();
    let mut residue = 0;
    let waiting = state.stop_refinements_tick(&events, now + LIVE_WORKER_QUANTUM, deadline);
    if !waiting {
        residue = outstanding;
    }
    assert!(!waiting);
    assert_eq!(residue, 0);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().text_of(&occurrence),
        Some("hello")
    );
    assert_eq!(
        warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
        0
    );
    let receipt = tail_patch_receipt_after_stop(
        true,
        1,
        Some(TailPatchWorkerAccounting {
            applied_jobs: state.tail_patch_jobs_applied,
            skipped_jobs: state.tail_patch_jobs_skipped,
            timeout_residue: residue,
        }),
        SessionConservationReceipt::default(),
    );
    assert_eq!(receipt.applied, 1);
    assert_eq!(receipt.timed_out, 0);
    assert_eq!(receipt.drain, TailPatchDrainDisposition::Completed);
}

#[test]
fn tc3_unmatched_completion_reports_without_consuming_work() {
    let (mut state, events, _receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    for defect in ["submission", "request", "missing_identity"] {
        let mut completion = labelled_completion(&request);
        match defect {
            "submission" => completion.submission_sequence += 1,
            "request" => completion.request_identity.as_mut().unwrap().request_id += 100,
            _ => completion.request_identity = None,
        }
        let logged = refinement_log(|| {
            state.complete_whisper_window(&events, completion, 20.0);
        });
        assert!(
            logged.contains("unmatched_completion"),
            "{defect}: {logged}"
        );
        for field in [
            "submission_sequence=",
            "request_id=",
            "sample_start=",
            "sample_end=",
        ] {
            assert!(logged.contains(field), "{defect}: {logged}");
        }
        assert_eq!(state.tail_patch_awaiting_completion(), 1);
        assert_eq!(state.refinement_submitted.len(), 1);
        assert_eq!(
            state.tail_patch_jobs_applied + state.tail_patch_jobs_skipped,
            0
        );
    }
    state.complete_whisper_window(
        &events,
        forensic_live_transport_word_completion(&request),
        20.0,
    );
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.tail_patch_jobs_applied, 1);
}

#[test]
fn tc3_async_completion_echoes_submission() {
    for succeeds in [false, true] {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        forensic_live_transport_capture(&mut state);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        pump_stopped(&mut state, &events);
        let request = requests.try_recv().unwrap();
        let inflight = TailPatchInFlight {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: request.provider_request.identity.clone(),
            admit_sample_start: request.admit_sample_start,
            admit_sample_end: request.admit_sample_end,
            member_occurrences: request.member_occurrences.clone(),
        };
        let result = if succeeds {
            Ok(TailPatchJobResult {
                utterance_id: request.utterance_id,
                outcome: TailPatchOutcome::NoChange,
                payload: forensic_live_transport_word_completion(&request)
                    .payload
                    .unwrap(),
            })
        } else {
            Err(anyhow::anyhow!("synthetic provider failure"))
        };
        let mut lane =
            AppleTailPatchLane::new(RATE, None, crate::stt::tail_provider::TailProviderId::Fake);
        let completion = lane.finish_for_worker(Some(inflight), result);
        let (sender, receiver) = std_mpsc::channel();
        assert!(lane.forward_completion_to_worker(&sender, completion));
        let returned = receiver.try_recv().unwrap();
        assert_eq!(returned.submission_sequence, request.submission_sequence);
        assert_eq!(
            returned.request_identity,
            Some(request.provider_request.identity)
        );
        assert_eq!(returned.member_occurrences, request.member_occurrences);
        assert_eq!(returned.payload.is_some(), succeeds);
        state.complete_whisper_window(&events, returned, 20.0);
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        assert_eq!(state.tail_patch_jobs_applied, u64::from(succeeds));
        assert_eq!(state.tail_patch_jobs_skipped, u64::from(!succeeds));
    }
}

#[test]
fn stale_completion_cannot_return_the_current_observer() {
    let (mut state, events, _receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().unwrap();
    let mut stale = finish(&request);
    stale.request_identity.as_mut().unwrap().range.capture_epoch += 1;
    state.complete_whisper_window(&events, stale, 20.0);
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    assert_eq!(state.refinement_submitted.len(), 1);
    state.complete_whisper_window(&events, finish(&request), 20.0);
    state.complete_whisper_window(&events, finish(&request), 20.0);
    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert_eq!(state.tail_patch_jobs_skipped, 1);
}

/// A submitted job whose members close before its completion returns
/// (invalid identity, no label, seal, or stop-path text-debt recovery)
/// must still occupy exactly one terminal bucket and must not mutate.
#[test]
fn completion_after_closed_members_is_one_skipped_job() {
    let (mut state, events, mut receiver, mut requests) = fixture(1);
    forensic_live_transport_capture(&mut state);
    reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
    pump_stopped(&mut state, &events);
    let request = requests.try_recv().expect("one submitted window");
    assert_eq!(state.tail_patch_awaiting_completion(), 1);
    let (member_id, occurrence) = request.member_occurrences[0].clone();
    state.fail_refinement(
        &events,
        member_id,
        &occurrence,
        RefinementFailure::StopDeadline,
    );
    let text_after_close = state
        .acoustic_ledger
        .lock()
        .expect("ledger")
        .text_of(&occurrence)
        .map(str::to_owned);
    let sealed_after_close = state
        .acoustic_ledger
        .lock()
        .expect("ledger")
        .is_sealed(&occurrence);
    while receiver.try_recv().is_ok() {}

    let logged = refinement_log(|| {
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
    });

    assert_eq!(state.tail_patch_awaiting_completion(), 0);
    assert!(state.refinement_submitted.is_empty());
    assert_eq!(state.tail_patch_jobs_applied, 0);
    assert_eq!(
        state.tail_patch_jobs_skipped, 1,
        "a completion whose members closed meanwhile must be skipped, not dropped"
    );
    assert!(
        !logged.contains("unmatched_completion"),
        "a matched refusal is skipped, log was: {logged}"
    );
    {
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert_eq!(
            ledger.text_of(&occurrence).map(str::to_owned),
            text_after_close,
            "stale completion must not change the grounded label"
        );
        assert_eq!(ledger.is_sealed(&occurrence), sealed_after_close);
    }
    let mutations = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
        .count();
    assert_eq!(mutations, 0, "stale completion must not admit or correct");

    let receipt = tail_patch_receipt_after_stop(
        true,
        1,
        Some(TailPatchWorkerAccounting {
            applied_jobs: state.tail_patch_jobs_applied,
            skipped_jobs: state.tail_patch_jobs_skipped,
            timeout_residue: 0,
        }),
        SessionConservationReceipt::default(),
    );
    assert!(receipt.is_reconciled());
    assert_eq!(receipt.skipped, 1);
    assert_eq!(receipt.abandoned, 0);
    assert_eq!(receipt.overcount, 0);
    assert_eq!(receipt.drain, TailPatchDrainDisposition::Completed);
}

#[test]
fn tail_patch_receipt_names_missing_terminal_evidence() {
    let receipt = tail_patch_receipt_after_stop(
        true,
        3,
        Some(TailPatchWorkerAccounting {
            applied_jobs: 1,
            skipped_jobs: 1,
            timeout_residue: 0,
        }),
        SessionConservationReceipt::default(),
    );
    assert_eq!(receipt.applied, 1);
    assert_eq!(receipt.skipped, 1);
    assert_eq!(receipt.timed_out, 0);
    assert_eq!(receipt.abandoned, 1);
    assert_eq!(receipt.overcount, 0);
    assert_eq!(receipt.drain, TailPatchDrainDisposition::Abandoned);
    assert!(receipt.is_reconciled());
    assert_eq!(
        TailPatchSessionReceipt::from_events(&[receipt.as_event()]),
        Some(receipt)
    );
}

#[test]
fn stop_pressure_and_completion_race_classify_each_member_once() {
    for completion_first in [false, true] {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
        state.pump_capture_windows(&events);
        let request = requests.try_recv().unwrap();
        if completion_first {
            state.complete_whisper_window(&events, finish(&request), 20.0);
        }
        let deadline = state.refinement_clock + Duration::from_secs(1);
        assert!(!state.stop_refinements_tick(&events, deadline, deadline));
        let failures = warnings(&mut receiver, RefinementFailure::StopDeadline.code());
        // Every unsent reservation needs its own terminal receipt too.
        assert_eq!(
            failures, 3,
            "a payload-none completion does not return the Whisper frontier"
        );
        state.complete_whisper_window(&events, finish(&request), 20.0);
        assert!(!state.stop_refinements_tick(&events, deadline, deadline));
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
            0
        );
        assert!(state.refinement_pending.is_empty());
        assert!(state.refinement_submitted.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion(), 0);
        state.seal_remaining_at_session_end(&events);
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        let refusal = ledger
            .seal_terminal("live-admission", 7)
            .expect_err("no label at Stop");
        assert_eq!(refusal, SealRefusal::TextRecoveryPending);
        report_terminal_seal_refusal(&events, refusal);
        assert_eq!(
            warnings(&mut receiver, LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE),
            1
        );
        assert_eq!(ledger.qualified_occurrences().count(), 3);
        assert_eq!(ledger.layer_trail().len(), 3);
        for occurrence in ledger.qualified_occurrences() {
            assert!(ledger.frontier_of(occurrence).unwrap().is_closed());
            assert!(!ledger.is_sealed(occurrence));
            assert!(ledger.text_of(occurrence).is_none());
        }
        let speech = crate::audio::capture_receipt::AcousticSpeechEvidence::measured(
            crate::audio::capture_receipt::CaptureEvidenceIdentity::new("live-admission", 7),
            crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER,
            crate::audio::capture_receipt::AcousticAvailability::Observed {
                observed_samples: closed(3)
                    .utterances()
                    .iter()
                    .map(|u| u.range.sample_end)
                    .max()
                    .unwrap_or_default(),
            },
            closed(3)
                .utterances()
                .iter()
                .map(|u| u.range.clone())
                .collect::<Vec<_>>(),
        );
        let coverage = ledger.assess_seal_coverage("live-admission", 7, &speech, 250);
        assert_eq!(coverage.covered_samples, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Incomplete);
        assert!(ledger.record_seal_coverage(coverage));
        assert_eq!(
            ledger.seal_terminal("live-admission", 7),
            Err(SealRefusal::TextRecoveryPending)
        );
        assert_eq!(state.sealed_count, 0);
        assert_eq!(
            state.pending_events.len(),
            3,
            "retain unresolved evidence for recovery"
        );
        assert_eq!(
            state.window_by_samples(0, 2_400).unwrap().samples,
            vec![0.25; 2_400]
        );
    }
}
