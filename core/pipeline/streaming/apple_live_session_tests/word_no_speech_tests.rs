    use super::*;
    use crate::pipeline::acoustic_ledger::WordPin;

    #[test]
    fn live_word_adjudication_uses_existing_silero_and_publishes_deletion() {
        let mut state = AppleSealState::new_for_session(16_000, "word-no-speech".into(), 1);
        state.lexicon_custom_path = PathBuf::from("/nonexistent/codescribe-word-lexicon.jsonl");
        state.audio.push(&vec![0.0; 16_000]);
        let mut fusion = SileroIngress::new(16_000, "word-no-speech", 1);
        assert!(
            fusion.vad_available(),
            "fixture requires the bundled Silero model"
        );
        fusion.note_observed_pcm(16_000, 16_000);
        state.fusion = Some(fusion);
        let owner = OccurrenceIdentity::new("word-no-speech", 1, 0, 16_000);
        let observation =
            LedgerObservationIdentity::new(LedgerObservationProducer::Apple, 1, 0, owner.clone());
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .admit_word_slots(&observation, &[WordPin::new(0, 1_000, "phantom")]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.adjudicate_current_word_slots(&tx, &owner, 7);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some(""));
        assert_eq!(ledger.word_deletions().len(), 1);
        assert!(
            matches!(rx.try_recv().unwrap(), EngineEvent::LedgerMutation {
            receipt: MutationReceipt::Correct { .. }, label, ..
        } if label.is_empty())
        );
    }

    #[test]
    fn live_word_adjudication_retains_quiet_audio_and_missing_pcm() {
        for (samples, epoch) in [
            (vec![0.000_001; 16_000], 1),
            (vec![], 1),
            (vec![0.0; 16_000], 2),
        ] {
            let mut state = AppleSealState::new_for_session(16_000, "word-no-speech".into(), 1);
            state.lexicon_custom_path = PathBuf::from("/nonexistent/codescribe-word-lexicon.jsonl");
            state.audio.push(&samples);
            let mut fusion = SileroIngress::new(16_000, "word-no-speech", epoch);
            fusion.note_observed_pcm(16_000, 16_000);
            state.fusion = Some(fusion);
            let owner = OccurrenceIdentity::new("word-no-speech", 1, 0, 16_000);
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                1,
                0,
                owner.clone(),
            );
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .admit_word_slots(&observation, &[WordPin::new(0, 1_000, "plan")]);
            let (tx, _rx) = mpsc::unbounded_channel();
            state.adjudicate_current_word_slots(&tx, &owner, 7);
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("plan"));
            assert!(ledger.word_deletions().is_empty());
        }
    }
