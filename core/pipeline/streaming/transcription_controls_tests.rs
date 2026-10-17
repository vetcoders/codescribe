//! Integrator-authored production event routing checks; injected recognizer
//! observations, no claim of microphone/model recognition acceptance.
use super::*;

fn direct_state(apple: bool) -> (AppleSealState, mpsc::UnboundedReceiver<EngineEvent>) {
    direct_state_with_audio(apple, &vec![0.25; 32_000])
}

fn direct_state_with_audio(
    apple: bool,
    pcm: &[f32],
) -> (AppleSealState, mpsc::UnboundedReceiver<EngineEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut state = AppleSealState::new_for_session(16_000, "direct-engine".into(), 7);
    state.seal_ledger_enabled = false;
    state.apple_live_enabled = apple;
    state.direct_producer = LedgerObservationProducer::Whisper;
    state.direct_end = Some(DirectTranscriptEndGuard {
        destination: tx,
        session_id: "direct-engine".into(),
        capture_epoch: 7,
    });
    state.audio.push(pcm);
    (state, rx)
}

#[test]
fn direct_engine_retains_raw_translation_and_disabled_apple_has_no_output() {
    let (mut state, mut lifecycle) = direct_state(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let observation = LedgerObservationIdentity::new(
        LedgerObservationProducer::Whisper,
        1,
        0,
        OccurrenceIdentity::new("direct-engine", 7, 0, 32_000),
    );
    let raw = "Iwo Iwo Iwo Iwo Iwo. I can speak Polish.";
    assert!(state.publish_direct_transcript(&tx, observation, raw.into(), true));
    match rx.try_recv().unwrap() {
        EngineEvent::DirectTranscript {
            text, final_chunk, ..
        } => {
            assert_eq!(text, raw);
            assert!(final_chunk);
        }
        event => panic!("wrong publication: {event:?}"),
    }
    emit_stream_events(
        vec![LiveStreamEvent::Partial {
            text: "Spóźniony polski podgląd".into(),
            segments: vec![],
        }],
        &tx,
        &mut state,
        2.0,
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualified_occurrences()
            .count(),
        0
    );
    assert!(
        lifecycle.try_recv().is_err(),
        "capture remains open until owner completion"
    );
}

#[test]
fn direct_engine_primary_translation_suppresses_late_enabled_apple_preview() {
    let (mut state, _lifecycle) = direct_state(true);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let observation = LedgerObservationIdentity::new(
        LedgerObservationProducer::Whisper,
        1,
        0,
        OccurrenceIdentity::new("direct-engine", 7, 0, 32_000),
    );
    assert!(state.publish_direct_transcript(&tx, observation, "English survives.".into(), true));
    assert!(matches!(
        rx.try_recv().unwrap(),
        EngineEvent::DirectTranscript { .. }
    ));
    emit_stream_events(
        vec![
            LiveStreamEvent::Partial {
                text: "Polska rewizja".into(),
                segments: vec![],
            },
            LiveStreamEvent::PhraseFinal {
                text: "Polska rewizja końcowa".into(),
                segments: vec![],
            },
        ],
        &tx,
        &mut state,
        2.0,
    );
    assert!(
        rx.try_recv().is_err(),
        "a late Apple lane cannot rewrite the selected primary"
    );
}

// Inject measured capture facts, not model recognition. These checks exercise
// the production publication corridor and its own capture identity/range.
fn direct_input_admission_case(
    fusion_extent: Option<(u64, u64)>,
    fusion_foreign: bool,
    fusion_speech: bool,
    fusion_invalid: bool,
    energy_samples: &[f32],
) -> bool {
    use crate::audio::capture_receipt::CaptureLevelAccumulator;
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
    let (mut state, _lifecycle) = direct_state_with_audio(false, &vec![0.0; 32_000]);
    if let Some((count, end)) = fusion_extent {
        let mut fusion = SileroIngress::new(
            16_000,
            if fusion_foreign {
                "other-capture"
            } else {
                "direct-engine"
            },
            7,
        );
        fusion.note_observed_pcm(count, end);
        if fusion_invalid {
            fusion.note_pcm_validity(&[f32::NAN], end);
        }
        if fusion_speech {
            fusion.observe_boundaries(&[
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechStart,
                    sample: 4_000,
                    speech_probability: 0.9,
                },
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechEnd,
                    sample: 8_000,
                    speech_probability: 0.1,
                },
            ]);
        }
        assert!(
            fusion.vad_available(),
            "an unavailable observer cannot stand in for measured quiet in this fixture"
        );
        state.fusion = Some(fusion);
    }
    let mut energy = CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, 16_000);
    energy.push_samples(energy_samples);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let observation = LedgerObservationIdentity::new(
        LedgerObservationProducer::Whisper,
        1,
        0,
        OccurrenceIdentity::new("direct-engine", 7, 0, 32_000),
    );
    let admitted = state.publish_direct_transcript(
        &tx,
        observation,
        "Iwo Iwo Iwo Iwo Iwo. English translation.".into(),
        true,
    );
    assert_eq!(
        rx.try_recv().is_ok(),
        admitted,
        "the real output channel follows admission"
    );
    assert_eq!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualified_occurrences()
            .count(),
        0,
        "direct protection fabricates no ledger occurrences"
    );
    admitted
}

#[test]
fn direct_input_admission_suppresses_valid_fully_measured_non_speech() {
    assert!(!direct_input_admission_case(
        Some((32_000, 32_000)),
        false,
        false,
        false,
        &vec![0.0; 32_000]
    ));
}

#[test]
fn direct_input_admission_keeps_missing_partial_invalid_discontinuous_or_foreign_evidence() {
    let quiet = vec![0.0; 32_000];
    for (extent, foreign, invalid, energy) in [
        (None, false, false, quiet.as_slice()),
        (Some((32_000, 32_000)), false, false, &[][..]),
        (Some((16_000, 16_000)), false, false, quiet.as_slice()),
        (Some((32_000, 32_000)), false, false, &quiet[..16_000]),
        (Some((16_000, 32_000)), false, false, quiet.as_slice()),
        (Some((32_000, 32_000)), true, false, quiet.as_slice()),
        (Some((32_000, 32_000)), false, true, quiet.as_slice()),
    ] {
        assert!(
            direct_input_admission_case(extent, foreign, false, invalid, energy),
            "missing/partial/invalid/foreign measurement must preserve possible speech: {extent:?}/{foreign}/{invalid}"
        );
    }
    let mut invalid_energy = quiet;
    invalid_energy[20_000] = f32::NAN;
    assert!(direct_input_admission_case(
        Some((32_000, 32_000)),
        false,
        false,
        false,
        &invalid_energy
    ));
}

#[test]
fn direct_input_admission_keeps_measured_speech_and_translation() {
    assert!(direct_input_admission_case(
        Some((32_000, 32_000)),
        false,
        true,
        false,
        &vec![0.0; 32_000]
    ));
}

#[test]
fn direct_input_admission_keeps_five_disjoint_occurrences_with_translated_labels() {
    use crate::audio::capture_receipt::CaptureLevelAccumulator;
    let (mut state, _lifecycle) = direct_state_with_audio(false, &vec![0.002; 32_000]);
    let mut fusion = SileroIngress::new(16_000, "direct-engine", 7);
    fusion.note_observed_pcm(32_000, 32_000);
    fusion.observe_boundaries(&[
        crate::audio::chunker::VadBoundaryEvidence {
            kind: crate::audio::chunker::VadBoundaryKind::SpeechStart,
            sample: 0,
            speech_probability: 0.9,
        },
        crate::audio::chunker::VadBoundaryEvidence {
            kind: crate::audio::chunker::VadBoundaryKind::SpeechEnd,
            sample: 32_000,
            speech_probability: 0.1,
        },
    ]);
    assert!(fusion.vad_available());
    state.fusion = Some(fusion);
    CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, 16_000)
        .push_samples(&vec![0.002; 32_000]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    for index in 0..5_u64 {
        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            index + 1,
            0,
            OccurrenceIdentity::new("direct-engine", 7, index * 6_400, (index + 1) * 6_400),
        );
        assert!(state.publish_direct_transcript(
            &tx,
            observation,
            "Iwo. I am speaking Polish.".into(),
            true
        ));
    }
    let mut observed = Vec::new();
    while let Ok(EngineEvent::DirectTranscript {
        observation, text, ..
    }) = rx.try_recv()
    {
        assert_eq!(text, "Iwo. I am speaking Polish.");
        observed.push((
            observation.occurrence.sample_start,
            observation.occurrence.sample_end,
        ));
    }
    assert_eq!(
        observed,
        (0..5_u64)
            .map(|i| (i * 6_400, (i + 1) * 6_400))
            .collect::<Vec<_>>(),
        "five separate capture ranges remain five output observations despite equal translated labels"
    );
}

#[test]
fn direct_input_admission_energy_spikes_cannot_reclassify_measured_non_speech() {
    // Energy is not a speech classifier: the retained denied-label windows
    // contain real energy spikes while Silero measures no speech there.
    assert!(!direct_input_admission_case(
        Some((32_000, 32_000)),
        false,
        false,
        false,
        &vec![0.002; 32_000]
    ));
}
