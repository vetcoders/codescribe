//! Integrator-authored production event routing checks; injected recognizer
//! observations, no claim of microphone/model recognition acceptance.
use super::*;

fn direct_state(apple: bool) -> (AppleSealState, mpsc::UnboundedReceiver<EngineEvent>) {
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
    state.audio.push(&vec![0.25; 32_000]);
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
