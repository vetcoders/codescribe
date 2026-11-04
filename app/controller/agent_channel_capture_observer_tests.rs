
    use super::*;
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
        OccurrenceIdentity, WordPin,
    };
    async fn fixture() -> (RecordingController, tempfile::TempDir, OpenAgentChannel) {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().unwrap();
        let binding = dir.path().join("agent-channel-binding.json");
        std::fs::write(&binding,r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"fixture-agent","provider":"codex","provider_session_id":"fixture-session"}}}"#).unwrap();
        controller
            .dispatch_agent_channel(
                3,
                &binding,
                &dir.path().join("bus.jsonl"),
                ChannelOpenMode::AttachedOnly,
            )
            .await
            .unwrap();
        let open = controller.agent_channel_snapshot(3).await.unwrap();
        (controller, dir, open)
    }
    fn ledger() -> AcousticLedger {
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("observer-channel-fixture", 1.0, 1);
        for session in ["owned-channel", "unrelated-dictation"] {
            let occurrence = OccurrenceIdentity::new(session, 1, 0, 16_000);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1_000.0,
                energy_integral: 100.0,
                mean_rms_dbfs: -20.0,
                peak_dbfs: -10.0,
                vad_open_sample: Some(0),
                vad_close_sample: Some(16_000),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            let observation =
                ObservationIdentity::new(ObservationProducer::Whisper, 47, 2, occurrence);
            assert!(
                ledger
                    .admit_word_slots_for_tests(
                        &observation,
                        &[WordPin::new(1_000, 14_000, session)]
                    )
                    .grants_mutation()
            );
        }
        ledger
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn live_channel_observer_uses_its_own_ledger_and_keeps_receipt_identity() {
        let (_controller, _dir, mut open) = fixture().await;
        open.mode = ChannelOpenMode::Live;
        open.session_id = Some("owned-channel".into());
        open.channel_session_id = open.session_id.clone();
        let ledger = Arc::new(StdMutex::new(ledger()));
        let original = ledger.lock().unwrap().layer_trail().to_vec();
        open.capture_ledger = Some(Arc::clone(&ledger));
        let card = RecordingController::channel_take_observer(&open);
        assert_eq!(card.session_id.as_deref(), Some("owned-channel"));
        assert_eq!(card.capture_epoch, Some(1));
        assert_eq!(card.words.len(), 1);
        assert_eq!(card.words[0].word, "owned-channel");
        let facts = card.ledger.unwrap();
        assert!(
            facts
                .decisions
                .iter()
                .all(|d| d.session_id == "owned-channel")
        );
        let original = original
            .into_iter()
            .filter(|d| d.observation.occurrence.session == "owned-channel")
            .collect::<Vec<_>>();
        assert_eq!(facts.decisions.len(), original.len());
        for (d, o) in facts.decisions.iter().zip(&original) {
            assert_eq!(d.receipt_id, o.receipt_id);
            assert_eq!(d.request, Some(o.observation.request));
            assert_eq!(d.generation, Some(o.observation.generation));
        }
        assert_eq!(ledger.lock().unwrap().layer_trail().len(), 2);
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn attached_channel_cannot_claim_live_evidence_even_if_a_handle_is_present() {
        let (_controller, _dir, mut open) = fixture().await;
        open.session_id = Some("owned-channel".into());
        open.channel_session_id = open.session_id.clone();
        open.capture_ledger = Some(Arc::new(StdMutex::new(ledger())));
        let card = RecordingController::channel_take_observer(&open);
        assert_eq!(card.session_id, None);
        assert_eq!(card.capture_epoch, None);
        assert_eq!(card.ledger, None);
        assert!(card.words.is_empty());
        assert_eq!(card.vad_speech_pct, None);
    }
