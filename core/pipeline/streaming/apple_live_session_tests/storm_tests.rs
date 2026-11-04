use super::*;
const TEST_SAMPLE_RATE: u32 = 16_000;
fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
    TranscriptSegment {
        confidence: None,
        text: text.into(),
        start_ts,
        end_ts,
    }
}

/// Feed `secs` of captured audio the way the worker does — chunk by chunk.
fn push_capture(state: &mut AppleSealState, secs: f32) {
    let total = (secs * TEST_SAMPLE_RATE as f32) as usize;
    let session = vec![0.25f32; total];
    for chunk in session.chunks(1024) {
        state.audio.push(chunk);
    }
}

fn at(secs: f32) -> u64 {
    (secs * TEST_SAMPLE_RATE as f32) as u64
}

/// Arm a state with the session Silero and mint two utterances separated by
/// a silence wider than the long-silence fence, exactly as the Supervisor
/// would: an open edge that extends, then a close, then a new edge.
///
/// The ledger is driven through the production decision function
/// ([`SileroIngress::observe`]) rather than a synthetic ledger, so what the
/// seal reads is what a real chunk observation produces. Only the two facts
/// Silero derives from the waveform are supplied by the fixture — the unit
/// suite must not depend on `init_silero_vad` succeeding.
fn arm_two_utterances(state: &mut AppleSealState) -> (u64, u64) {
    let mut ingress = SileroIngress::new(TEST_SAMPLE_RATE, state.session_id.clone(), 0);
    let first = ingress
        .observe(Some((at(0.0), at(1.0))), false, at(1.0))
        .open
        .expect("first speech edge mints an identity");
    ingress.observe(Some((at(0.0), at(2.0))), false, at(2.0));
    let closed = ingress.observe(None, true, at(2.0)).closed;
    assert_eq!(closed, vec![first]);

    // The first utterance is already closed. The gap is wider than the
    // acoustic merge, so the next edge mints a second identity.
    let gap = at(super::super::silero_fusion::ACOUSTIC_SPEECH_MERGE_GAP_SECS) + at(1.0);
    let second_start = at(2.0) + gap;
    let second = ingress
        .observe(
            Some((second_start, second_start + at(2.0))),
            false,
            second_start + at(2.0),
        )
        .open
        .expect("speech after the fence mints a SECOND identity");
    assert_ne!(first, second, "the fence must split identity");

    state.fusion = Some(ingress);
    state.fusion_seal_armed = true;
    (first, second)
}

fn arm_fusion_slice_admission(state: &mut AppleSealState) -> Vec<TranscriptSegment> {
    state.energy_calibration = Some(EnergyCalibration::new(
        "fusion-slice-structural-test",
        0.0,
        0,
    ));
    push_capture(state, 12.0);
    arm_two_utterances(state);
    let second_start = state
        .fusion
        .as_ref()
        .expect("fusion fixture")
        .ledger()
        .utterances()[1]
        .range
        .sample_start as f32
        / TEST_SAMPLE_RATE as f32;
    vec![
        segment("Iwo", 0.2, 0.8),
        segment("Iwo", second_start + 0.2, second_start + 0.8),
    ]
}

#[test]
fn unmatched_words_warn_once_per_session() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
    arm_fusion_slice_admission(&mut state);
    let words = vec![segment("leftover", 9.0, 9.1)];
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    for _ in 0..100 {
        assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
    }
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    let warnings = events
        .iter()
        .filter(|event| {
            matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap")
        })
        .count();
    assert_eq!(warnings, 1, "unchanged leftovers must warn only once");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
    );

    // A repeated observation does not warn again; equal text on new PCM does.
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    assert!(rx.try_recv().is_err());
    assert!(seal_sliced_by_silero(
        &mut state,
        &tx,
        &[segment("leftover", 9.2, 9.3)]
    ));
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap"))
            .count(),
        1
    );
}

#[test]
fn unmatched_word_three_deliveries_keep_one_store_and_mirror_entry() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
    arm_fusion_slice_admission(&mut state);
    let words = vec![segment("leftover", 9.0, 9.125)];

    for delivery in 0..3 {
        assert!(seal_sliced_by_silero(&mut state, &tx, &words));
        assert_eq!(state.unmatched_silero_words.len(), 1);
        let retained = &state.unmatched_silero_words[0];
        assert_eq!(
            (
                retained.sample_start,
                retained.sample_end,
                retained.text.as_str()
            ),
            (at(9.0), at(9.125), "leftover")
        );
        assert_eq!(state.unadmitted_revision, 1);
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        if delivery == 0 {
            let mirrors = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::UnadmittedAppleWords {
                        revision, words, ..
                    } => Some((revision, words)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(mirrors.len(), 1);
            assert_eq!(*mirrors[0].0, 1);
            assert_eq!(mirrors[0].1.len(), 1);
            assert_eq!(mirrors[0].1[0].text, "leftover");
            assert_eq!(mirrors[0].1[0].sample_start, at(9.0));
            assert_eq!(mirrors[0].1[0].sample_end, at(9.125));
            assert_eq!(mirrors[0].1[0].source, UnadmittedAppleWordSource::Unmatched);
        } else {
            assert!(events.is_empty(), "identical re-delivery must be silent");
        }
    }
}

#[test]
fn unmatched_words_keep_distinct_pins_and_text() {
    for second in [
        segment("leftover", 9.25, 9.375),
        segment("leftover", 9.0, 9.25),
        segment("leftover", 9.0625, 9.125),
        segment("revised", 9.0, 9.125),
    ] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
        arm_fusion_slice_admission(&mut state);
        let first = segment("leftover", 9.0, 9.125);
        // An exact copy separated by a distinct word is still a re-delivery.
        let words = vec![first.clone(), second.clone(), first];
        assert!(seal_sliced_by_silero(&mut state, &tx, &words));
        assert_eq!(state.unmatched_silero_words.len(), 2);
        let expected = vec![
            (at(9.0), at(9.125), "leftover"),
            (at(second.start_ts), at(second.end_ts), second.text.as_str()),
        ];
        assert_eq!(
            state
                .unmatched_silero_words
                .iter()
                .map(|word| (word.sample_start, word.sample_end, word.text.as_str()))
                .collect::<Vec<_>>(),
            expected
        );
        let mirrors = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::UnadmittedAppleWords { words, .. } => Some(words),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(mirrors.len(), 1);
        // Clock overlap alone cannot erase another native lexical pin.
        let preview = if second.start_ts == 9.0 && second.end_ts == 9.125 {
            // Two surfaces for exactly the same native PCM pin are one
            // preview position; both hypotheses remain retained above.
            vec![(at(second.start_ts), at(second.end_ts), second.text.as_str())]
        } else {
            expected.clone()
        };
        assert_eq!(
            mirrors[0]
                .iter()
                .map(|word| (word.sample_start, word.sample_end, word.text.as_str()))
                .collect::<Vec<_>>(),
            preview
        );
    }
}

#[test]
fn no_time_overlap_retries_on_silero_extension_and_close() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
    arm_fusion_slice_admission(&mut state);
    seal_sliced_by_silero(&mut state, &tx, &[segment("later", 9.0, 9.1)]);
    while rx.try_recv().is_ok() {}
    let fusion = state.fusion.as_mut().unwrap();
    let start = fusion
        .ledger()
        .utterances()
        .last()
        .unwrap()
        .range
        .sample_start;
    fusion.observe(Some((start, at(10.0))), false, at(10.0));
    seal_sliced_by_silero(&mut state, &tx, &[]);
    assert!(state.unmatched_silero_words.is_empty());
    assert!(!state.pending_silero_words.is_empty());
    state.fusion.as_mut().unwrap().observe(None, true, at(10.0));
    seal_sliced_by_silero(&mut state, &tx, &[]);
    assert!(state.pending_silero_words.is_empty());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
    );
    assert!(!events.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap")));
    assert_eq!(state.no_time_overlap_warnings, 1);
    assert_eq!(state.warned_unmatched_words.len(), 1);
}

#[test]
fn silero_tick_with_retained_words_is_constant_cost() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
    arm_fusion_slice_admission(&mut state);
    let words = (0..104)
        .map(|i| {
            let start = 9.0 + i as f32 * 0.01;
            segment("leftover", start, start + 0.005)
        })
        .collect::<Vec<_>>();
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    while rx.try_recv().is_ok() {}
    let storage = state.unmatched_silero_words.as_ptr();
    let started = std::time::Instant::now();
    let mut retained_storage = true;
    for _ in 0..200 {
        assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
        retained_storage &= storage == state.unmatched_silero_words.as_ptr();
    }
    let elapsed = started.elapsed();
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    eprintln!(
        "F3: 200 ticks, 104 words: {elapsed:?}, events={}",
        events.len()
    );
    assert!(
        events.is_empty(),
        "idle callbacks must not enqueue warnings or ledger mutations"
    );
    // Deterministic work bound, independent of host scheduling: no copying
    // the retained word vector on an unchanged Silero boundary snapshot.
    assert!(
        retained_storage,
        "idle callbacks must reuse retained word storage"
    );
    assert_eq!(state.unmatched_silero_words.len(), 104);
}
