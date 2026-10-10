//! Integrator fixtures: real ledger mutation, with measured synthetic PCM and no decoder.
use super::*;
use crate::pipeline::acoustic_ledger::WordPin;
use crate::pipeline::acoustic_ledger::word_adjudication_tests::measured_ledger;

fn fixture(
    words: &[&str],
    windows: &[(u64, u64)],
) -> (
    AppleSealState,
    OccurrenceIdentity,
    mpsc::UnboundedReceiver<EngineEvent>,
) {
    let session = "corroborated-dictionary";
    let owner = OccurrenceIdentity::new(session, 1, 0, 160_000);
    let mut pcm = vec![0.0; 160_000];
    let pins = words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            let start = 48_000 + index as u64 * 7_000;
            let end = start + 3_200;
            pcm[start as usize..end as usize].fill(0.2);
            WordPin::new(start, end, *word)
        })
        .collect::<Vec<_>>();
    let mut ledger = measured_ledger(&owner, &pcm);
    ledger.schedule_frontier(owner.clone(), [LedgerObservationProducer::Whisper]);
    for (index, &(start, end)) in windows.iter().enumerate() {
        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            index as u64 + 1,
            0,
            owner.clone(),
        );
        let window_pins = pins
            .iter()
            .cloned()
            .map(|pin| pin.with_decode_window(start, end))
            .collect::<Vec<_>>();
        ledger.admit_word_slots(&observation, &window_pins);
    }
    assert_eq!(ledger.text_of(&owner), Some(words.join(" ").as_str()));
    let (sender, _requests) = mpsc::channel(1);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        16_000,
        session.into(),
        1,
        sender,
        Arc::new(Mutex::new(ledger)),
        None,
    );
    state.lexicon_custom_path =
        PathBuf::from("/nonexistent/corroborated-dictionary/lexicon.custom.jsonl");
    let (_, receiver) = mpsc::unbounded_channel();
    (state, owner, receiver)
}

#[test]
fn two_complete_frames_correct_one_label_and_keep_five_physical_iwo() {
    let words = ["Iwo", "Iwo", "Iwo", "Iwo", "Iwo", "Alfaxone."];
    let (mut state, owner, _) = fixture(&words, &[(0, 128_000), (8_000, 136_000)]);
    let before = state
        .acoustic_ledger
        .lock()
        .unwrap()
        .slots_of(&owner)
        .unwrap()
        .to_vec();
    let (events, mut receiver) = mpsc::unbounded_channel();
    state.apply_corroborated_lexicon(&events, &owner, 99);
    let ledger = state.acoustic_ledger.lock().unwrap();
    let after = ledger.slots_of(&owner).unwrap();
    assert_eq!(
        ledger.text_of(&owner),
        Some("Iwo Iwo Iwo Iwo Iwo Alfaksalon.")
    );
    assert_eq!(after.len(), 6);
    assert_eq!(after.iter().filter(|slot| slot.text == "Iwo").count(), 5);
    for (left, right) in before.iter().zip(after) {
        assert_eq!(
            (left.sample_start, left.sample_end),
            (right.sample_start, right.sample_end)
        );
    }
    assert!(!ledger.is_sealed(&owner));
    assert_eq!(state.lexicon_rewrites, 1);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        EngineEvent::LedgerMutation { .. }
    ));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn replayed_frame_and_stop_cannot_apply_fuzzy_correction() {
    for (windows, stopping) in [
        (vec![(0, 128_000), (0, 128_000)], false),
        (vec![(0, 128_000), (8_000, 136_000)], true),
    ] {
        let (mut state, owner, _) = fixture(&["Alfaxone"], &windows);
        state.capture_stopping = stopping;
        let (events, mut receiver) = mpsc::unbounded_channel();
        state.apply_corroborated_lexicon(&events, &owner, 99);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("Alfaxone")
        );
        assert_eq!(state.lexicon_rewrites, 0);
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn two_frames_merge_only_the_exact_medical_or_programming_word_targets() {
    for (words, expected) in [
        (["postgrze", "SQL"], "PostgreSQL"),
        (["Robena", "coxip"], "Robenacoxib"),
    ] {
        let (mut state, owner, _) = fixture(&words, &[(0, 128_000), (8_000, 136_000)]);
        let (events, mut receiver) = mpsc::unbounded_channel();
        state.apply_corroborated_lexicon(&events, &owner, 99);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some(expected));
        let slots = ledger.slots_of(&owner).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(
            (slots[0].sample_start, slots[0].sample_end),
            (48_000, 58_200)
        );
        assert!(!ledger.is_sealed(&owner));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            EngineEvent::LedgerMutation { .. }
        ));
    }
}
