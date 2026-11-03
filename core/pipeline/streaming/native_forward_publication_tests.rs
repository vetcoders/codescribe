//! Real session/planner/provider seam with deterministic returned words.
//! Geometry and conservation acceptance, not microphone or model-quality claims.
use super::*;
use crate::pipeline::streaming::silero_fusion::UtteranceLedger;
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
    TailSegmentGrain, TailTimingQuality,
};

struct NativeFixture {
    state: AppleSealState,
    events: mpsc::UnboundedSender<EngineEvent>,
    _receiver: mpsc::UnboundedReceiver<EngineEvent>,
    requests: mpsc::Receiver<TailPatchRequest>,
    owner: OccurrenceIdentity,
    words: Vec<TimedTailSegment>,
}

fn fixture(
    rate: u32,
    duration_ms: u64,
    script: &[(&str, u64, u64)],
    coarse: bool,
) -> NativeFixture {
    fixture_with_owners(rate, duration_ms, script, coarse, &[(0, duration_ms)])
}

fn fixture_with_owners(
    rate: u32,
    duration_ms: u64,
    script: &[(&str, u64, u64)],
    coarse: bool,
    owners: &[(u64, u64)],
) -> NativeFixture {
    let session = format!("native-forward-{rate}-{duration_ms}-{coarse}");
    let at = |ms: u64| u64::from(rate) * ms / 1000;
    let capture_end = at(duration_ms);
    let words = script
        .iter()
        .map(|&(text, start, end)| TimedTailSegment {
            confidence: None,
            grain: TailSegmentGrain::Word,
            text: text.into(),
            range: TailSampleRange {
                session: session.clone(),
                capture_epoch: 1,
                sample_start: at(start),
                sample_end: at(end),
            },
        })
        .collect::<Vec<_>>();
    let mut pcm = vec![0.00001; capture_end as usize];
    for word in &words {
        pcm[word.range.sample_start as usize..word.range.sample_end as usize].fill(0.2);
    }
    let (events, _receiver) = mpsc::unbounded_channel();
    let (sender, requests) = mpsc::channel(16);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        rate,
        session.clone(),
        1,
        sender,
        Arc::new(Mutex::new(AcousticLedger::new())),
        Some(EnergyCalibration {
            version: "native-forward-pcm".into(),
            min_energy_integral: 0.01,
            min_valley_samples: 1,
        }),
    );
    state.existence_floor_rms = Some(0.01);
    state.audio.push(&pcm);
    let mut capture = CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, rate);
    for chunk in pcm.chunks((rate / 50) as usize) {
        capture.push_samples(chunk);
    }
    let mut physical = UtteranceLedger::new();
    for &(start, end) in owners {
        physical.open_or_extend(&session, 1, at(start), at(end));
        physical.close_open(at(end));
    }
    assert!(reconcile_silero_ledger(&mut state, &events, &physical, &[]));
    let owner = OccurrenceIdentity::new(&session, 1, at(owners[0].0), at(owners[0].1));
    assert!(state.acoustic_ledger.lock().unwrap().is_qualified(&owner));
    if coarse {
        let observation =
            LedgerObservationIdentity::new(LedgerObservationProducer::Apple, 1, 0, owner.clone());
        assert!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .admit(&observation, "prowizoryczna")
                .grants_mutation()
        );
    }
    state.pump_capture_windows(&events);
    NativeFixture {
        state,
        events,
        _receiver,
        requests,
        owner,
        words,
    }
}

fn complete(f: &mut NativeFixture, request: TailPatchRequest, corrupt_context: bool) {
    let frame = &request.provider_request.identity.range;
    let mut segments = f
        .words
        .iter()
        .filter(|word| frame.contains(&word.range))
        .cloned()
        .collect::<Vec<_>>();
    if corrupt_context && frame.sample_start > 0 {
        for word in &mut segments {
            if word.range.sample_end < u64::from(f.state.sample_rate) * 6 {
                word.text = "wyskocznie".into();
            }
        }
    }
    let payload = TailProviderPayload {
        identity: request.provider_request.identity.clone(),
        text: segments
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        segments,
        avg_logprob: Some(-0.2),
        compression_ratio: Some(1.0),
        provider_id: TailProviderId::Fake,
        elapsed_ms: 1,
        evidence: TailProviderEvidence {
            segment_grain: TailSegmentGrain::Word,
            source: TailEvidenceSource::Whisper,
            revision: Some("native-forward-fixture".into()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: Some(-0.2),
            vad_selected_samples: None,
        },
    };
    payload.validate().unwrap();
    f.state.complete_whisper_window(
        &f.events,
        TailPatchCompletion {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: Some(payload),
            member_occurrences: request.member_occurrences,
        },
        f.state.audio.session_sample_end() as f32 / f.state.sample_rate as f32,
    );
}

fn drain(f: &mut NativeFixture, reverse: bool, corrupt_context: bool) -> usize {
    let mut pending = std::iter::from_fn(|| f.requests.try_recv().ok()).collect::<Vec<_>>();
    if reverse {
        pending.reverse();
    }
    let count = pending.len();
    for request in pending {
        assert!(
            request.provider_request.identity.request_id < (1_u64 << 63),
            "no lexical-trial job"
        );
        complete(f, request, corrupt_context);
    }
    count
}

fn stop(f: &mut NativeFixture, reverse: bool, corrupt_context: bool) -> usize {
    f.state.capture_stopping = true;
    f.state.pump_capture_windows(&f.events);
    let count = drain(f, reverse, corrupt_context);
    f.state.pump_capture_windows(&f.events);
    f.state.return_outstanding_whisper_without_label(&f.events);
    f.state.close_admission_horizon(&f.events, u64::MAX);
    count
}

#[test]
fn native_forward_frames_keep_repetitions_seams_tail_and_completion_order() {
    let script = [
        ("raz", 1000, 1400),
        ("raz", 4000, 4400),
        ("szew", 5900, 6100),
        ("dalej", 7000, 7400),
        ("koniec", 10000, 10400),
        ("ogon", 13000, 13400),
    ];
    for rate in [16_000, 48_000] {
        for reverse in [false, true] {
            for coarse in [false, true] {
                let mut f = fixture(rate, 15000, &script, coarse);
                assert_eq!(drain(&mut f, reverse, false), 3);
                assert_eq!(
                    f.state.acoustic_ledger.lock().unwrap().text_of(&f.owner),
                    Some("raz raz szew dalej koniec")
                );
                assert_eq!(
                    stop(&mut f, reverse, false),
                    0,
                    "exact EOF uses retained tail without new decode"
                );
                let ledger = f.state.acoustic_ledger.lock().unwrap();
                assert_eq!(
                    ledger.text_of(&f.owner),
                    Some("raz raz szew dalej koniec ogon")
                );
                assert_eq!(ledger.slots_of(&f.owner).unwrap().len(), script.len());
                assert_eq!(ledger.qualified_occurrences().count(), 1);
                assert!(!ledger.text_recovery_pending(&f.owner));
                assert!(ledger.is_sealed(&f.owner));
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }
}

#[test]
fn native_context_disagreement_cannot_mutate_published_word_before_or_at_stop() {
    let script = [
        ("błyskowicznie", 5100, 5500),
        ("dalej", 7000, 7400),
        ("ogon", 13000, 13400),
    ];
    let mut f = fixture(48_000, 15000, &script, false);
    let first = f.requests.try_recv().unwrap();
    complete(&mut f, first, false);
    let held = f
        .state
        .acoustic_ledger
        .lock()
        .unwrap()
        .slots_of(&f.owner)
        .unwrap()[0]
        .clone();
    drain(&mut f, false, true);
    assert_eq!(stop(&mut f, false, true), 0);
    let ledger = f.state.acoustic_ledger.lock().unwrap();
    assert_eq!(ledger.text_of(&f.owner), Some("błyskowicznie dalej ogon"));
    assert!(ledger.slots_of(&f.owner).unwrap().contains(&held));
}

#[test]
fn native_quiet_context_only_tail_can_settle_without_erasing_the_word() {
    for coarse in [false, true] {
        let mut f = fixture(16_000, 9500, &[("szew", 3750, 5000)], coarse);
        drain(&mut f, false, false);
        assert_eq!(stop(&mut f, false, false), 1);
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&f.owner), Some("szew"));
        assert!(!ledger.text_recovery_pending(&f.owner));
        assert!(ledger.is_sealed(&f.owner));
    }
}

#[test]
fn native_short_take_publishes_every_word_on_one_actual_eof_return() {
    for duration in [1000, 5900, 8990] {
        let mut f = fixture(
            48_000,
            duration,
            &[
                ("pierwsze", 100, 300),
                ("ostatnie", duration - 300, duration - 100),
            ],
            true,
        );
        assert_eq!(drain(&mut f, false, false), 0);
        assert_eq!(stop(&mut f, false, false), 1);
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&f.owner), Some("pierwsze ostatnie"));
        assert!(!ledger.text_recovery_pending(&f.owner));
        assert!(ledger.is_sealed(&f.owner));
    }
}

#[test]
fn native_same_frame_adjacent_repetitions_keep_distinct_original_pins() {
    for rate in [16_000, 48_000] {
        for coarse in [false, true] {
            let mut f = fixture(
                rate,
                5900,
                &[
                    ("raz", 1000, 1250),
                    ("raz", 1250, 1600),
                    ("dalej", 3000, 3400),
                ],
                coarse,
            );
            assert_eq!(stop(&mut f, false, false), 1);
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&f.owner), Some("raz raz dalej"));
            let pins = ledger
                .slots_of(&f.owner)
                .unwrap()
                .iter()
                .filter(|slot| slot.text == "raz")
                .map(|slot| (slot.sample_start, slot.sample_end))
                .collect::<Vec<_>>();
            assert_eq!(
                pins,
                [
                    (u64::from(rate), u64::from(rate) * 1250 / 1000),
                    (u64::from(rate) * 1250 / 1000, u64::from(rate) * 1600 / 1000)
                ]
            );
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }
}

#[test]
fn native_neighbor_word_does_not_block_a_previous_owners_measured_quiet_tail() {
    let mut f = fixture_with_owners(
        48_000,
        15000,
        &[("alpha", 5100, 5500), ("neighbor", 11000, 11400)],
        false,
        &[(0, 10600), (10600, 13000)],
    );
    assert_eq!(drain(&mut f, false, false), 3);
    {
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&f.owner), Some("alpha"));
        assert!(
            !ledger.text_recovery_pending(&f.owner),
            "a different owner's in-band word cannot make this owner's quiet tail unaccounted"
        );
    }
    assert_eq!(stop(&mut f, false, false), 0);
    let ledger = f.state.acoustic_ledger.lock().unwrap();
    assert!(ledger.is_sealed(&f.owner));
    let neighbor = OccurrenceIdentity::new(&f.owner.session, 1, 508_800, 624_000);
    assert_eq!(ledger.text_of(&neighbor), Some("neighbor"));
    assert!(ledger.is_sealed(&neighbor));
    assert_eq!(ledger.qualified_occurrences().count(), 2);
}

pub(super) fn assert_one_independent_witness_settles_owned_word_disputes() {
    use crate::pipeline::acoustic_ledger::WordPin;
    let rate = 16_000;
    let script = [
        ("1286", 1000, 2000),
        ("nie", 3000, 4000),
        ("10", 8000, 9000),
    ];
    let mut f = fixture(rate, 10000, &script, false);
    f.state.apple_live_enabled = true;
    {
        let mut ledger = f.state.acoustic_ledger.lock().unwrap();
        let apple = LedgerObservationIdentity::new(
            LedgerObservationProducer::Apple,
            42,
            0,
            f.owner.clone(),
        );
        let words = [
            ("56", 16000, 32000),
            ("tak", 48000, 64000),
            ("7", 128000, 144000),
        ]
        .map(|(text, start, end)| WordPin::new(start, end, text).with_decode_window(0, 160000));
        assert!(ledger.admit_word_slots(&apple, &words).grants_mutation());
        ledger.schedule_observer(f.owner.clone(), LedgerObservationProducer::AppleDictation);
    }
    assert_eq!(drain(&mut f, false, false), 1);
    assert_eq!(
        stop(&mut f, false, false),
        1,
        "only the ordinary EOF grid frame is added"
    );
    assert!(f.requests.try_recv().is_err());
    assert!(!f.state.acoustic_ledger.lock().unwrap().is_sealed(&f.owner));

    let identity = TailRequestIdentity {
        request_id: (1_u64 << 62) + 1,
        range: TailSampleRange {
            session: f.owner.session.clone(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 160000,
        },
    };
    let mut segments = f.words.clone();
    for segment in &mut segments {
        segment.grain = TailSegmentGrain::Phrase;
    }
    let payload = TailProviderPayload {
        identity: identity.clone(),
        text: "1286 nie 10".into(),
        segments,
        avg_logprob: None,
        compression_ratio: None,
        provider_id: TailProviderId::AppleDictation,
        elapsed_ms: 1,
        evidence: TailProviderEvidence {
            segment_grain: TailSegmentGrain::Phrase,
            source: TailEvidenceSource::AppleDictation,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: None,
            vad_selected_samples: None,
        },
    };
    payload.validate().unwrap();
    admit_dictation_witness_payload(
        &mut f.state,
        &f.events,
        &identity,
        &payload,
        &[f.owner.clone()],
        &[],
        u64::from(rate) / 4,
    );
    {
        let mut ledger = f.state.acoustic_ledger.lock().unwrap();
        ledger.note_frontier_return(&f.owner, LedgerObservationProducer::AppleDictation);
        assert_eq!(ledger.text_of(&f.owner), Some("1286 nie 10"));
    }
    f.state.close_admission_horizon(&f.events, u64::MAX);
    let _ = reconcile_terminal_coverage(&mut f.state, &f.events);
    {
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger.is_sealed(&f.owner),
            "diagnostic={:?}; frontier={:?}",
            ledger.recovery_diagnostic(&f.owner),
            ledger.frontier_of(&f.owner)
        );
    }
    assert!(
        f.requests.try_recv().is_err(),
        "the independent witness never schedules a Whisper retrial"
    );
}

#[test]
fn nonword_whisper_context_cannot_publish_before_its_owner() {
    let mut f = fixture_with_owners(
        16_000,
        15_000,
        &[("właściwe", 7_000, 8_000)],
        false,
        &[(7_000, 8_000)],
    );
    let first = f.requests.try_recv().unwrap();
    let second = f.requests.try_recv().unwrap();
    for (index, request) in [first, second].into_iter().enumerate() {
        let mut segment = f.words[0].clone();
        segment.grain = TailSegmentGrain::Phrase;
        let payload = TailProviderPayload {
            identity: request.provider_request.identity.clone(),
            text: "właściwe".into(),
            segments: vec![segment],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.0),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain: TailSegmentGrain::Phrase,
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        };
        payload.validate().unwrap();
        f.state.complete_whisper_window(
            &f.events,
            TailPatchCompletion {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity),
                member_occurrences: request.member_occurrences,
                payload: Some(payload),
            },
            15.0,
        );
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        if index == 0 {
            assert_eq!(ledger.text_of(&f.owner), None);
            assert!(ledger.text_recovery_pending(&f.owner));
        } else {
            assert_eq!(ledger.text_of(&f.owner), Some("właściwe"));
        }
        assert_eq!(ledger.conservation().residue(), 0);
    }
}
