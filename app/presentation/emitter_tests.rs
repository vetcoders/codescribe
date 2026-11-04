    use super::{
        IncrementalShapingRefusal, PresentationEmitter, TerminalFormatterRequest,
        TranscriptReducer, UserRevisionCommit, UserRevisionIntent, UserRevisionRefusal,
    };
    use crate::presentation::transcript_bus::{
        TranscriptBus, TranscriptBusEvidenceEvent, TranscriptDelivery, TranscriptMode,
        TranscriptProjectionPhase, TranscriptSession, TranscriptSessionEndReason,
    };
    use crate::presentation::transcript_projection::TranscriptProjectionReader;
    use codescribe_core::config::FormattingPolicy;
    use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
    use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticLedger, ConsultationPresentationInput,
        DocumentRevisionProvenance, EnergyCalibration, IncrementalShapingReceipt, MutationReceipt,
        ObservationIdentity, ObservationProducer, OccurrenceIdentity, SealRefusal,
    };
    use codescribe_core::pipeline::contracts::PreviewFinalDisposition;
    use codescribe_core::pipeline::contracts::{
        AnnotationKind, DeltaSink, EngineEvent, EventSink, LayerSource, LayerSummary, PreviewPin,
        TranscriptDelta, UnadmittedAppleWord, UnadmittedAppleWordSource,
    };
    use codescribe_core::stt::tail_provider::TailSampleRange;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    fn live_cue_raw_preview(rev: u64, text: &str) -> EngineEvent {
        EngineEvent::Preview {
            rev,
            text: text.into(),
            pin: PreviewPin::open_occurrence(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 16_000,
                sample_end: 32_000,
            }),
        }
    }

    #[tokio::test]
    async fn live_cue_apple_preview_is_immediate_and_cannot_mutate_delivery() {
        let (emitter, delivery, ledger) = opened_delivery_take();
        let paints = Arc::new(StdMutex::new(Vec::<String>::new()));
        let observed = Arc::clone(&paints);
        let mut emitter = emitter.with_apple_preview_observer(Arc::new(move |text| {
            observed.lock().unwrap().push(text.to_owned());
        }));
        emitter.on_event(&live_cue_raw_preview(1, "to już bieżące słowa"));
        assert_eq!(paints.lock().unwrap().as_slice(), ["to już bieżące słowa"]);
        assert!(delivery.lock().await.is_empty());
        assert!(
            ledger
                .lock()
                .unwrap()
                .text_of(&OccurrenceIdentity::new("take", 7, 0, 16_000))
                .is_none()
        );

        emitter.on_event(&partial_mirror(2, "pełny podgląd dokumentu", 0, 16_000));
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "Zatwierdzony tekst",
        );
        emitter.on_event(&mutation);
        emitter.on_event(&closed_mirror(
            3,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            3,
        ));
        assert_eq!(
            paints.lock().unwrap().len(),
            1,
            "Reducer paints cannot overwrite the pointer"
        );
        emitter.on_event(&EngineEvent::VadEnd {
            speech_prob: 0.0,
            ts_ms: 1000,
        });
        assert_eq!(paints.lock().unwrap().last().unwrap(), "");
        emitter.on_event(&live_cue_raw_preview(3, "następna fraza"));
        emitter.on_event(&EngineEvent::NoSpeech {
            reason: "fixture quiet".into(),
        });
        assert_eq!(paints.lock().unwrap().last().unwrap(), "");
        emitter.on_event(&live_cue_raw_preview(4, "ostatni podgląd"));
        emitter.finish().await;
        assert_eq!(paints.lock().unwrap().last().unwrap(), "");
        assert_eq!(delivery.lock().await.as_str(), "Zatwierdzony tekst");
    }

    #[tokio::test]
    async fn live_cue_retired_take_cannot_paint_late_apple_words() {
        let (emitter, _, _) = opened_delivery_take();
        let paints = Arc::new(StdMutex::new(Vec::<String>::new()));
        let observed = Arc::clone(&paints);
        let mut emitter = emitter.with_apple_preview_observer(Arc::new(move |text| {
            observed.lock().unwrap().push(text.to_owned());
        }));
        emitter.on_event(&live_cue_raw_preview(1, "bieżąca fraza"));
        emitter.retire_presentation();
        emitter.on_event(&live_cue_raw_preview(2, "spóźniona poprzednia fraza"));
        emitter.on_event(&EngineEvent::VadEnd {
            speech_prob: 0.0,
            ts_ms: 1000,
        });
        emitter.finish().await;
        assert_eq!(paints.lock().unwrap().as_slice(), ["bieżąca fraza"]);
    }

    /// A complete mirror containing only this open phrase at its actual PCM range.
    fn partial_mirror(rev: u64, text: &str, start: u64, end: u64) -> EngineEvent {
        mirror(
            rev,
            mirror_words(
                text,
                start,
                end,
                UnadmittedAppleWordSource::OpenPartial { rev, phrase_id: 1 },
            ),
        )
    }

    fn mirror(revision: u64, words: Vec<UnadmittedAppleWord>) -> EngineEvent {
        EngineEvent::UnadmittedAppleWords {
            revision,
            words,
            closed_phrases: Default::default(),
        }
    }

    fn closed_mirror(
        revision: u64,
        words: Vec<UnadmittedAppleWord>,
        phrase_id: u64,
        outcome: codescribe_core::pipeline::contracts::ApplePhraseOutcome,
        count: usize,
    ) -> EngineEvent {
        use codescribe_core::pipeline::contracts::{ApplePhraseOutcome, ClosedApplePhrase};
        EngineEvent::UnadmittedAppleWords {
            revision,
            words,
            closed_phrases: std::collections::BTreeMap::from([(
                phrase_id,
                ClosedApplePhrase {
                    arrival_index: (phrase_id - 1) as usize,
                    outcomes: std::collections::BTreeMap::from([(outcome, count)]),
                    was_untimed: outcome == ApplePhraseOutcome::Untimed,
                },
            )]),
        }
    }

    fn mirror_words(
        text: &str,
        sample_start: u64,
        sample_end: u64,
        source: UnadmittedAppleWordSource,
    ) -> Vec<UnadmittedAppleWord> {
        text.split_whitespace()
            .map(|word| UnadmittedAppleWord {
                text: word.into(),
                sample_start,
                sample_end,
                source,
            })
            .collect()
    }

    #[derive(Default)]
    struct RecordingDeltaSink {
        deltas: StdMutex<Vec<TranscriptDelta>>,
    }

    #[tokio::test]
    async fn pending_recovery_does_not_hide_new_live_words() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let delivery = Arc::new(Mutex::new(String::new()));
        let emitter = super::PresentationEmitter::new(delivery.clone(), None, None)
            .with_cursor_observer(Arc::new(move |paint| {
                observed.lock().unwrap().push(paint.clone());
            }));
        emitter.on_capture_opened("take", 7);
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: codescribe_core::pipeline::contracts::SpeechIntegrity {
                session_id: "take".into(),
                capture_epoch: 7,
                sequence: 1,
                acoustic_speech_ms_since_text_advance: 0,
                pending_occurrences: 1,
                phase: codescribe_core::pipeline::contracts::SpeechIntegrityPhase::Recovering,
                speech_live: false,
            },
        });
        assert_eq!(paints.lock().unwrap().last().unwrap().text, "…");
        emitter.on_event(&partial_mirror(1, "nowe słowa na żywo", 0, 16_000));
        let paint = paints.lock().unwrap().last().unwrap().clone();
        assert_eq!(paint.text, "nowe słowa na żywo");
        assert!(
            paint.degraded,
            "new words cannot certify the earlier missing speech"
        );
        assert!(delivery.lock().await.is_empty());
    }

    #[tokio::test]
    async fn compact_projection_keeps_capture_identity_and_publication_order() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let emitter =
            super::PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None)
                .with_cursor_observer(Arc::new(move |paint| {
                    observed.lock().unwrap().push(paint.clone());
                }));
        emitter.on_capture_opened("take", 7);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..10 {
                        emitter.paint_cursor("jeden dwa trzy cztery pięć sześć");
                    }
                });
            }
        });
        let paints = paints.lock().unwrap();
        assert_eq!(paints.len(), 81);
        assert!(paints[0].text.is_empty());
        for (index, paint) in paints.iter().enumerate() {
            assert_eq!(paint.sequence, index as u64 + 1);
            assert_eq!(paint.session_id, "take");
            assert_eq!(paint.capture_epoch, 7);
            assert_eq!(
                paint.text,
                if index == 0 {
                    ""
                } else {
                    "jeden dwa trzy cztery pięć sześć"
                }
            );
            let json = serde_json::to_string(paint).unwrap();
            assert_eq!(
                *paint,
                serde_json::from_str::<super::CompactProjection>(&json).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn capture_projects_full_preview_and_acoustic_debt_without_delivery() {
        use super::CursorVoice;
        use codescribe_core::pipeline::contracts::{SpeechIntegrity, SpeechIntegrityPhase};
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = paints.clone();
        let voices = Arc::new(StdMutex::new(Vec::new()));
        let heard = voices.clone();
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                dictation_session("take", false),
                temp.path().join("cursor.jsonl"),
                None,
            )
            .unwrap(),
        );
        let emitter = super::PresentationEmitter::new_with_transcript_bus(
            delivery.clone(),
            None,
            None,
            Some(bus),
        )
        .with_cursor_observer(Arc::new(move |projection| {
            observed
                .lock()
                .unwrap()
                .push((projection.text.clone(), projection.degraded));
            heard.lock().unwrap().push(projection.voice);
        }));
        emitter.on_event(&partial_mirror(
            1,
            "zero jeden dwa trzy cztery pięć",
            0,
            16_000,
        ));
        assert!(paints.lock().unwrap().is_empty());
        let mut evidence = SpeechIntegrity {
            session_id: "take".into(),
            capture_epoch: 7,
            sequence: 1,
            acoustic_speech_ms_since_text_advance: 2400,
            pending_occurrences: 1,
            phase: SpeechIntegrityPhase::Stalled,
            speech_live: false,
        };
        let mut foreign_first = evidence.clone();
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: evidence.clone(),
        });
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        emitter.on_capture_opened("take", 0);
        emitter.on_capture_opened("previous take", 7);
        assert!(emitter.cursor_capture.get().is_none());
        emitter.on_capture_opened("take", 7);
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("zero jeden dwa trzy cztery pięć".into(), false)
        );
        emitter.on_capture_opened("take", 8);
        foreign_first.capture_epoch = 6;
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: foreign_first.clone(),
        });
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        foreign_first.session_id = "previous take".into();
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: foreign_first,
        });
        assert!(!paints.lock().unwrap().last().unwrap().1);
        assert!(emitter.cursor_integrity.lock().unwrap().is_none());
        for phase in [
            SpeechIntegrityPhase::Stalled,
            SpeechIntegrityPhase::Recovering,
            SpeechIntegrityPhase::Unresolved,
        ] {
            evidence.phase = phase;
            emitter.on_event(&EngineEvent::SpeechIntegrity {
                evidence: evidence.clone(),
            });
            assert_eq!(
                paints.lock().unwrap().last().unwrap(),
                &("zero jeden dwa trzy cztery pięć".into(), true)
            );
            assert_eq!(voices.lock().unwrap().last(), Some(&CursorVoice::Lagging));
            emitter.on_event(&partial_mirror(
                evidence.sequence + 1,
                "nowe słowa na żywo",
                0,
                16_000,
            ));
            assert_eq!(
                paints.lock().unwrap().last().unwrap(),
                &("nowe słowa na żywo".into(), true)
            );
            emitter.paint_cursor("");
            assert_eq!(paints.lock().unwrap().last().unwrap(), &("…".into(), true));
            emitter.paint_cursor("zero jeden dwa trzy cztery pięć");
            evidence.sequence += 1;
        }
        let before = paints.lock().unwrap().len();
        for (epoch, sequence) in [(7, evidence.sequence - 1), (8, u64::MAX)] {
            let mut stale = evidence.clone();
            stale.capture_epoch = epoch;
            stale.sequence = sequence;
            stale.phase = SpeechIntegrityPhase::Tracking;
            emitter.on_event(&EngineEvent::SpeechIntegrity { evidence: stale });
            assert_eq!(paints.lock().unwrap().len(), before);
            assert!(paints.lock().unwrap().last().unwrap().1);
        }
        let mut stale = evidence.clone();
        stale.session_id = "old take".into();
        emitter.on_event(&EngineEvent::SpeechIntegrity { evidence: stale });
        assert_eq!(paints.lock().unwrap().len(), before);
        // A canonical repair paints while amber. Clearing the warning must
        // reveal that repair, not resurrect the earlier provisional preview.
        emitter.paint_cursor("odzyskany cały fragment");
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("odzyskany cały fragment".into(), true)
        );
        evidence.phase = SpeechIntegrityPhase::Tracking;
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: evidence.clone(),
        });
        assert_eq!(
            paints.lock().unwrap().last().unwrap(),
            &("odzyskany cały fragment".into(), false)
        );
        // Maciej, 2026-10-14: the dot tells a speaking voice from a quiet one.
        assert_eq!(voices.lock().unwrap().last(), Some(&CursorVoice::Quiet));
        evidence.sequence += 1;
        evidence.speech_live = true;
        emitter.on_event(&EngineEvent::SpeechIntegrity { evidence });
        assert_eq!(voices.lock().unwrap().last(), Some(&CursorVoice::Speaking));
        assert!(delivery.lock().await.is_empty());
    }

    #[test]
    fn cursor_voice_reads_owed_text_before_speech() {
        use super::CursorVoice;
        use codescribe_core::pipeline::contracts::{SpeechIntegrity, SpeechIntegrityPhase};
        let integrity = |phase, speech_live| SpeechIntegrity {
            session_id: "take".into(),
            capture_epoch: 1,
            sequence: 1,
            acoustic_speech_ms_since_text_advance: 0,
            pending_occurrences: 0,
            phase,
            speech_live,
        };
        assert_eq!(CursorVoice::from_integrity(None), CursorVoice::Unobserved);
        for speech_live in [false, true] {
            assert_eq!(
                CursorVoice::from_integrity(Some(&integrity(
                    SpeechIntegrityPhase::Unavailable,
                    speech_live
                ))),
                CursorVoice::Unobserved
            );
            for owed in [
                SpeechIntegrityPhase::Stalled,
                SpeechIntegrityPhase::Recovering,
                SpeechIntegrityPhase::Unresolved,
            ] {
                assert_eq!(
                    CursorVoice::from_integrity(Some(&integrity(owed, speech_live))),
                    CursorVoice::Lagging,
                    "owed words outrank a speaking voice"
                );
            }
        }
        for phase in [
            SpeechIntegrityPhase::Listening,
            SpeechIntegrityPhase::Tracking,
        ] {
            assert_eq!(
                CursorVoice::from_integrity(Some(&integrity(phase, true))),
                CursorVoice::Speaking
            );
            assert_eq!(
                CursorVoice::from_integrity(Some(&integrity(phase, false))),
                CursorVoice::Quiet
            );
        }
    }

    impl DeltaSink for RecordingDeltaSink {
        fn apply(&self, delta: &TranscriptDelta) {
            self.deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(delta.clone());
        }
    }

    /// Qualify one occurrence through the ledger's calibrated energy predicate,
    /// then admit an Apple observation over exactly that span.
    ///
    /// Both steps are load-bearing. `admit` alone records a layer decision whose
    /// serial list is copied from `evidence`, so an occurrence that never
    /// cleared `qualify` produces a decision that is not evidence-backed — and
    /// the reducer must refuse it. Authenticating here is what makes the
    /// assertions downstream about repetition and revision projection, rather
    /// than about the admission gate itself.
    fn admitted_mutation(
        ledger: &mut AcousticLedger,
        occurrence: OccurrenceIdentity,
        request: u64,
        label: &str,
    ) -> EngineEvent {
        let calibration = EnergyCalibration {
            version: "emitter-test".to_string(),
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
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, request, 0, occurrence);
        let receipt = ledger.admit(&observation, label);
        EngineEvent::LedgerMutation {
            observation,
            label: label.to_string(),
            receipt,
        }
    }

    /// The capture energy ladder's measurement of one take: speech over
    /// `speech`, measured quiet elsewhere up to `observed_samples`. A Whisper
    /// or cloud decode retires held wording only where its own words cover
    /// it or this measurement proves the rest quiet (absence proof, D5).
    fn record_measured_speech(
        ledger: &mut AcousticLedger,
        session: &str,
        capture_epoch: u64,
        observed_samples: u64,
        speech: &[(u64, u64)],
    ) {
        use codescribe_core::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CAPTURE_ENERGY_PRODUCER,
            CaptureEvidenceIdentity,
        };
        ledger.record_speech_evidence(&AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(session, capture_epoch),
            CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::Observed { observed_samples },
            speech
                .iter()
                .map(|&(sample_start, sample_end)| TailSampleRange {
                    session: session.into(),
                    capture_epoch,
                    sample_start,
                    sample_end,
                })
                .collect(),
        ));
    }

    #[test]
    fn group_answer_preserves_later_speech_and_authenticates_bus_publication() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let mut members = Vec::new();
        for (index, label) in ["iwo", "iwo", "dalsze słowa"].into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(
                "group-live",
                1,
                index as u64 * 16_000,
                (index as u64 + 1) * 16_000,
            );
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = admitted_mutation(&mut ledger, occurrence.clone(), index as u64, label)
            else {
                unreachable!()
            };
            reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .unwrap();
            if index < 2 {
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
                let seal = ledger.seal(&occurrence).unwrap().clone();
                reducer.apply_ledger_seal(&seal).unwrap();
                members.push(ConsultationPresentationMember {
                    occurrence: occurrence.clone(),
                    source_label: label.into(),
                    seal_receipt: seal.receipt_id,
                });
                let _ = reducer.apply_incremental_shaping(&mut ledger, &occurrence);
            }
        }
        let revision = reducer
            .apply_consultation_presentation(
                &mut ledger,
                ConsultationPresentationInput {
                    consultation_id: "Max",
                    turn_id: "first",
                    source_revision: 0,
                    revision: 1,
                    members: &members,
                    rendered_text: "Gotowe polecenie",
                },
            )
            .unwrap();
        assert_eq!(revision.rendered_text, "Gotowe polecenie dalsze słowa");
        assert!(revision.authenticates_publication());
        assert_eq!(reducer.revision, revision.source_raw_revision);
        assert_eq!(reducer.committed_rendered_text(), "Iwo iwo dalsze słowa");
        assert_eq!(ledger.text_of(&members[0].occurrence), Some("iwo"));
        assert_eq!(ledger.text_of(&members[1].occurrence), Some("iwo"));
        assert!(!reducer.terminal);
        let mut forged = revision.clone();
        forged.rendered_text = "forged".into();
        assert!(!forged.authenticates_publication());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("groups.jsonl");
        let bus = TranscriptBus::open_at(dictation_session("group-live", true), path.clone(), None)
            .unwrap();
        bus.record_derived_projection(&forged);
        bus.record_derived_projection(&revision);
        let history =
            crate::presentation::transcript_bus::document_history_at(&path, "group-live").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].rendered_text, revision.rendered_text);
        assert!(
            reducer
                .apply_consultation_presentation(
                    &mut ledger,
                    ConsultationPresentationInput {
                        consultation_id: "Max",
                        turn_id: "first",
                        source_revision: 0,
                        revision: 1,
                        members: &members,
                        rendered_text: "duplicate",
                    }
                )
                .is_err()
        );
        assert_eq!(ledger.consultation_presentations().len(), 1);

        let later = OccurrenceIdentity::new("group-live", 1, 48_000, 64_000);
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted_mutation(&mut ledger, later, 4, "jutro")
        else {
            unreachable!()
        };
        let suffix = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();
        assert_eq!(suffix.rendered_text, "Iwo iwo dalsze słowa jutro");
        assert_eq!(
            reducer.consulted_rendered_text(),
            "Gotowe polecenie dalsze słowa jutro"
        );
        assert!(suffix.authenticates_publication(&ledger, "group-live"));
        assert_eq!(bus.publish_revision(&suffix, &ledger).len(), 4);
        assert_eq!(ledger.len(), 4);
    }

    fn raw_final(text: &str) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn pending_ranges_follow_paint_provenance_and_settle_without_changing_bytes() {
        let (emitter, ledger) = mirror_take();
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = Arc::clone(&paints);
        let mut emitter = emitter.with_cursor_observer(Arc::new(move |paint| {
            observed.lock().unwrap().push(paint.clone());
        }));
        let first = OccurrenceIdentity::new("take", 7, 0, 8_000);
        let unanchored = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, first);
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &unanchored,
            "🐢 e\u{301}",
            super::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation: unanchored,
            label: "🐢 e\u{301}".into(),
            receipt,
        });
        let middle = OccurrenceIdentity::new("take", 7, 8_000, 16_000);
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(), middle, 2, "Iwo");
        emitter.on_event(&mutation);
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "🐢 e\u{301}",
                24_000,
                32_000,
                UnadmittedAppleWordSource::Pending { utterance_id: 3 },
            ),
        ));
        assert!(emitter.wait_paint_published().await);
        let before = paints.lock().unwrap().last().unwrap().clone();
        assert_eq!(before.text, "🐢 e\u{301} Iwo 🐢 e\u{301}");
        assert_eq!(
            before.pending_ranges,
            vec![
                super::PendingRange {
                    utf16_start: 0,
                    utf16_end: 5
                },
                super::PendingRange {
                    utf16_start: 10,
                    utf16_end: 15
                },
            ]
        );
        assert_eq!(
            emitter
                .session_state
                .lock()
                .unwrap()
                .committed_rendered_text(),
            "Iwo"
        );

        let last = OccurrenceIdentity::new("take", 7, 24_000, 32_000);
        let mutation = admitted_mutation(&mut ledger.lock().unwrap(), last, 3, "🐢 e\u{301}");
        emitter.on_event(&mutation);
        emitter.on_event(&mirror(2, Vec::new()));
        assert!(emitter.wait_paint_published().await);
        let after = paints.lock().unwrap().last().unwrap().clone();
        assert_eq!(after.text.as_bytes(), before.text.as_bytes());
        assert_eq!(
            after.pending_ranges,
            vec![super::PendingRange {
                utf16_start: 0,
                utf16_end: 5
            }]
        );
        assert!(after.sequence > before.sequence);
        assert_eq!(
            (&after.session_id, after.capture_epoch),
            (&before.session_id, before.capture_epoch)
        );
        assert_eq!(
            emitter
                .session_state
                .lock()
                .unwrap()
                .committed_rendered_text(),
            "Iwo 🐢 e\u{301}"
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn mirror_paints_overlay_without_writing_delivery() {
        let delivery = Arc::new(Mutex::new("ledger truth".to_string()));
        let deltas = Arc::new(RecordingDeltaSink::default());
        let mut emitter =
            PresentationEmitter::new(Arc::clone(&delivery), Some(deltas.clone()), None);

        emitter.on_event(&partial_mirror(1, "volatile words", 0, 16_000));
        assert!(emitter.wait_paint_published().await);

        assert_eq!(delivery.lock().await.as_str(), "ledger truth");
        assert!(
            !deltas
                .deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        emitter.finish().await;
    }

    /// App-side drain proof for the armed worker's pre-finish event sequence.
    /// Worker ordering is pinned in the core tests; here authentic mutation and
    /// seal events must reach the reducer before the acknowledgement is read.
    #[tokio::test]
    async fn last_window_ack_observes_committed_sealed_partial_before_finish_event() {
        let mut take = live_take("stop-ack");
        take.emitter.on_capture_opened("stop-ack", 7);
        let occurrence = OccurrenceIdentity::new("stop-ack", 7, 0, 16_000);
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let finish_events = AtomicUsize::new(0);
        let producer = async {
            take.admit(&occurrence, 1, "Iwo at stop");
            take.seal(&occurrence);
            ack_tx.send(()).unwrap();
            finish_rx.await.unwrap();
            finish_events.fetch_add(1, Ordering::SeqCst);
            take.emitter.on_event(&raw_final("Iwo after finish"));
        };
        let consumer = async {
            ack_rx.await.unwrap();
            assert_eq!(finish_events.load(Ordering::SeqCst), 0);
            {
                let reducer = take.emitter.session_state.lock().unwrap();
                assert!(reducer.committed_rendered_text().contains("Iwo at stop"));
                let entry = reducer.document_by_occurrence.get(&occurrence).unwrap();
                assert_eq!(entry.label, "Iwo at stop");
                assert!(entry.seal_receipt.is_some());
            }
            let frozen = take.emitter.visible_canvas_snapshot().unwrap();
            assert!(frozen.has_committed_document);
            assert_eq!(frozen.preview_only_words, 0);
            assert_eq!(frozen.qualified_occurrences, 1);
            finish_tx.send(()).unwrap();
        };
        tokio::join!(producer, consumer);
        assert_eq!(finish_events.load(Ordering::SeqCst), 1);
        take.emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_canvas_keeps_preview_only_words_raw_without_a_user_revision() {
        let (mut emitter, delivery, ledger) = opened_delivery_take();
        emitter.on_event(&partial_mirror(1, "one two three four five", 0, 16_000));
        let frozen = emitter.visible_canvas_snapshot().expect("opened take");
        assert_eq!(frozen.session_id, "take");
        assert_eq!(frozen.capture_epoch, 7);
        assert_eq!(frozen.revision, 0);
        assert_eq!(frozen.text, "one two three four five");
        assert_eq!(frozen.preview_only_words, 5);
        assert!(!frozen.has_committed_document);
        assert_eq!(frozen.qualified_occurrences, 0);
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
        assert!(delivery.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stop_canvas_with_committed_and_preview_words_skips_light_plus() {
        let (mut emitter, delivery, ledger) = opened_delivery_take();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "committed words",
        );
        emitter.on_event(&mutation);
        let committed = emitter.visible_canvas_snapshot().unwrap();
        emitter.on_event(&partial_mirror(2, "preview words", 16_000, 32_000));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.text, format!("{} preview words", committed.text));
        assert_eq!(frozen.preview_only_words, 2);
        assert!(frozen.has_committed_document);
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), committed.text);
    }

    #[tokio::test]
    async fn stop_canvas_counts_only_visible_unanchored_words_as_preview() {
        let (mut emitter, _delivery, ledger) = opened_delivery_take();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "committed words",
        );
        emitter.on_event(&mutation);
        for (request, start, end, label) in [
            (2, 0, 8_000, "covered evidence"),
            (3, 16_000, 32_000, "visible evidence"),
        ] {
            let observation = ObservationIdentity::new(
                ObservationProducer::Apple,
                request,
                0,
                OccurrenceIdentity::new("take", 7, start, end),
            );
            let receipt = ledger.lock().unwrap().keep_visible_unanchored(
                &observation,
                label,
                codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
            );
            emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: label.to_string(),
                receipt,
            });
        }
        emitter.on_event(&partial_mirror(4, "last preview", 32_000, 48_000));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 4);
        assert!(frozen.text.ends_with("visible evidence last preview"));
        assert!(!frozen.text.contains("covered evidence"));
        assert!(
            emitter
                .session_state
                .lock()
                .unwrap()
                .unanchored_evidence("take", 7)
                .iter()
                .any(|item| item.text == "covered evidence")
        );
        let accounted = frozen.missing_words_from(&frozen);
        assert_eq!(accounted.len(), 2);
        assert!(
            accounted
                .iter()
                .all(|word| word.reason.starts_with("covered_by_committed occurrence="))
        );
        assert_eq!(
            emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap(),
            frozen
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_canvas_speech_count_is_capture_filtered_without_document_text() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        for (session, epoch) in [("take", 7), ("take", 8), ("another-take", 7)] {
            let _ = admitted_mutation(
                &mut ledger.lock().unwrap(),
                OccurrenceIdentity::new(session, epoch, 0, 16_000),
                1,
                "speech",
            );
        }
        let mut emitter = emitter_on_ledger(&ledger);
        emitter.on_capture_opened("take", 7);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert!(frozen.text.is_empty());
        assert!(!frozen.has_committed_document);
        assert_eq!(frozen.qualified_occurrences, 1);
        assert_eq!(ledger.lock().unwrap().qualified_occurrences().count(), 3);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn frozen_stop_canvas_publishes_the_exact_light_plus_paste() {
        let fixture = BusFixture::new("paste-take", true, "paste.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        let occurrence = OccurrenceIdentity::new("paste-take", 7, 0, 16_000);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            occurrence,
            1,
            "to działa bo jest proste",
        );
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_capture_opened("paste-take", 7);
        emitter.on_event(&mutation);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        let paste = emitter.shape_frozen_canvas_at_stop(frozen.clone()).unwrap();
        assert_eq!(
            paste.text,
            codescribe_core::pipeline::light_plus::apply(&frozen.text)
        );
        assert!(paste.revision > frozen.revision);
        assert_eq!(
            ledger.lock().unwrap().manual_document_revisions()[0].provenance,
            "light-plus"
        );
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), paste.text);
        let bus_text = std::fs::read_to_string(bus_path).unwrap();
        assert!(bus_text.contains("light-plus"));
        assert!(bus_text.contains(&paste.text));
    }

    #[test]
    fn stop_preserves_all_existing_bytes_even_with_a_long_unshaped_owner() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let owner = OccurrenceIdentity::new("stop-boundary-only", 7, 0, 960_000);
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted_mutation(
            &mut ledger,
            owner,
            1,
            "pierwszy fragment po to aby ostatni fragment",
        )
        else {
            unreachable!()
        };
        reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();
        let before = reducer.committed_rendered_text();
        let intent = reducer.light_plus_intent(&ledger, 0.8).unwrap();
        assert_eq!(intent.rendered_text, format!("{before}."));
        reducer.mark_terminal_lifecycle();
        reducer.apply_user_revision(&mut ledger, &intent).unwrap();
        assert!(reducer.light_plus_intent(&ledger, 0.8).is_none());
        assert_eq!(reducer.committed_rendered_text(), format!("{before}."));
    }

    // Counters observe real reducer work without changing production behavior.
    // Registration is per synthetic session, so parallel tests cannot affect it.
    #[derive(Clone, Copy, Default)]
    pub(super) struct WorkCounts {
        pub(super) shape_calls: usize,
        pub(super) context_bytes: usize,
        pub(super) owner_visits: usize,
        pub(super) stable_checks: usize,
    }
    static SHAPING_WORK: std::sync::LazyLock<
        std::sync::Mutex<std::collections::BTreeMap<String, WorkCounts>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
    struct WorkCounter(String);
    impl WorkCounter {
        fn register(session: &str) -> Self {
            assert!(
                SHAPING_WORK
                    .lock()
                    .unwrap()
                    .insert(session.into(), WorkCounts::default())
                    .is_none(),
                "unique test session"
            );
            Self(session.into())
        }
        fn snapshot(&self) -> WorkCounts {
            *SHAPING_WORK.lock().unwrap().get(&self.0).unwrap()
        }
    }
    impl Drop for WorkCounter {
        fn drop(&mut self) {
            SHAPING_WORK.lock().unwrap().remove(&self.0);
        }
    }
    pub(super) fn record_shaping_work(session: &str, update: impl FnOnce(&mut WorkCounts)) {
        if let Some(counts) = SHAPING_WORK.lock().unwrap().get_mut(session) {
            update(counts);
        }
    }

    #[tokio::test]
    async fn light_plus_stable_seal_has_no_repeat_shaping() {
        let count = 32_u64;
        let work = WorkCounter::register("light-plus-work-seal");
        let fixture = BusFixture::new("light-plus-work-seal", true, "synthetic.jsonl", "");
        fixture.bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_capture_opened("light-plus-work-seal", 7);
        let initial_before = work.snapshot().shape_calls;
        let mut owners = Vec::new();
        let mut labels = Vec::new();
        for i in 0..count {
            let owner =
                OccurrenceIdentity::new("light-plus-work-seal", 7, i * 16_000, (i + 1) * 16_000);
            let label = format!("słowo{i}");
            let event = admitted_mutation(
                &mut fixture.ledger.lock().unwrap(),
                owner.clone(),
                i + 1,
                &label,
            );
            emitter.on_event(&event);
            owners.push(owner);
            labels.push(label);
        }
        let initial_calls = work.snapshot().shape_calls - initial_before;
        assert!(
            initial_calls >= count as usize,
            "real normal admissions evaluated every source"
        );
        let before = work.snapshot().shape_calls;
        let context_before = work.snapshot().context_bytes;
        let owner = owners.last().unwrap();
        let receipt = {
            let mut ledger = fixture.ledger.lock().unwrap();
            ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(owner, ObservationProducer::Apple));
            ledger.seal(owner).expect("actual occurrence seal").clone()
        };
        assert!(receipt.is_occurrence_seal());
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
        let repeat = work.snapshot().shape_calls - before;
        let context_bytes = work.snapshot().context_bytes - context_before;
        let expected = format!("Słowo0 {}", labels[1..].join(" "));
        assert_eq!(emitter.visible_canvas_snapshot().unwrap().text, expected);
        {
            let ledger = fixture.ledger.lock().unwrap();
            for (owner, label) in owners.iter().zip(&labels) {
                assert_eq!(ledger.text_of(owner), Some(label.as_str()));
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
        emitter.finish().await;
        assert_eq!(*fixture.delivery.lock().await, expected);
        eprintln!(
            "LIGHT_PLUS_WORK_JSON {}",
            serde_json::json!({"owners":count,"initial_calls":initial_calls,"repeated_calls_after_unchanged_seal":repeat,"repeated_left_context_bytes":context_bytes,"actual_capture_or_asr":false})
        );
        assert_eq!(
            repeat, 0,
            "unchanged labels and left context must not rerun already evaluated Light+"
        );
    }

    #[tokio::test]
    async fn light_plus_noop_context_recomputes_after_real_human_predecessor_change() {
        let mut take = live_take("light-plus-human-context");
        take.emitter
            .on_capture_opened("light-plus-human-context", 3);
        let head = OccurrenceIdentity::new("light-plus-human-context", 3, 0, 16_000);
        let tail = OccurrenceIdentity::new("light-plus-human-context", 3, 16_000, 32_000);
        take.admit(&head, 1, "Iwo");
        take.admit(&tail, 2, "dalej");
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo dalej"
        );
        take.seal(&tail);
        let event = {
            let mut ledger = take.ledger.lock().unwrap();
            let observation =
                ObservationIdentity::new(ObservationProducer::ManualHuman, 9, 0, head.clone());
            let receipt = ledger.admit(&observation, "Iwo.");
            assert!(receipt.grants_mutation());
            EngineEvent::LedgerMutation {
                observation,
                label: "Iwo.".into(),
                receipt,
            }
        };
        take.emitter.on_event(&event);
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo. Dalej"
        );
        {
            let ledger = take.ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&head), Some("Iwo."));
            assert_eq!(ledger.text_of(&tail), Some("dalej"));
            assert_eq!(ledger.len(), 2);
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
            assert!(
                ledger
                    .incremental_shapings()
                    .iter()
                    .any(|s| s.occurrence == tail
                        && s.left_context == "Iwo."
                        && s.shaped_text == "Dalej")
            );
        }
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, "Iwo. Dalej");
    }
    #[tokio::test]
    async fn light_plus_late_gap_fill_invalidates_presentation_without_rewriting_history() {
        let mut take = live_take("light-plus-gap-context");
        take.emitter.on_capture_opened("light-plus-gap-context", 3);
        let head = OccurrenceIdentity::new("light-plus-gap-context", 3, 0, 16_000);
        let tail = OccurrenceIdentity::new("light-plus-gap-context", 3, 48_000, 64_000);
        let middle = OccurrenceIdentity::new("light-plus-gap-context", 3, 16_000, 48_000);
        take.admit(&head, 1, "Iwo");
        take.admit(&tail, 3, "dalej");
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo. Dalej"
        );
        let history = take.shapings();
        assert_eq!(history.len(), 1);
        assert!(history[0].sentence_break_before);
        take.admit(&middle, 2, "i mowa trwa");
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo i mowa trwa dalej"
        );
        assert_eq!(
            &take.shapings()[..history.len()],
            history.as_slice(),
            "immutable old shaping receipts remain inspectable"
        );
        {
            let ledger = take.ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&head), Some("Iwo"));
            assert_eq!(ledger.text_of(&middle), Some("i mowa trwa"));
            assert_eq!(ledger.text_of(&tail), Some("dalej"));
            assert_eq!(ledger.len(), 3);
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, "Iwo i mowa trwa dalej");
    }

    #[tokio::test]
    async fn light_plus_stable_last_seal_visits_only_its_bounded_owner() {
        let count = 32_u64;
        let work = WorkCounter::register("light-plus-work-owner");
        let fixture = BusFixture::new("light-plus-work-owner", true, "synthetic.jsonl", "");
        fixture.bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_capture_opened("light-plus-work-owner", 7);
        let initial_before = work.snapshot().shape_calls;
        let mut owners = Vec::new();
        let mut labels = Vec::new();
        for i in 0..count {
            let owner =
                OccurrenceIdentity::new("light-plus-work-owner", 7, i * 16_000, (i + 1) * 16_000);
            let label = format!("słowo{i}");
            let event = admitted_mutation(
                &mut fixture.ledger.lock().unwrap(),
                owner.clone(),
                i + 1,
                &label,
            );
            emitter.on_event(&event);
            owners.push(owner);
            labels.push(label);
        }
        let initial_calls = work.snapshot().shape_calls - initial_before;
        assert!(
            initial_calls >= count as usize,
            "real normal admissions evaluated every source"
        );
        let before = work.snapshot().owner_visits;
        let stable_before = work.snapshot().stable_checks;
        let context_before = work.snapshot().context_bytes;
        let owner = owners.last().unwrap();
        let receipt = {
            let mut ledger = fixture.ledger.lock().unwrap();
            ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(owner, ObservationProducer::Apple));
            ledger.seal(owner).expect("actual occurrence seal").clone()
        };
        assert!(receipt.is_occurrence_seal());
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
        let repeat = work.snapshot().owner_visits - before;
        let context_bytes = work.snapshot().context_bytes - context_before;
        let expected = format!("Słowo0 {}", labels[1..].join(" "));
        assert_eq!(emitter.visible_canvas_snapshot().unwrap().text, expected);
        {
            let ledger = fixture.ledger.lock().unwrap();
            for (owner, label) in owners.iter().zip(&labels) {
                assert_eq!(ledger.text_of(owner), Some(label.as_str()));
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
        emitter.finish().await;
        assert_eq!(*fixture.delivery.lock().await, expected);
        eprintln!(
            "LIGHT_PLUS_WORK_JSON {}",
            serde_json::json!({"owners":count,"initial_calls":initial_calls,"repeated_calls_after_unchanged_seal":repeat,"repeated_left_context_bytes":context_bytes,"actual_capture_or_asr":false})
        );
        let stable_checks = work.snapshot().stable_checks - stable_before;
        eprintln!("LIGHT_PLUS_STABILITY_CHECKS {stable_checks}");
        assert!(
            stable_checks <= 1,
            "bounded seal must not scan stable owners: {stable_checks}"
        );
        assert!(
            repeat <= 1,
            "unchanged last-occurrence seal must not visit all previous owners: {repeat}"
        );
    }

    #[tokio::test]
    async fn sequential_formatter_attempts_keep_raw_and_advance_document_cas() {
        let mut take = live_take("light-plus-derived");
        take.emitter.on_capture_opened("light-plus-derived", 3);
        let owner = OccurrenceIdentity::new("light-plus-derived", 3, 0, 16_000);
        take.admit(&owner, 1, "Iwo yyy plan");
        take.seal(&owner);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: owner.session.clone(),
            layer_summary: LayerSummary::default(),
        });
        let source = take.emitter.terminal_formatter_request().unwrap();
        let first_sequence = take.projected.lock().unwrap().last().unwrap().sequence;
        for text in ["Smart first", "Smart second"] {
            take.emitter
                .apply_formatter_revision(
                    source.session_id.clone(),
                    take.emitter.document_timeline(&source.session_id).0,
                    FormattingPolicy::Smart,
                    AiFormatResult {
                        text: text.into(),
                        reasoning_text: None,
                        status: AiFormatStatus::Applied,
                    },
                )
                .unwrap();
        }
        {
            let events = take.projected.lock().unwrap();
            let derived = events
                .iter()
                .filter(|e| e.reducer_action == "derived_projection")
                .collect::<Vec<_>>();
            assert_eq!(
                derived.len(),
                2,
                "derived updates are not duplicate Raw paints"
            );
            assert!(
                derived
                    .iter()
                    .enumerate()
                    .all(|(index, e)| e.reducer_revision
                        == source.source_revision + index as u64 + 1
                        && e.rendered_text == source.source_text
                        && !e.lifecycle_terminal)
            );
            assert_eq!(derived[0].delivery_text.as_deref(), Some("Smart first"));
            assert_eq!(derived[1].delivery_text.as_deref(), Some("Smart second"));
            assert!(
                derived[0].sequence > first_sequence && derived[1].sequence > derived[0].sequence
            );
        }
        let current = take.emitter.terminal_formatter_request().unwrap();
        assert_eq!(current.source_revision, source.source_revision + 2);
        assert_eq!(current.source_text, source.source_text);
        assert_eq!(
            take.ledger.lock().unwrap().text_of(&owner),
            Some("Iwo yyy plan")
        );
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, source.source_text);
    }

    #[tokio::test]
    async fn light_plus_noop_evaluation_recomputes_when_sentence_pause_changes() {
        let mut take = live_take("light-plus-pause");
        take.emitter.on_capture_opened("light-plus-pause", 3);
        let head = OccurrenceIdentity::new("light-plus-pause", 3, 0, 16_000);
        let tail = OccurrenceIdentity::new("light-plus-pause", 3, 24_000, 40_000);
        take.admit(&head, 1, "Iwo");
        take.admit(&tail, 2, "dalej");
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo dalej"
        );
        take.seal(&tail);
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo dalej"
        );
        take.emitter = take.emitter.with_sentence_pause_sec(0.3);
        let next = OccurrenceIdentity::new("light-plus-pause", 3, 40_000, 56_000);
        take.admit(&next, 3, "tak");
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo. Dalej tak",
            "a new authenticated admission evaluates changed pause threshold"
        );
        {
            let ledger = take.ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&head), Some("Iwo"));
            assert_eq!(ledger.text_of(&tail), Some("dalej"));
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
            assert!(
                ledger
                    .incremental_shapings()
                    .iter()
                    .any(|s| s.occurrence == tail && s.sentence_break_before)
            );
        }
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, "Iwo. Dalej tak");
    }

    #[tokio::test]
    async fn late_mutation_after_frozen_paste_remains_a_bus_revision() {
        let fixture = BusFixture::new("late-paste", true, "late.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_capture_opened("late-paste", 7);
        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("late-paste", 7, 0, 16_000),
            1,
            "pierwsze słowa",
        );
        emitter.on_event(&first);
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        let paste = emitter.shape_frozen_canvas_at_stop(frozen).unwrap();
        assert_eq!(paste.text, "Pierwsze słowa.");

        let late = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("late-paste", 7, 16_000, 32_000),
            2,
            "drugie słowa",
        );
        emitter.on_event(&late);
        let revised = emitter.visible_canvas_snapshot().unwrap();
        assert!(revised.revision > paste.revision);
        assert_eq!(revised.text, "Pierwsze słowa drugie słowa.");
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), revised.text);
        let bus_text = std::fs::read_to_string(bus_path).unwrap();
        assert!(bus_text.contains(&paste.text));
        assert!(bus_text.contains(&revised.text));
    }

    #[tokio::test]
    async fn frozen_stop_canvas_remains_a_prefix_of_five_occurrence_revision() {
        let (mut emitter, delivery, ledger) = opened_delivery_take();
        for index in 0..5 {
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                admitted_mutation(
                    &mut ledger,
                    OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                    index + 1,
                    "Iwo",
                )
            };
            emitter.on_event(&mutation);
            if index == 0 {
                let frozen = emitter.visible_canvas_snapshot().unwrap();
                assert_eq!(frozen.revision, 1);
                assert_eq!(frozen.text, "Iwo");
            }
        }
        let revised = emitter.visible_canvas_snapshot().unwrap();
        emitter.finish().await;
        assert_eq!(revised.text, "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(revised.revision, 5);
        assert_eq!(revised.qualified_occurrences, 5);
        assert_eq!(delivery.lock().await.as_str(), revised.text);
        assert_eq!(ledger.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn stop_canvas_includes_the_third_occurrence_open_at_release() {
        let (mut emitter, _delivery, ledger) = opened_delivery_take();
        for index in 0..2 {
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                admitted_mutation(
                    &mut ledger,
                    OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                    index + 1,
                    "Iwo",
                )
            };
            emitter.on_event(&mutation);
        }
        emitter.on_event(&partial_mirror(3, "Iwo", 32_000, 48_000));
        // Stop paste v2 (R3): the open third word is overlay-visible at stop,
        // but only as preview, without occurrence authority.
        let open = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(open.text, "Iwo Iwo Iwo");
        assert_eq!(open.preview_only_words, 1);
        let last = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("take", 7, 32_000, 48_000),
                3,
                "Iwo",
            )
        };
        emitter.on_event(&last);
        emitter.on_event(&closed_mirror(
            4,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            1,
        ));
        emitter.on_event(&preview_disposition(3, PreviewFinalDisposition::Admitted));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 0);
        assert_eq!(
            frozen.text, "Iwo Iwo Iwo",
            "last occurrence missing at stop"
        );
        assert_eq!(frozen.revision, 3);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn single_occurrence_closed_at_release_is_the_first_deliverable_canvas() {
        let (mut emitter, _delivery, ledger) = opened_delivery_take();
        emitter.on_event(&partial_mirror(1, "last words", 0, 16_000));
        // Stop paste v2 (R3): preview-only words are visible at stop, with no
        // committed document behind them.
        let open = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(open.text, "last words");
        assert_eq!(open.preview_only_words, 2);
        assert!(!open.has_committed_document);
        let last = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("take", 7, 0, 16_000),
                1,
                "last words",
            )
        };
        emitter.on_event(&last);
        emitter.on_event(&closed_mirror(
            2,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            2,
        ));
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.preview_only_words, 0);
        assert_eq!(frozen.text, "Last words");
        assert_eq!(frozen.revision, 2);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_snapshot_equals_the_last_overlay_paint() {
        let (mut emitter, delivery, ledger) = opened_delivery_take();
        let mut paint_count = 0;
        let mut assert_paint = |expected_text: &str, expected_preview_words: usize| {
            let frozen = emitter.visible_canvas_snapshot().unwrap();
            let paints = emitter.paint_commands.lock().unwrap();
            assert!(paints.len() > paint_count, "event must dispatch a paint");
            paint_count = paints.len();
            assert_eq!(&frozen.text, paints.last().unwrap());
            assert_eq!(frozen.text, expected_text);
            assert_eq!(frozen.preview_only_words, expected_preview_words);
        };

        // Longer than the compact cursor's five-word tail: compare full paints.
        emitter.on_event(&partial_mirror(1, "One two three four five six", 0, 16_000));
        assert_paint("One two three four five six", 6);

        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "One two three four five six",
        );
        emitter.on_event(&first);
        emitter.on_event(&closed_mirror(
            2,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            6,
        ));
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        assert_paint("One two three four five six", 0);

        emitter.on_event(&partial_mirror(3, "open tail", 16_000, 32_000));
        assert_paint("One two three four five six open tail", 2);

        let tail = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, tail.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "unanchored words",
            codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "unanchored words".into(),
            receipt,
        });
        emitter.on_event(&closed_mirror(
            4,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Unmatched,
            2,
        ));
        emitter.on_event(&preview_disposition(
            2,
            PreviewFinalDisposition::KeptUnanchored,
        ));
        assert_paint("One two three four five six unanchored words", 2);

        let last = admitted_mutation(&mut ledger.lock().unwrap(), tail, 3, "settled tail");
        emitter.on_event(&last);
        assert_paint("One two three four five six settled tail", 0);
        emitter.finish().await;
        assert_eq!(
            delivery.lock().await.as_str(),
            "One two three four five six settled tail"
        );
    }

    /// W2-08: a take's refused overlap pins are counted by ledger reason.
    /// Only the take's first refusal stays a warning, and finalisation
    /// drains the counts for the next take.
    #[tokio::test]
    async fn overlap_refusals_are_counted_by_ledger_reason() {
        use codescribe_core::pipeline::contracts;
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = emitter_on_ledger(&ledger);
        let code = contracts::OVERLAP_PIN_REFUSED;
        let refusal = |reason| contracts::overlap_pin_refused_message((0, 16), "0..16", reason);
        let replay = refusal("replayed_range_identity");
        let silent = refusal("no_voiced_hop_in_pin");

        assert!(!emitter.overlap_refusal_repeats(code, &replay));
        assert!(emitter.overlap_refusal_repeats(code, &replay));
        assert!(emitter.overlap_refusal_repeats(code, &silent));
        let other = emitter.overlap_refusal_repeats("tail_patch_refused", &replay);
        assert!(!other, "only overlap refusals are counted");
        let counts = emitter.overlap_refusals.lock().unwrap().clone();
        assert_eq!(counts.get("replayed_range_identity"), Some(&2));
        assert_eq!(counts.get("no_voiced_hop_in_pin"), Some(&1));
        assert_eq!(counts.len(), 2);

        emitter.report_overlap_refusals();
        assert!(emitter.overlap_refusals.lock().unwrap().is_empty());
        assert!(!emitter.overlap_refusal_repeats(code, &silent));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn mutation_without_paint_keeps_the_preview_in_the_stop_snapshot() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = emitter_on_ledger(&ledger);
        emitter.on_capture_opened("take", 7);
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let first = admitted_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), 1, "Iwo");
        emitter.on_event(&first);
        let seal = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            ledger.seal(&occurrence).unwrap().clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&mirror(
            2,
            mirror_words(
                "next words",
                16_000,
                32_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 2,
                    phrase_id: 1,
                },
            ),
        ));
        let before = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(before.text, "Iwo next words");
        assert_eq!(before.preview_only_words, 2);
        let paint_count = emitter.paint_commands.lock().unwrap().len();

        // A late machine observation over a sealed occurrence has no mutation
        // authority. This is a ledger-issued refusal, not injected reducer state.
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence);
        let receipt = ledger.lock().unwrap().admit(&observation, "changed words");
        assert!(!receipt.grants_mutation());
        assert!(!matches!(
            receipt,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "changed words".into(),
            receipt,
        });
        assert_eq!(emitter.paint_commands.lock().unwrap().len(), paint_count);
        let after = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(after, before);
        assert_eq!(
            &after.text,
            emitter.paint_commands.lock().unwrap().last().unwrap()
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_snapshot_zero_width_paint_includes_unanchored_words() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let mut emitter = emitter_on_ledger(&ledger);
        emitter.on_capture_opened("take", 7);
        let first = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "Iwo",
        );
        emitter.on_event(&first);
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            2,
            0,
            OccurrenceIdentity::new("take", 7, 16_000, 32_000),
        );
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "unanchored words",
            codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "unanchored words".into(),
            receipt,
        });
        assert_eq!(
            emitter
                .visible_canvas_snapshot()
                .unwrap()
                .preview_only_words,
            2
        );
        emitter.on_event(&mirror(
            3,
            mirror_words(
                "side evidence",
                32_000,
                32_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 3,
                    phrase_id: 1,
                },
            ),
        ));
        let frozen = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(
            &frozen.text,
            emitter.paint_commands.lock().unwrap().last().unwrap()
        );
        assert_eq!(frozen.text, "Iwo unanchored words side evidence");
        assert_eq!(frozen.preview_only_words, 4);
        // A committed paint at EOF must still record the visible evidence.
        emitter.send_committed_paint("Iwo".into());
        assert_eq!(emitter.visible_canvas_snapshot().unwrap(), frozen);
        emitter.finish().await;
    }

    fn preview_disposition(rev: u64, disposition: PreviewFinalDisposition) -> EngineEvent {
        EngineEvent::PreviewDisposition {
            superseded_through_rev: rev,
            final_disposition: disposition,
            refused_evidence: Vec::new(),
        }
    }

    /// Dictation bus session. `latched_target` is the only field these fixtures vary.
    fn dictation_session(session_id: impl Into<String>, latched_target: bool) -> TranscriptSession {
        TranscriptSession {
            session_id: session_id.into(),
            mode: TranscriptMode::Dictation,
            has_latched_target: latched_target,
            latched_target_is_self: false,
            audience: None,
            badge_only: false,
        }
    }

    /// Resource ownership only: each scenario still performs every lifecycle and ledger step.
    struct BusFixture {
        _temp: tempfile::TempDir,
        path: std::path::PathBuf,
        bus: Arc<TranscriptBus>,
        ledger: Arc<StdMutex<AcousticLedger>>,
        delivery: Arc<Mutex<String>>,
    }

    impl BusFixture {
        fn new(session: &str, latched_target: bool, file: &str, initial_delivery: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(file);
            let bus = Arc::new(
                TranscriptBus::open_at(
                    dictation_session(session, latched_target),
                    path.clone(),
                    None,
                )
                .unwrap(),
            );
            Self {
                _temp: temp,
                path,
                bus,
                ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
                delivery: Arc::new(Mutex::new(initial_delivery.into())),
            }
        }

        fn emitter(
            &self,
            deltas: Option<Arc<dyn DeltaSink>>,
            observer: Option<super::ProjectionObserver>,
        ) -> PresentationEmitter {
            PresentationEmitter::new_with_authority(
                Arc::clone(&self.delivery),
                deltas,
                None,
                Some(Arc::clone(&self.bus)),
                Some(Arc::clone(&self.ledger)),
                observer,
            )
        }
    }

    /// Empty delivery buffer bound to an existing ledger. No bus, sink, or callback.
    fn emitter_on_ledger(ledger: &Arc<StdMutex<AcousticLedger>>) -> PresentationEmitter {
        PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(ledger)),
            None,
        )
    }

    /// Literal-delivery mirror opened on `session` at capture epoch 7.
    fn mirror_session(session: &str) -> (PresentationEmitter, Arc<StdMutex<AcousticLedger>>) {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let emitter = emitter_on_ledger(&ledger);
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened(session, 7);
        (emitter, ledger)
    }

    fn mirror_take() -> (PresentationEmitter, Arc<StdMutex<AcousticLedger>>) {
        mirror_session("take")
    }

    /// Delivery buffer and ledger, opened on take/7, with literal delivery left off.
    fn opened_delivery_take() -> (
        PresentationEmitter,
        Arc<Mutex<String>>,
        Arc<StdMutex<AcousticLedger>>,
    ) {
        let delivery = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.on_capture_opened("take", 7);
        (emitter, delivery, ledger)
    }

    #[tokio::test]
    async fn timed_replays_do_not_double_and_zero_width_repetition_is_counted() {
        let (mut emitter, _) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "Iwo Iwo",
                0,
                16_000,
                UnadmittedAppleWordSource::Pending { utterance_id: 1 },
            ),
        ));
        assert_eq!(emitter.visible_canvas_snapshot().unwrap().text, "Iwo");
        emitter.on_event(&mirror(
            2,
            mirror_words(
                "Iwo Iwo",
                20_000,
                20_000,
                UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
            ),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        assert_eq!(stopped.text, "Iwo Iwo");
        emitter.on_event(&mirror(
            3,
            mirror_words(
                "Iwo",
                20_000,
                20_000,
                UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
            ),
        ));
        let reasons = stopped.missing_words_from(&emitter.finish_stop_canvas().unwrap());
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].reason, "unaccounted");
        emitter.finish().await;
    }

    fn numbered_words(prefix: &str, count: usize) -> String {
        (0..count)
            .map(|index| format!("{prefix}_{index:03}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Receipt 6bba5e1d: a formatted final and the raw preview of the same
    /// speech. The open partial sits on a stuck pin and leaves through phrase
    /// close before the stop snapshot. The unmatched replay remains inside the
    /// committed extent, so the paste keeps the formatted document once.
    #[tokio::test]
    async fn formatted_final_retires_raw_preview_on_the_same_span() {
        let (mut emitter, ledger) = mirror_take();
        let raw = "kazda wersja jest gorsza";
        let formatted = "Każda wersja jest gorsza.";
        emitter.on_event(&mirror(
            1,
            mirror_words(
                raw,
                48_000,
                48_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 4,
                    phrase_id: 9,
                },
            ),
        ));
        emitter.on_event(&closed_mirror(
            2,
            mirror_words(raw, 48_000, 58_000, UnadmittedAppleWordSource::Unmatched),
            9,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            raw.split_whitespace().count(),
        ));
        let painted = emitter.begin_stop_canvas().unwrap();
        assert!(painted.text.contains("kazda"));
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 58_368, 336_384),
            1,
            formatted,
        );
        emitter.on_event(&mutation);
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, formatted);
        for token in ["Każda", "wersja", "jest", "gorsza."] {
            assert_eq!(pasted.text.matches(token).count(), 1, "{token}");
        }
        assert!(!pasted.text.to_lowercase().contains("kazda"));
        let reasons = painted.missing_words_from(&pasted);
        assert!(!reasons.is_empty());
        assert!(
            reasons
                .iter()
                .all(|word| word.reason.starts_with("admitted_into occurrence=")),
            "{reasons:?}"
        );
        emitter.finish().await;
    }

    /// A final that adds a tail past the last committed sample keeps that tail
    /// once, and keeps every formatted word.
    #[tokio::test]
    async fn formatted_final_keeps_a_genuine_tail_word() {
        let (mut emitter, ledger) = mirror_take();
        let raw = "kazda wersja jest gorsza";
        let formatted = "Każda wersja jest gorsza.";
        let mut words = mirror_words(raw, 48_000, 58_000, UnadmittedAppleWordSource::Unmatched);
        words.extend(mirror_words(
            "ogonek",
            2_200_000,
            2_216_000,
            UnadmittedAppleWordSource::Unmatched,
        ));
        emitter.on_event(&mirror(1, words));
        let painted = emitter.visible_canvas_snapshot().unwrap();
        assert!(painted.text.contains("ogonek"));
        assert!(painted.text.contains("kazda"));
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 58_368, 336_384),
            1,
            formatted,
        );
        emitter.on_event(&mutation);
        let pasted = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(pasted.text, format!("{formatted} ogonek"));
        assert_eq!(pasted.text.matches("ogonek").count(), 1);
        for token in formatted.split_whitespace() {
            assert_eq!(pasted.text.matches(token).count(), 1, "{token}");
        }
        assert!(!pasted.text.to_lowercase().contains("kazda"));
        emitter.finish().await;
    }

    /// Take 6bba5e1d painted 178 words and pasted one copy. Production had
    /// already closed the open partial; the doubling was the unmatched replay.
    /// That replay must disappear, and every formatted word must remain.
    #[tokio::test]
    async fn receipt_6bba5e1d_paints_one_copy_without_losing_formatted_words() {
        let (mut emitter, ledger) = mirror_take();
        let raw = numbered_words("raw", 178);
        let formatted = numbered_words("fin", 69);
        emitter.on_event(&mirror(
            1,
            mirror_words(
                &raw,
                48_000,
                48_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 1,
                    phrase_id: 7,
                },
            ),
        ));
        emitter.on_event(&closed_mirror(
            2,
            mirror_words(&raw, 48_000, 58_000, UnadmittedAppleWordSource::Unmatched),
            7,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            178,
        ));
        let painted = emitter.begin_stop_canvas().unwrap();
        assert_eq!(painted.text.split_whitespace().count(), 178);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 58_368, 336_384),
            1,
            &formatted,
        );
        emitter.on_event(&mutation);
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, formatted);
        assert_eq!(pasted.text.split_whitespace().count(), 69);
        assert!(!pasted.text.contains("raw_000"));
        assert!(!pasted.text.contains("raw_177"));
        for token in formatted.split_whitespace() {
            assert_eq!(pasted.text.matches(token).count(), 1, "{token}");
        }
        let reasons = painted.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 178);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason.starts_with("admitted_into occurrence=")),
            "{:?}",
            reasons.first()
        );
        emitter.finish().await;
    }

    /// An explicit archival insert stays the paste. The open partial of the
    /// same span closes before the snapshot, so it does not append a second copy.
    #[tokio::test]
    async fn manual_insert_of_archival_text_is_not_doubled_by_raw_preview() {
        let (mut emitter, ledger) = mirror_take();
        let archival = "Archiwalna transkrypcja do wstawienia";
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "Tekst bazowy",
        );
        emitter.on_event(&mutation);
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "take".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let revision = emitter.visible_canvas_snapshot().unwrap().revision;
        emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "take".to_string(),
                source_revision: revision,
                rendered_text: archival.to_string(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .expect("archival insert commits");
        emitter.on_event(&partial_mirror(2, "surowy duplikat", 0, 0));
        emitter.on_event(&closed_mirror(
            3,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            2,
        ));
        let pasted = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(pasted.text, archival);
        assert!(!pasted.text.contains("surowy"));
        assert!(!pasted.text.contains("duplikat"));
        emitter.finish().await;
    }

    /// A stuck open-partial pin is inside the committed extent and still paints
    /// new words. An unmatched replay on that same extent restates the document
    /// and leaves the overlay.
    #[tokio::test]
    async fn stale_open_partial_keeps_new_words_after_commit() {
        let (mut emitter, ledger) = mirror_take();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 64_000),
            1,
            "committed",
        );
        emitter.on_event(&mutation);
        let mut words = mirror_words(
            "fresh speech",
            0,
            0,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 4,
                phrase_id: 3,
            },
        );
        words.extend(mirror_words(
            "later phrase",
            8_000,
            12_000,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 4,
                phrase_id: 4,
            },
        ));
        words.extend(mirror_words(
            "replayed",
            1_000,
            8_000,
            UnadmittedAppleWordSource::Unmatched,
        ));
        emitter.on_event(&mirror(1, words));
        let snapshot = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(snapshot.text, "committed fresh speech later phrase");
        assert!(!snapshot.text.contains("replayed"));
        assert_eq!(snapshot.preview_only_words, 4);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn ledger_evidence_and_mirror_overlap_is_one_identity() {
        let (mut emitter, ledger) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words("evidence", 0, 16_000, UnadmittedAppleWordSource::Unmatched),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            1,
            0,
            OccurrenceIdentity::new("take", 7, 0, 16_000),
        );
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "evidence",
            super::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "evidence".into(),
            receipt,
        });
        assert_eq!(emitter.visible_canvas_snapshot().unwrap().text, "evidence");
        emitter.on_event(&mirror(2, Vec::new()));
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "evidence");
        assert!(stopped.missing_words_from(&pasted).is_empty());
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_reads_ranged_sources_then_untimed_then_open_partial() {
        let (mut emitter, ledger) = mirror_take();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "committed",
        );
        emitter.on_event(&mutation);
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            2,
            0,
            OccurrenceIdentity::new("take", 7, 16_000, 32_000),
        );
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "evidence",
            super::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "evidence".into(),
            receipt,
        });
        let mut words = mirror_words(
            "pending",
            32_000,
            48_000,
            UnadmittedAppleWordSource::Pending { utterance_id: 2 },
        );
        words.extend(mirror_words(
            "unmatched",
            48_000,
            64_000,
            UnadmittedAppleWordSource::Unmatched,
        ));
        words.extend(mirror_words(
            "partial",
            64_000,
            80_000,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 1,
                phrase_id: 1,
            },
        ));
        words.extend(mirror_words(
            "untimed",
            80_000,
            80_000,
            UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
        ));
        emitter.on_event(&mirror(1, words));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(
            pasted.text,
            "committed evidence pending unmatched untimed partial"
        );
        assert_eq!(pasted.preview_only_words, 5);
        assert_eq!(pasted.untimed_final_words, 1);
        assert!(stopped.missing_words_from(&pasted).is_empty());
        emitter.finish().await;
    }

    #[tokio::test]
    async fn ledger_then_mirror_then_receipt_reads_each_word_once() {
        let (mut emitter, ledger) = mirror_take();
        let mut words = mirror_words(
            "admitted",
            0,
            16_000,
            UnadmittedAppleWordSource::Pending { utterance_id: 1 },
        );
        let unmatched = mirror_words(
            "unmatched",
            16_000,
            32_000,
            UnadmittedAppleWordSource::Unmatched,
        );
        words.extend(unmatched.clone());
        emitter.on_event(&mirror(1, words));
        let stopped = emitter.begin_stop_canvas().unwrap();
        assert_eq!(stopped.text, "admitted unmatched");
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "admitted",
        );
        emitter.on_event(&mutation);
        let between_ledger_and_mirror = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(between_ledger_and_mirror.text, "admitted unmatched");
        assert_eq!(between_ledger_and_mirror.preview_only_words, 1);
        emitter.on_event(&mirror(2, unmatched));
        let between_mirror_and_receipt = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(between_mirror_and_receipt.text, "admitted unmatched");
        emitter.on_event(&preview_disposition(1, PreviewFinalDisposition::Admitted));
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted, between_mirror_and_receipt);
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].reason.starts_with("admitted_into occurrence="));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn deferred_words_close_during_stop_without_duplicate_paste() {
        let (mut emitter, ledger) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "deferred words",
                0,
                16_000,
                UnadmittedAppleWordSource::Pending { utterance_id: 3 },
            ),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "deferred words",
        );
        emitter.on_event(&mutation);
        emitter.on_event(&mirror(2, Vec::new()));
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "deferred words");
        assert_eq!(pasted.preview_only_words, 0);
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 2);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason.starts_with("admitted_into occurrence="))
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn partial_shortening_accounts_only_for_words_absent_from_new_state() {
        for (start, end) in [(16_000, 32_000), (16_000, 16_000)] {
            let (mut emitter, _) = mirror_take();
            emitter.on_event(&mirror(
                1,
                mirror_words(
                    "one two three",
                    start,
                    end,
                    UnadmittedAppleWordSource::OpenPartial {
                        rev: 1,
                        phrase_id: 1,
                    },
                ),
            ));
            let stopped = emitter.begin_stop_canvas().unwrap();
            emitter.on_event(&mirror(
                2,
                mirror_words(
                    "one",
                    start,
                    end,
                    UnadmittedAppleWordSource::OpenPartial {
                        rev: 2,
                        phrase_id: 1,
                    },
                ),
            ));
            let pasted = emitter.finish_stop_canvas().unwrap();
            assert_eq!(pasted.text, "one");
            let reasons = stopped.missing_words_from(&pasted);
            assert_eq!(reasons.len(), 2);
            assert_eq!(
                reasons
                    .iter()
                    .map(|word| word.word.as_str())
                    .collect::<Vec<_>>(),
                ["two", "three"]
            );
            assert!(
                reasons
                    .iter()
                    .all(|word| word.reason == "superseded_by_partial rev=2")
            );
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn identical_words_keep_phrase_identity_across_revision_then_close() {
        let (mut emitter, _) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "same words",
                16_000,
                32_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 1,
                    phrase_id: 1,
                },
            ),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&mirror(
            2,
            mirror_words(
                "same words",
                16_000,
                32_000,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 2,
                    phrase_id: 1,
                },
            ),
        ));
        let revised = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(revised.text, stopped.text);
        assert!(stopped.missing_words_from(&revised).is_empty());
        emitter.on_event(&closed_mirror(
            3,
            mirror_words(
                "same words",
                16_000,
                32_000,
                UnadmittedAppleWordSource::Pending { utterance_id: 9 },
            ),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Pending,
            2,
        ));
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, stopped.text);
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 2);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason == "closed_by_final phrase=1 outcomes=pending=2")
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_accounting_does_not_borrow_the_same_text_from_another_range() {
        let (mut emitter, _) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words("Iwo", 0, 16_000, UnadmittedAppleWordSource::Unmatched),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&mirror(
            2,
            mirror_words("Iwo", 32_000, 48_000, UnadmittedAppleWordSource::Unmatched),
        ));
        let pasted = emitter.finish_stop_canvas().unwrap();
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].reason, "unaccounted");
        emitter.finish().await;
    }

    #[tokio::test]
    async fn mirror_replacement_keeps_pending_words_beside_unrelated_commit() {
        let (mut emitter, ledger) = mirror_take();
        let pending = mirror_words(
            "pending",
            32_000,
            48_000,
            UnadmittedAppleWordSource::Pending { utterance_id: 3 },
        );
        let mut first = pending.clone();
        first.extend(mirror_words(
            "old suffix",
            48_000,
            64_000,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 1,
                phrase_id: 1,
            },
        ));
        emitter.on_event(&mirror(1, first));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let mut replacement = pending;
        replacement.extend(mirror_words(
            "latest",
            48_000,
            64_000,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 2,
                phrase_id: 1,
            },
        ));
        emitter.on_event(&mirror(2, replacement));
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "earlier",
        );
        emitter.on_event(&mutation);
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "earlier pending latest");
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 2);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason == "superseded_by_partial rev=2")
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn untimed_final_stays_beside_commit_until_same_phrase_gets_timing() {
        let (mut emitter, ledger) = mirror_take();
        emitter.on_event(&closed_mirror(
            1,
            mirror_words(
                "untimed words",
                20_000,
                20_000,
                UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
            ),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Untimed,
            2,
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        assert_eq!(stopped.text, "untimed words");
        assert_eq!(stopped.untimed_final_words, 2);
        emitter.on_event(&preview_disposition(8, PreviewFinalDisposition::Admitted));
        assert_eq!(emitter.visible_canvas_snapshot().unwrap(), stopped);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 16_000, 32_000),
            1,
            "tail words",
        );
        emitter.on_event(&mutation);
        assert_eq!(
            emitter.visible_canvas_snapshot().unwrap().text,
            "tail words untimed words"
        );
        let mut replacement = closed_mirror(
            2,
            Vec::new(),
            1,
            codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
            2,
        );
        if let EngineEvent::UnadmittedAppleWords { closed_phrases, .. } = &mut replacement {
            closed_phrases.get_mut(&1).unwrap().was_untimed = true;
        }
        emitter.on_event(&replacement);
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "tail words");
        assert_eq!(pasted.untimed_final_words, 0);
        assert_eq!(pasted.untimed_final_phrases, vec![(1, 0)]);
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 2);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason == "closed_by_final phrase=1 outcomes=admitted=2")
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn open_partial_closes_to_pending_or_unmatched_state() {
        for (source, outcome) in [
            (
                UnadmittedAppleWordSource::Pending { utterance_id: 7 },
                codescribe_core::pipeline::contracts::ApplePhraseOutcome::Pending,
            ),
            (
                UnadmittedAppleWordSource::Unmatched,
                codescribe_core::pipeline::contracts::ApplePhraseOutcome::Unmatched,
            ),
        ] {
            let (mut emitter, _) = mirror_take();
            emitter.on_event(&mirror(
                1,
                mirror_words(
                    "open",
                    8_000,
                    12_000,
                    UnadmittedAppleWordSource::OpenPartial {
                        rev: 1,
                        phrase_id: 1,
                    },
                ),
            ));
            let stopped = emitter.begin_stop_canvas().unwrap();
            emitter.on_event(&closed_mirror(
                2,
                mirror_words("closed", 60 * 16_000, 62 * 16_000, source),
                1,
                outcome,
                1,
            ));
            let pasted = emitter.finish_stop_canvas().unwrap();
            assert_eq!(pasted.text, "closed");
            let reasons = stopped.missing_words_from(&pasted);
            assert_eq!(reasons.len(), 1);
            assert_eq!(
                reasons[0].reason,
                format!("closed_by_final phrase=1 outcomes={}=1", outcome.as_str())
            );
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn stuck_partial_pin_paints_at_tail_for_twenty_revisions() {
        for start in [0, 4_800] {
            let (mut emitter, ledger) = mirror_take();
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                OccurrenceIdentity::new("take", 7, start, 64_000),
                1,
                "committed",
            );
            emitter.on_event(&mutation);
            for rev in 1..=20 {
                let text = format!("open revision {rev}");
                emitter.on_event(&partial_mirror(rev, &text, 0, 0));
                let snapshot = emitter.visible_canvas_snapshot().unwrap();
                assert_eq!(snapshot.text, format!("committed {text}"));
                assert_eq!(snapshot.preview_only_words, 3);
            }
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn stuck_pin_stop_is_closed_by_distant_final_including_apple_rewrite_and_five_iwo() {
        for (partial, final_text) in [
            ("tail phrase", "tail phrase complete"),
            ("Loctree jest", "lock tree jest"),
            ("Iwo Iwo Iwo Iwo Iwo", "Iwo Iwo Iwo Iwo Iwo"),
        ] {
            let (mut emitter, ledger) = mirror_take();
            emitter.on_event(&partial_mirror(1, partial, 12_800, 12_800));
            let stopped = emitter.begin_stop_canvas().unwrap();
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                OccurrenceIdentity::new("take", 7, 963_200, 992_000),
                1,
                final_text,
            );
            emitter.on_event(&mutation);
            emitter.on_event(&closed_mirror(
                2,
                Vec::new(),
                1,
                codescribe_core::pipeline::contracts::ApplePhraseOutcome::Admitted,
                final_text.split_whitespace().count(),
            ));
            let pasted = emitter.finish_stop_canvas().unwrap();
            assert_eq!(pasted.text, final_text);
            assert_eq!(
                pasted.text.split_whitespace().count(),
                final_text.split_whitespace().count()
            );
            let reasons = stopped.missing_words_from(&pasted);
            assert_eq!(reasons.len(), partial.split_whitespace().count());
            assert!(reasons.iter().all(|word| {
                word.reason
                    .starts_with("closed_by_final phrase=1 outcomes=admitted=")
            }));
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn phrase_multiset_counts_repetitions_and_does_not_borrow_another_phrase() {
        let (mut emitter, _) = mirror_take();
        emitter.on_event(&partial_mirror(1, "Iwo Iwo", 0, 0));
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.on_event(&partial_mirror(2, "Iwo", 9_000, 9_000));
        let shortened = emitter.visible_canvas_snapshot().unwrap();
        let reasons = stopped.missing_words_from(&shortened);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].reason, "superseded_by_partial rev=2");
        emitter.on_event(&mirror(
            3,
            mirror_words(
                "Iwo Iwo",
                0,
                0,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 3,
                    phrase_id: 2,
                },
            ),
        ));
        let vanished = emitter.finish_stop_canvas().unwrap();
        let reasons = stopped.missing_words_from(&vanished);
        assert_eq!(reasons.len(), 2);
        assert!(reasons.iter().all(|word| word.reason == "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn ranged_mirror_word_is_accounted_by_covering_ledger_evidence() {
        let (mut emitter, ledger) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "old words",
                16_000,
                24_000,
                UnadmittedAppleWordSource::Pending { utterance_id: 9 },
            ),
        ));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            1,
            0,
            OccurrenceIdentity::new("take", 7, 8_000, 32_000),
        );
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            "revised evidence",
            super::NoAuthorityReason::NoRange,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "revised evidence".into(),
            receipt,
        });
        emitter.on_event(&mirror(2, Vec::new()));
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "revised evidence");
        let reasons = stopped.missing_words_from(&pasted);
        assert_eq!(reasons.len(), 2);
        assert!(
            reasons
                .iter()
                .all(|word| word.reason.starts_with("retained_as_evidence occurrence="))
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn untimed_phrases_follow_arrival_then_open_partial_with_no_pin_authority() {
        use codescribe_core::pipeline::contracts::{ApplePhraseOutcome, ClosedApplePhrase};
        let (mut emitter, ledger) = mirror_take();
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 64_000),
            1,
            "ranged",
        );
        emitter.on_event(&mutation);
        let mut words = mirror_words(
            "second",
            0,
            0,
            UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 2 },
        );
        words.extend(mirror_words(
            "open",
            0,
            0,
            UnadmittedAppleWordSource::OpenPartial {
                rev: 3,
                phrase_id: 3,
            },
        ));
        words.extend(mirror_words(
            "first",
            60_000,
            60_000,
            UnadmittedAppleWordSource::RefusedUntimed { phrase_id: 1 },
        ));
        emitter.on_event(&EngineEvent::UnadmittedAppleWords {
            revision: 1,
            words,
            closed_phrases: [1, 2]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        ClosedApplePhrase {
                            arrival_index: (id - 1) as usize,
                            outcomes: std::collections::BTreeMap::from([(
                                ApplePhraseOutcome::Untimed,
                                1,
                            )]),
                            was_untimed: true,
                        },
                    )
                })
                .collect(),
        });
        let stopped = emitter.begin_stop_canvas().unwrap();
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "ranged first second open");
        assert_eq!(pasted.untimed_final_phrases, vec![(1, 0), (2, 1)]);
        assert_eq!(pasted.untimed_final_words, 2);
        assert!(stopped.missing_words_from(&pasted).is_empty());
        emitter.finish().await;
    }

    #[tokio::test]
    async fn five_distinct_unadmitted_iwo_ranges_remain_five_in_both_reads() {
        let (mut emitter, _) = mirror_take();
        let words = (0..5)
            .map(|index| UnadmittedAppleWord {
                text: "Iwo".into(),
                sample_start: index * 16_000,
                sample_end: (index + 1) * 16_000,
                source: UnadmittedAppleWordSource::Unmatched,
            })
            .collect();
        emitter.on_event(&mirror(1, words));
        let stopped = emitter.begin_stop_canvas().unwrap();
        let pasted = emitter.finish_stop_canvas().unwrap();
        assert_eq!(pasted.text, "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(pasted.preview_only_words, 5);
        assert!(stopped.missing_words_from(&pasted).is_empty());
        emitter.finish().await;
    }

    #[tokio::test]
    async fn disposition_receipts_never_retain_or_clear_mirror_words() {
        let (mut emitter, _) = mirror_take();
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "pipeline truth",
                16_000,
                32_000,
                UnadmittedAppleWordSource::Unmatched,
            ),
        ));
        let before = emitter.begin_stop_canvas().unwrap();
        for disposition in [
            PreviewFinalDisposition::Admitted,
            PreviewFinalDisposition::KeptUnanchored,
            PreviewFinalDisposition::Refused {
                reason: "untimed".into(),
            },
        ] {
            emitter.on_event(&EngineEvent::PreviewDisposition {
                superseded_through_rev: 1,
                final_disposition: disposition,
                refused_evidence: vec![
                    codescribe_core::pipeline::contracts::RefusedPreviewEvidence {
                        range: None,
                        text: "receipt text".into(),
                        reason: "untimed".into(),
                    },
                ],
            });
            assert_eq!(emitter.visible_canvas_snapshot().unwrap(), before);
        }
        emitter.on_event(&mirror(2, Vec::new()));
        let after = emitter.finish_stop_canvas().unwrap();
        assert!(after.text.is_empty());
        assert!(
            before
                .missing_words_from(&after)
                .iter()
                .all(|word| word.reason == "unaccounted")
        );
        emitter.finish().await;
    }

    // Root-only presentation controls: relabel permission is explicit.
    #[tokio::test]
    async fn forensic_presenter_unproven_acoustic_omission_cannot_close_stop_words() {
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            let (mut emitter, ledger) = mirror_take();
            let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
            let initial = admitted_mutation(
                &mut ledger.lock().unwrap(),
                occurrence.clone(),
                981,
                "old label words",
            );
            emitter.on_event(&initial);
            let stopped = emitter.begin_stop_canvas().unwrap();
            let source = ledger
                .lock()
                .unwrap()
                .slots_of(&occurrence)
                .unwrap()
                .to_vec();
            let observation = ObservationIdentity::new(producer, 982, 0, occurrence.clone());
            let receipt = ledger.lock().unwrap().admit(&observation, "new label");
            assert!(!receipt.grants_mutation(), "{producer:?}: {receipt:?}");
            emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: "new label".into(),
                receipt,
            });
            let final_canvas = emitter.finish_stop_canvas().unwrap();
            assert_eq!(final_canvas, stopped);
            assert!(stopped.missing_words_from(&final_canvas).is_empty());
            assert_eq!(
                ledger.lock().unwrap().slots_of(&occurrence).unwrap(),
                source
            );
            assert_eq!(ledger.lock().unwrap().conservation().residue(), 0);
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn forensic_presenter_explicit_human_relabel_accounts_for_source_words() {
        let (mut emitter, ledger) = mirror_take();
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let initial = admitted_mutation(
            &mut ledger.lock().unwrap(),
            occurrence.clone(),
            991,
            "old label words",
        );
        emitter.on_event(&initial);
        let stopped = emitter.begin_stop_canvas().unwrap();
        let observation =
            ObservationIdentity::new(ObservationProducer::ManualHuman, 992, 0, occurrence.clone());
        let receipt = ledger.lock().unwrap().admit(&observation, "new label");
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "new label".into(),
            receipt,
        });
        let final_canvas = emitter.finish_stop_canvas().unwrap();
        assert_eq!(final_canvas.text, "new label");
        let missing = stopped.missing_words_from(&final_canvas);
        assert_eq!(missing.len(), 3);
        assert!(
            missing
                .iter()
                .all(|word| word.reason == format!("relabeled_in_place occurrence={occurrence:?}"))
        );
        assert_eq!(
            ledger.lock().unwrap().slots_of(&occurrence).unwrap()[0].producer,
            ObservationProducer::ManualHuman
        );
        assert_eq!(ledger.lock().unwrap().conservation().residue(), 0);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn forensic_presenter_complete_decode_relabels_coarse_source_without_word_floor() {
        use codescribe_core::pipeline::acoustic_ledger::WordPin;
        for producer in [ObservationProducer::Whisper, ObservationProducer::CloudLive] {
            for offset in [0, 16_000] {
                let (mut emitter, ledger) = mirror_take();
                let occurrence = OccurrenceIdentity::new("take", 7, offset, offset + 16_000);
                let initial = admitted_mutation(
                    &mut ledger.lock().unwrap(),
                    occurrence.clone(),
                    993,
                    "old label words",
                );
                emitter.on_event(&initial);
                let stopped = emitter.begin_stop_canvas().unwrap();
                let source = ledger
                    .lock()
                    .unwrap()
                    .slots_of(&occurrence)
                    .unwrap()
                    .to_vec();
                // The two decoded words abut over measured speech; the ladder
                // proves the owner's edges quiet, so the third held word may go.
                record_measured_speech(
                    &mut ledger.lock().unwrap(),
                    "take",
                    7,
                    offset + 32_000,
                    &[(offset + 1_000, offset + 15_000)],
                );
                let observation = ObservationIdentity::new(producer, 994, 0, occurrence.clone());
                let receipt = ledger.lock().unwrap().admit_word_slots_for_tests(
                    &observation,
                    &[
                        WordPin::new(offset + 1_000, offset + 7_000, "new")
                            .with_decode_window(0, offset + 32_000),
                        WordPin::new(offset + 7_000, offset + 15_000, "label")
                            .with_decode_window(0, offset + 32_000),
                    ],
                );
                assert!(
                    matches!(receipt, MutationReceipt::Correct { .. }),
                    "{receipt:?}"
                );
                {
                    let ledger = ledger.lock().unwrap();
                    let operation = ledger.slot_operations().last().unwrap();
                    assert_eq!(operation.sources, source);
                    assert_eq!(
                        operation.source_ranges,
                        [
                            occurrence.clone(),
                            OccurrenceIdentity::new("take", 7, offset + 1_000, offset + 7_000),
                            OccurrenceIdentity::new("take", 7, offset + 7_000, offset + 15_000),
                        ]
                    );
                    assert_eq!(operation.outputs.len(), 2);
                    assert!(
                        operation
                            .outputs
                            .iter()
                            .all(|slot| slot.producer == producer)
                    );
                }
                emitter.on_event(&EngineEvent::LedgerMutation {
                    observation,
                    label: "new label".into(),
                    receipt,
                });
                let final_canvas = emitter.finish_stop_canvas().unwrap();
                assert_eq!(final_canvas.text, "new label");
                let missing = stopped.missing_words_from(&final_canvas);
                assert_eq!(missing.len(), 3);
                assert!(
                    missing.iter().all(|word| word.reason
                        == format!("relabeled_in_place occurrence={occurrence:?}"))
                );
                assert_eq!(ledger.lock().unwrap().conservation().residue(), 0);
                emitter.finish().await;
            }
        }
    }

    #[tokio::test]
    async fn relabel_in_place_during_the_wait_is_accounted_not_a_defect() {
        let (mut emitter, ledger) = mirror_take();
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let mutation =
            admitted_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), 1, "before");
        emitter.on_event(&mutation);
        let stopped = emitter.begin_stop_canvas().unwrap();
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence.clone());
        let receipt = ledger.lock().unwrap().admit(&observation, "after");
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "after".into(),
            receipt,
        });
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "after");
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].reason,
            format!("relabeled_in_place occurrence={occurrence:?}")
        );
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_wait_accounts_for_shaping_relabel_and_untouched_occurrences() {
        // Hold presentation until the seal arrives during the STOP wait.
        let (mut emitter, ledger) = mirror_take();
        let occurrences = (0..3)
            .map(|index| OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000))
            .collect::<Vec<_>>();
        for (index, label) in ["first phrase", "old label words", "untouched words"]
            .iter()
            .enumerate()
        {
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                occurrences[index].clone(),
                index as u64 + 1,
                label,
            );
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.set_literal_delivery(false);
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrences[0].clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrences[0], ObservationProducer::Apple));
            ledger.seal(&occurrences[0]).unwrap().clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
        record_measured_speech(
            &mut ledger.lock().unwrap(),
            "take",
            7,
            occurrences[2].sample_end,
            &[
                (occurrences[0].sample_start, occurrences[0].sample_end),
                (
                    occurrences[1].sample_start + 1_000,
                    occurrences[1].sample_end - 1_000,
                ),
                (occurrences[2].sample_start, occurrences[2].sample_end),
            ],
        );
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 4, 0, occurrences[1].clone());
        // This acoustic revision carries complete Word/decode evidence over
        // measured speech; a shorter whole-label proposal alone cannot retire
        // source wording.
        let receipt = ledger.lock().unwrap().admit_word_slots_for_tests(
            &observation,
            &[
                codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                    occurrences[1].sample_start + 1_000,
                    occurrences[1].sample_start + 7_000,
                    "new",
                )
                .with_decode_window(0, occurrences[1].sample_end + 16_000),
                codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                    occurrences[1].sample_start + 7_000,
                    occurrences[1].sample_end - 1_000,
                    "label",
                )
                .with_decode_window(0, occurrences[1].sample_end + 16_000),
            ],
        );
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "new label".into(),
            receipt,
        });
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "First phrase new label untouched words");
        assert_eq!(
            frozen.text,
            *emitter.paint_commands.lock().unwrap().last().unwrap()
        );
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 5);
        assert_eq!(
            missing
                .iter()
                .filter(|word| word.reason
                    == format!("reshaped_in_place occurrence={:?}", occurrences[0]))
                .count(),
            2
        );
        assert_eq!(
            missing
                .iter()
                .filter(|word| word.reason
                    == format!("relabeled_in_place occurrence={:?}", occurrences[1]))
                .count(),
            3
        );
        assert_eq!(stopped.visible_words.len() - missing.len(), 2);
        assert!(missing.iter().all(|word| word.reason != "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn removed_committed_occurrence_without_successor_is_unaccounted() {
        let (mut emitter, ledger) = mirror_take();
        let removed = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let survivor = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        for (index, occurrence) in [&removed, &survivor].iter().enumerate() {
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                (*occurrence).clone(),
                index as u64 + 1,
                "same words",
            );
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        // Fault injection: lose a reducer entry while identical words survive
        // at a disjoint PCM range. Text must not conceal the loss.
        emitter
            .session_state
            .lock()
            .unwrap()
            .document_by_occurrence
            .remove(&removed);
        emitter.send_committed_paint("same words".into());
        let frozen = emitter.finish_stop_canvas().unwrap();
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 2);
        assert!(missing.iter().all(|word| word.reason == "unaccounted"));

        // A different committed owner may account for that range only when
        // its PCM actually contains it on the same capture clock.
        let owner = OccurrenceIdentity::new("take", 7, 0, 32_000);
        {
            let mut state = emitter.session_state.lock().unwrap();
            let mut entry = state.document_by_occurrence.remove(&survivor).unwrap();
            entry.occurrence = owner.clone();
            state.document_by_occurrence.insert(owner.clone(), entry);
        }
        emitter.send_committed_paint("same words".into());
        let covered = emitter.visible_canvas_snapshot().unwrap();
        let accounted = stopped.missing_words_from(&covered);
        assert_eq!(accounted.len(), 4);
        assert!(
            accounted
                .iter()
                .all(|word| word.reason == format!("covered_by_committed occurrence={owner:?}"))
        );
        let mut foreign = covered.clone();
        foreign.capture_epoch += 1;
        assert!(
            stopped
                .missing_words_from(&foreign)
                .iter()
                .all(|word| word.reason == "unaccounted")
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn document_revision_accounts_for_member_occurrences() {
        let (mut emitter, ledger) = mirror_take();
        let first = OccurrenceIdentity::new("take", 7, 0, 16_000);
        let second = OccurrenceIdentity::new("take", 7, 16_000, 32_000);
        for (index, occurrence) in [&first, &second].iter().enumerate() {
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                (*occurrence).clone(),
                index as u64 + 1,
                "old words",
            );
            emitter.on_event(&mutation);
        }
        let plain = emitter.visible_canvas_snapshot().unwrap();
        emitter.on_event(&EngineEvent::ContextMarker {
            position: 0,
            label: "{selection_1}".into(),
        });
        let stopped = emitter.begin_stop_canvas().unwrap();
        assert!(plain.missing_words_from(&stopped).is_empty());
        record_measured_speech(
            &mut ledger.lock().unwrap(),
            "take",
            7,
            second.sample_end,
            &[
                (first.sample_start + 1_000, first.sample_end - 1_000),
                (second.sample_start, second.sample_end),
            ],
        );
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, first.clone());
        // This acoustic revision carries complete Word/decode evidence over
        // measured speech; a shorter whole-label proposal alone cannot retire
        // source wording.
        let receipt = ledger.lock().unwrap().admit_word_slots_for_tests(
            &observation,
            &[
                codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                    first.sample_start + 1_000,
                    first.sample_start + 7_000,
                    "new",
                )
                .with_decode_window(0, first.sample_end + 16_000),
                codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                    first.sample_start + 7_000,
                    first.sample_end - 1_000,
                    "label",
                )
                .with_decode_window(0, first.sample_end + 16_000),
            ],
        );
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "new label".into(),
            receipt,
        });
        let relabeled = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(relabeled.text, "{selection_1} new label old words");
        assert!(
            stopped
                .missing_words_from(&relabeled)
                .iter()
                .all(|word| word.reason == format!("relabeled_in_place occurrence={first:?}"))
        );

        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "take".into(),
            layer_summary: LayerSummary::default(),
        });
        emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "take".into(),
                source_revision: relabeled.revision,
                rendered_text: "Revised document.".into(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .unwrap();
        let revised = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(revised.committed_sources.len(), 2);
        let reshaped = relabeled.missing_words_from(&revised);
        assert_eq!(reshaped.len(), relabeled.visible_words.len());
        assert!(reshaped.iter().all(|word| {
            word.reason
                .contains(&format!("reshaped_in_place occurrence={first:?}"))
                && word
                    .reason
                    .contains(&format!("reshaped_in_place occurrence={second:?}"))
        }));
        assert!(revised.missing_words_from(&revised).is_empty());

        emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "take".into(),
                source_revision: revised.revision,
                rendered_text: "Another document revision.".into(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .unwrap();
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(frozen.text, "Another document revision.");
        assert!(revised.missing_words_from(&frozen).iter().all(|word| {
            word.reason
                .contains(&format!("reshaped_in_place occurrence={first:?}"))
                && word
                    .reason
                    .contains(&format!("reshaped_in_place occurrence={second:?}"))
        }));

        // Joint document provenance is not permission to lose a member.
        let mut removed = frozen.clone();
        removed.committed_sources.remove(&second);
        assert!(
            revised
                .missing_words_from(&removed)
                .iter()
                .all(|word| word.reason == "unaccounted")
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn frozen_shaping_accounts_for_every_committed_member() {
        let (mut emitter, ledger) = mirror_take();
        for index in 0..2 {
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000),
                index + 1,
                "some words",
            );
            emitter.on_event(&mutation);
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        emitter.set_literal_delivery(false);
        let frozen = emitter
            .shape_frozen_canvas_at_stop(stopped.clone())
            .unwrap();
        assert_eq!(
            frozen.text,
            format!("{}.", stopped.text),
            "Stop keeps the existing casing"
        );
        let missing = stopped.missing_words_from(&frozen);
        assert_eq!(missing.len(), 4);
        for occurrence in stopped.committed_sources.keys() {
            assert_eq!(
                missing
                    .iter()
                    .filter(|word| word.reason
                        == format!("reshaped_in_place occurrence={occurrence:?}"))
                    .count(),
                2
            );
        }
        let painted = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(frozen.visible_words, painted.visible_words);
        assert_eq!(frozen.committed_sources, painted.committed_sources);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn stop_wait_accounts_for_all_consultation_members() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let (mut emitter, ledger) = mirror_take();
        let mut members = Vec::new();
        for index in 0..2 {
            let occurrence =
                OccurrenceIdentity::new("take", 7, index * 16_000, (index + 1) * 16_000);
            let mutation = admitted_mutation(
                &mut ledger.lock().unwrap(),
                occurrence.clone(),
                index + 1,
                "old words",
            );
            emitter.on_event(&mutation);
            let seal = {
                let mut ledger = ledger.lock().unwrap();
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                ledger.seal(&occurrence).unwrap().clone()
            };
            emitter.on_event(&EngineEvent::LedgerSeal {
                receipt: seal.clone(),
            });
            members.push(ConsultationPresentationMember {
                occurrence,
                source_label: "old words".into(),
                seal_receipt: seal.receipt_id,
            });
        }
        let stopped = emitter.begin_stop_canvas().unwrap();
        let revision = {
            let mut ledger = ledger.lock().unwrap();
            emitter
                .session_state
                .lock()
                .unwrap()
                .apply_consultation_presentation(
                    &mut ledger,
                    ConsultationPresentationInput {
                        consultation_id: "Max",
                        turn_id: "stop-wait",
                        source_revision: 0,
                        revision: 1,
                        members: &members,
                        rendered_text: "Combined answer",
                    },
                )
                .unwrap()
        };
        emitter.publish_derived_paint(&revision);
        let frozen = emitter.finish_stop_canvas().unwrap();
        assert_eq!(revision.rendered_text, "Combined answer");
        assert_eq!(frozen.text, stopped.text);
        assert_eq!(frozen.committed_sources.len(), 2);
        let missing = stopped.missing_words_from(&frozen);
        assert!(missing.is_empty());
        {
            let mut state = emitter.session_state.lock().unwrap();
            state.document_by_occurrence.remove(&members[1].occurrence);
            state.invalidate_for_document_edit(
                &members[1].occurrence,
                true,
                &ledger.lock().unwrap(),
            );
        }
        emitter.send_committed_paint("old words".into());
        let removed = emitter.visible_canvas_snapshot().unwrap();
        let lost = frozen.missing_words_from(&removed);
        assert_eq!(lost.len(), 2);
        assert!(lost.iter().all(|word| word.reason == "unaccounted"));
        emitter.finish().await;
    }

    #[tokio::test]
    async fn retired_presentation_still_updates_its_own_delivery_buffer() {
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let delivery = Arc::new(Mutex::new(String::new()));
        let deltas = Arc::new(RecordingDeltaSink::default());
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            Some(deltas.clone()),
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("take", 7);
        emitter.retire_presentation();
        let admitted = admitted_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("take", 7, 0, 16_000),
            1,
            "late words",
        );
        emitter.on_event(&admitted);
        emitter.finish().await;
        assert!(deltas.deltas.lock().unwrap().is_empty());
        assert_eq!(*delivery.lock().await, "late words");
    }

    #[tokio::test]
    async fn ledger_mutation_paints_overlay_and_writes_exact_revision_to_delivery() {
        let deltas = Arc::new(RecordingDeltaSink::default());
        let fixture = BusFixture::new("session", true, "ledger.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        let projection_count = Arc::new(AtomicUsize::new(0));
        let projection_count_for_callback = Arc::clone(&projection_count);
        let mutation = {
            let mut guard = ledger.lock().unwrap_or_else(|error| error.into_inner());
            admitted_mutation(
                &mut guard,
                OccurrenceIdentity::new("session", 3, 0, 16_000),
                1,
                "Iwo",
            )
        };
        bus.publish_started();
        let mut emitter = fixture.emitter(
            Some(deltas.clone()),
            Some(Arc::new(move |_| {
                projection_count_for_callback.fetch_add(1, Ordering::SeqCst);
            })),
        );

        emitter.on_event(&mutation);
        emitter.finish().await;
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("committed book must produce a terminal projection");

        assert_eq!(delivery.lock().await.as_str(), "Iwo");
        assert!(
            !deltas
                .deltas
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        assert_eq!(projection_count.load(Ordering::SeqCst), 1);
        assert_eq!(terminal.rendered_text, "Iwo");
        assert_eq!(terminal.phase, TranscriptProjectionPhase::Formatted);
        assert!(terminal.can_paste);
        assert!(terminal.can_insert);
        assert!(terminal.can_copy);
        assert!(terminal.can_retranscribe);
        assert!(terminal.can_format);
        assert!(terminal.terminal);
        let bus_bytes = std::fs::read(bus_path).unwrap();
        assert!(
            std::str::from_utf8(&bus_bytes)
                .unwrap()
                .contains("codescribe.transcript-evidence.v1")
        );
        let mut reader = TranscriptProjectionReader::new();
        let tail_projections = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("projection tail must parse");
        let tail_terminal = tail_projections.last().expect("terminal tail projection");
        assert_eq!(tail_terminal.rendered_text, "Iwo");
        assert_eq!(tail_terminal.phase, TranscriptProjectionPhase::Formatted);
        assert!(tail_terminal.can_paste);
        assert!(tail_terminal.terminal);
    }

    #[tokio::test]
    async fn raw_text_lane_failure_and_session_close_cannot_change_delivery() {
        let fixture = BusFixture::new("session", false, "raw-events.jsonl", "last ledger revision");
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        let projection_count = Arc::new(AtomicUsize::new(0));
        let projection_count_for_callback = Arc::clone(&projection_count);
        let mut emitter = fixture.emitter(
            None,
            Some(Arc::new(move |_| {
                projection_count_for_callback.fetch_add(1, Ordering::SeqCst);
            })),
        );

        emitter.on_event(&partial_mirror(1, "volatile", 0, 16_000));
        emitter.on_event(&raw_final("raw final"));
        emitter.on_event(&EngineEvent::Correction {
            rev: 2,
            text: "correction".to_string(),
            previous_text: "raw final".to_string(),
        });
        emitter.on_event(&EngineEvent::ReplaceRange {
            utterance_id: 1,
            start: 0,
            end: 1,
            text: "replacement".to_string(),
            source: LayerSource::TailPatch,
        });
        emitter.on_event(&EngineEvent::InsertAnnotation {
            utterance_id: 1,
            position: 0,
            text: "annotation".to_string(),
            kind: AnnotationKind::HesitationPause,
        });
        emitter.on_event(&EngineEvent::Warning {
            code: "agent_lane_transport_failed".to_string(),
            message: "Tool-enabled response failed (ConnectError: gateway unavailable)".to_string(),
        });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        emitter.finish().await;

        assert_eq!(delivery.lock().await.as_str(), "last ledger revision");
        assert!(!delivery.lock().await.contains("ConnectError"));
        assert_eq!(projection_count.load(Ordering::SeqCst), 0);
        assert!(std::fs::read_to_string(bus_path).unwrap().is_empty());
    }

    // Explicit human recovery must not require the failed recognizer to have
    // committed a lexical floor. Its document receipt is not a word-to-PCM seal.
    async fn forensic_empty_terminal_recovery(provenance: DocumentRevisionProvenance) {
        let fixture = BusFixture::new("empty-recovery", false, "empty.jsonl", "");
        let owner = OccurrenceIdentity::new("empty-recovery", 7, 0, 16_000);
        let calibration = EnergyCalibration {
            version: "empty-recovery-controlled".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: owner.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(0),
            vad_close_sample: Some(16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(
            fixture
                .ledger
                .lock()
                .unwrap()
                .qualify(&evidence, &calibration)
                .is_qualified()
        );
        fixture.bus.publish_started();
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let callback = Arc::clone(&observed);
        let mut emitter = fixture.emitter(
            None,
            Some(Arc::new(move |projection| {
                callback.lock().unwrap().push(projection.clone());
            })),
        );
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened("empty-recovery", 7);
        emitter.on_event(&partial_mirror(1, "hipoteza bez zatwierdzenia", 0, 16_000));
        assert_eq!(
            emitter
                .visible_canvas_snapshot()
                .expect("the capture owns its preview snapshot")
                .text,
            "hipoteza bez zatwierdzenia"
        );
        assert!(fixture.delivery.lock().await.is_empty());
        assert!(fixture.ledger.lock().unwrap().text_of(&owner).is_none());
        let intent = UserRevisionIntent {
            session_id: "empty-recovery".into(),
            source_revision: 0,
            rendered_text: "Tekst odzyskany przez człowieka".into(),
            provenance,
        };
        assert_eq!(
            emitter.apply_user_revision(intent.clone()),
            Err(UserRevisionRefusal::NotTerminal)
        );
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "empty-recovery".into(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = fixture
            .bus
            .publish_ended(
                TranscriptSessionEndReason::CoverageRefusedEmpty,
                true,
                TranscriptDelivery::Retained,
            )
            .expect("a failed empty take still has a terminal lifecycle");
        assert!(terminal.terminal && terminal.lifecycle_terminal);
        assert_eq!(terminal.reducer_revision, intent.source_revision);
        assert!(terminal.rendered_text.is_empty());
        assert!(
            fixture.delivery.lock().await.is_empty(),
            "a lifecycle event cannot commit preview"
        );
        let mut foreign = intent.clone();
        foreign.session_id = "different-take".into();
        assert!(emitter.apply_user_revision(foreign).is_err());
        let mut stale = intent.clone();
        stale.source_revision = 999;
        assert!(emitter.apply_user_revision(stale).is_err());
        assert!(
            fixture
                .ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .is_empty()
        );
        let committed = emitter.apply_user_revision(intent.clone()).expect(
            "explicit recovery must create the first human document after failed transcription",
        );
        assert_eq!(committed.session_id, intent.session_id);
        assert_eq!(committed.source_revision, intent.source_revision);
        assert_eq!(committed.revision, intent.source_revision + 1);
        assert_eq!(committed.rendered_text, intent.rendered_text);
        assert!(
            committed
                .provenance_receipt
                .starts_with(provenance.as_str())
        );
        {
            let ledger = fixture.ledger.lock().unwrap();
            assert!(
                ledger.text_of(&owner).is_none(),
                "human recovery cannot invent acoustic labels"
            );
            assert!(
                ledger.slots_of(&owner).is_none(),
                "human recovery cannot mint word pins"
            );
            assert!(ledger.seal_of(&owner).is_none());
            assert!(
                ledger
                    .terminal_finality("empty-recovery", 7)
                    .into_refusal()
                    .is_some()
            );
            let receipts = ledger.manual_document_revisions();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].rendered_text, intent.rendered_text);
            assert_eq!(receipts[0].session_id, intent.session_id);
            assert_eq!(receipts[0].revision, committed.revision);
        }
        {
            let projections = observed.lock().unwrap();
            let last = projections
                .last()
                .expect("the Bus must publish the authenticated manual document");
            assert_eq!(last.session_id, intent.session_id);
            assert_eq!(last.reducer_revision, committed.revision);
            assert_eq!(last.rendered_text, intent.rendered_text);
            assert!(last.terminal && !last.lifecycle_terminal);
            assert!(
                last.acoustic_receipts.is_empty(),
                "manual-only recovery has no acoustic certification"
            );
        }
        assert!(
            emitter.apply_user_revision(intent.clone()).is_err(),
            "CAS must reject the old source revision"
        );
        emitter.finish().await;
        assert_eq!(fixture.delivery.lock().await.as_str(), intent.rendered_text);
        let history = crate::presentation::transcript_bus::document_history_at(
            &fixture.path,
            &intent.session_id,
        )
        .unwrap();
        assert_eq!(
            history
                .last()
                .expect("manual recovery must survive history replay")
                .rendered_text,
            intent.rendered_text
        );
    }

    #[tokio::test]
    async fn forensic_empty_terminal_rejects_automatic_or_unowned_bootstrap() {
        for owned in [false, true] {
            let fixture = BusFixture::new("bootstrap", false, "bootstrap.jsonl", "");
            fixture.bus.publish_started();
            let mut emitter = fixture.emitter(None, None);
            if owned {
                emitter.on_capture_opened("bootstrap", 7);
            }
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: "bootstrap".into(),
                layer_summary: LayerSummary::default(),
            });
            fixture
                .bus
                .publish_ended(
                    TranscriptSessionEndReason::TranscriptionFailed,
                    true,
                    TranscriptDelivery::Retained,
                )
                .unwrap();
            let provenances = if owned {
                [
                    DocumentRevisionProvenance::LightPlus,
                    DocumentRevisionProvenance::Formatter,
                ]
            } else {
                [
                    DocumentRevisionProvenance::UserEdit,
                    DocumentRevisionProvenance::Retranscribe,
                ]
            };
            for provenance in provenances {
                assert!(
                    emitter
                        .apply_user_revision(UserRevisionIntent {
                            session_id: "bootstrap".into(),
                            source_revision: 0,
                            rendered_text: "Nieuprawniony pierwszy dokument".into(),
                            provenance,
                        })
                        .is_err()
                );
            }
            assert!(
                fixture
                    .ledger
                    .lock()
                    .unwrap()
                    .manual_document_revisions()
                    .is_empty()
            );
            emitter.finish().await;
            assert!(fixture.delivery.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn forensic_empty_terminal_allows_explicit_user_document() {
        forensic_empty_terminal_recovery(DocumentRevisionProvenance::UserEdit).await;
    }

    #[tokio::test]
    async fn forensic_empty_terminal_allows_explicit_button_retranscribe() {
        forensic_empty_terminal_recovery(DocumentRevisionProvenance::Retranscribe).await;
    }

    /// Effect witness for W4-T15: the request names a session/revision rather
    /// than text identity, Rust mints the next document revision and a
    /// `user-edit` ledger receipt, the Bus persists it after microphone
    /// lifecycle end, and replay returns the same terminal bytes.
    #[tokio::test]
    async fn explicit_revisions_commit_after_refused_terminal_seal() {
        let fixture = BusFixture::new("refused-take", false, "refused.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let occurrence = OccurrenceIdentity::new("refused-take", 7, 0, 16_000);
        let mutation = admitted_mutation(
            &mut ledger.lock().unwrap_or_else(|error| error.into_inner()),
            occurrence,
            1,
            "Pierwsza wersja",
        );
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_event(&mutation);
        let coverage = {
            use codescribe_core::audio::capture_receipt::{
                AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
            };
            let mut ledger = ledger.lock().unwrap();
            let receipt = ledger.assess_seal_coverage(
                "refused-take",
                7,
                &AcousticSpeechEvidence::measured(
                    CaptureEvidenceIdentity::new("refused-take", 7),
                    "capture_energy",
                    AcousticAvailability::Observed {
                        observed_samples: 48_000,
                    },
                    vec![TailSampleRange {
                        session: "refused-take".into(),
                        capture_epoch: 7,
                        sample_start: 0,
                        sample_end: 48_000,
                    }],
                ),
                8_000,
            );
            assert!(!receipt.status.is_complete());
            assert!(ledger.record_seal_coverage(receipt.clone()));
            assert_eq!(
                ledger.seal_terminal("refused-take", 7),
                Err(SealRefusal::CoverageIncomplete)
            );
            receipt
        };
        emitter.on_event(&EngineEvent::SealCoverage {
            receipt: coverage,
            comparison: None,
        });
        let open_revision = emitter.session_state.lock().unwrap().revision;
        assert_eq!(
            emitter.terminal_revision_source("refused-take", open_revision),
            Err(UserRevisionRefusal::NotTerminal),
        );
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "refused-take".to_string(),
            layer_summary: LayerSummary::default(),
        });
        assert_eq!(
            ledger.lock().unwrap().manual_document_revisions()[0].provenance,
            "light-plus",
            "a refused acoustic terminal still permits a presentation revision"
        );
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::CoverageRefused,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let first = crate::presentation::transcript_bus::document_history_at(
            &fixture.path.clone(),
            "refused-take",
        )
        .unwrap()
        .first()
        .unwrap()
        .clone();
        let formatted = emitter
            .apply_formatter_revision(
                "refused-take".to_string(),
                terminal.reducer_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Druga wersja".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("formatter result needs lifecycle end, not a seal");
        assert!(formatted.provenance_receipt.starts_with("formatter-"));
        let edit = UserRevisionIntent {
            session_id: "refused-take".to_string(),
            source_revision: emitter.document_timeline("refused-take").0,
            rendered_text: formatted.rendered_text.clone(),
            provenance: DocumentRevisionProvenance::UserEdit,
        };
        let user_edit = emitter
            .navigate_document("refused-take".into(), edit.source_revision, 0)
            .expect("history restore needs lifecycle end, not a seal");
        assert_eq!(user_edit.rendered_text, terminal.rendered_text);
        assert_ne!(first.rendered_text, formatted.rendered_text);
        assert!(user_edit.provenance_receipt.starts_with("navigation-"));
        let formatted_cas = edit.source_revision;
        let committed = emitter
            .apply_user_revision(UserRevisionIntent {
                source_revision: user_edit.revision,
                rendered_text: "Trzecia wersja".to_string(),
                provenance: DocumentRevisionProvenance::Retranscribe,
                ..edit
            })
            .expect("explicit button pass revises the refused take");
        assert_eq!(committed.rendered_text, "Trzecia wersja");
        assert!(committed.provenance_receipt.starts_with("retranscribe-"));
        assert!(
            ledger
                .lock()
                .unwrap()
                .terminal_finality("refused-take", 7)
                .into_refusal()
                .is_some()
        );
        let rows = std::fs::read_to_string(fixture.path.clone()).unwrap();
        assert!(rows.contains("\"phase\":\"coverage_refused\""));
        assert!(rows.contains("\"reducer_action\":\"apply_manual_edit\""));
        assert!(rows.contains("\"seal_coverage\""));
        let ended_at = rows
            .lines()
            .position(|row| row.contains("\"status\":\"session_ended\""))
            .expect("refused lifecycle row is published");
        let first_edit_after_end = rows
            .lines()
            .skip(ended_at + 1)
            .find(|row| row.contains("\"reducer_action\":\"apply_manual_edit\""))
            .expect("formatter is the first edit after session_ended");
        assert!(first_edit_after_end.contains("\"phase\":\"coverage_refused\""));
        assert!(first_edit_after_end.contains(&format!("\"reducer_revision\":{formatted_cas}")));
        assert!(
            rows.lines()
                .any(|row| row.contains(&formatted.provenance_receipt)
                    && row.contains("codescribe.derived-transcript.v1"))
        );
        for row in rows
            .lines()
            .filter(|row| row.contains("\"reducer_action\":\"apply_manual_edit\""))
        {
            assert!(row.contains("\"phase\":\"coverage_refused\""));
            assert!(row.contains("\"seal_receipt\":null"));
        }
        assert_eq!(
            crate::presentation::transcript_bus::document_history_at(
                &fixture.path.clone(),
                "refused-take"
            )
            .unwrap()
            .last()
            .unwrap()
            .rendered_text,
            "Trzecia wersja"
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn terminal_user_revision_is_ledger_stamped_and_replayable() {
        let fixture = BusFixture::new("revision-session", true, "user-revision.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        let occurrence = OccurrenceIdentity::new("revision-session", 4, 0, 16_000);
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "Tekst bazowy");
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = fixture.emitter(
            None,
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("revision-session", 4)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "revision-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");

        let commit = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "revision-session".to_string(),
                source_revision: terminal.reducer_revision,
                rendered_text: "Tekst poprawiony przez użytkownika".to_string(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .expect("current terminal revision intent must commit");
        assert_eq!(commit.revision, terminal.reducer_revision + 1);
        assert_eq!(commit.rendered_text, "Tekst poprawiony przez użytkownika");
        assert!(commit.provenance_receipt.starts_with("user-edit-"));
        let stale = emitter.apply_user_revision(UserRevisionIntent {
            session_id: "revision-session".to_string(),
            source_revision: terminal.reducer_revision,
            rendered_text: "Spóźniona edycja".to_string(),
            provenance: DocumentRevisionProvenance::UserEdit,
        });
        assert!(matches!(
            stale,
            Err(UserRevisionRefusal::StaleRevision { .. })
        ));

        emitter.finish().await;
        assert_eq!(
            delivery.lock().await.as_str(),
            "Tekst poprawiony przez użytkownika"
        );
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        // Two document revisions: the terminal Light+ floor ("Tekst bazowy."),
        // then the user edit on top of it.
        assert_eq!(ledger.manual_document_revisions().len(), 2);
        assert_eq!(
            ledger.manual_document_revisions()[0].provenance,
            "light-plus"
        );
        let receipt = &ledger.manual_document_revisions()[1];
        assert_eq!(receipt.provenance, "user-edit");
        assert_eq!(receipt.rendered_text, commit.rendered_text);
        assert_eq!(receipt.source_occurrences, vec![occurrence]);
        drop(ledger);

        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let revision_projection = projected
            .iter()
            .rev()
            .find(|event| event.reducer_revision == commit.revision)
            .expect("user revision projection callback");
        assert_eq!(revision_projection.reducer_action, "apply_manual_edit");
        assert_eq!(
            revision_projection.phase,
            TranscriptProjectionPhase::Formatted
        );
        assert!(revision_projection.terminal);
        assert_eq!(revision_projection.rendered_text, commit.rendered_text);
        assert_eq!(
            revision_projection.acoustic_receipts[0]
                .manual_edit_receipt
                .as_deref(),
            Some(commit.provenance_receipt.as_str())
        );
        drop(projected);

        let bus_bytes = std::fs::read(bus_path).unwrap();
        let mut reader = TranscriptProjectionReader::new();
        let replay = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("revision Bus bytes must replay");
        let replayed_revision = replay.last().expect("replayed user revision");
        assert_eq!(replayed_revision.reducer_revision, commit.revision);
        assert_eq!(replayed_revision.reducer_action, "apply_manual_edit");
        assert_eq!(replayed_revision.rendered_text, commit.rendered_text);
        assert!(replayed_revision.terminal);
    }

    #[tokio::test]
    async fn restoring_first_bus_version_appends_new_document_version() {
        let fixture = BusFixture::new("version-take", false, "versions.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let path = fixture.path.clone();
        let occurrence = OccurrenceIdentity::new("version-take", 3, 0, 16_000);
        let mutation = {
            let mut ledger = ledger.lock().unwrap();
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "First words");
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.on_event(&mutation);
        let seal = ledger
            .lock()
            .unwrap()
            .seal_terminal("version-take", 3)
            .unwrap();
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "version-take".into(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let initial =
            crate::presentation::transcript_bus::document_history_at(&path, "version-take")
                .unwrap();
        let first = initial.first().unwrap().clone();
        let second = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: terminal.reducer_revision,
                rendered_text: "Second words".into(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .unwrap();
        let third = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: second.revision,
                rendered_text: "Third words".into(),
                provenance: DocumentRevisionProvenance::Retranscribe,
            })
            .unwrap();
        let before =
            crate::presentation::transcript_bus::document_history_at(&path, "version-take")
                .unwrap();
        assert_eq!(before[before.len() - 2].provenance, "user-edit");
        assert_eq!(before.last().unwrap().provenance, "retranscribe");
        let restored = emitter
            .apply_user_revision(UserRevisionIntent {
                session_id: "version-take".into(),
                source_revision: third.revision,
                rendered_text: first.rendered_text.clone(),
                provenance: DocumentRevisionProvenance::UserEdit,
            })
            .unwrap();
        let after = crate::presentation::transcript_bus::document_history_at(&path, "version-take")
            .unwrap();
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(after.last().unwrap().revision, restored.revision);
        assert_eq!(after.last().unwrap().rendered_text, first.rendered_text);
        assert_eq!(after.last().unwrap().provenance, "user-edit");
        emitter.finish().await;
    }

    /// I4m effect witness: a failed formatter result cannot touch ledger, Bus,
    /// projection, or delivery. An applied result then mints formatter
    /// provenance and repaints only through the committed projection callback.
    #[tokio::test]
    async fn terminal_formatter_revision_commits_effect_and_failure_is_pure() {
        let fixture = BusFixture::new("formatter-session", true, "formatter-revision.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        let occurrence = OccurrenceIdentity::new("formatter-session", 5, 0, 16_000);
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(
                &mut ledger,
                occurrence.clone(),
                1,
                "to jest tekst wymagający formatowania",
            );
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = fixture.emitter(
            None,
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("formatter-session", 5)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "formatter-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");

        let source = emitter
            .terminal_revision_source("formatter-session", terminal.reducer_revision)
            .expect("current terminal source");
        // Light+ ran at the terminal seal, so the formatter is fed shaped text
        // (Light+ before LLM, as the controller always ordered it).
        assert_eq!(source, "To jest tekst wymagający formatowania.");
        let revisions_before_failure = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .manual_document_revisions()
            .len();
        let bus_before_failure = std::fs::read(&bus_path).unwrap();
        let delivery_before_failure = delivery.lock().await.clone();
        let projection_count_before_failure = projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        let failure = emitter.apply_formatter_revision(
            "formatter-session".to_string(),
            terminal.reducer_revision,
            FormattingPolicy::Smart,
            AiFormatResult {
                text: source.clone(),
                reasoning_text: None,
                status: AiFormatStatus::Failed,
            },
        );
        assert_eq!(failure, Err(UserRevisionRefusal::FormatterFailed));
        assert_eq!(
            ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .len(),
            revisions_before_failure
        );
        assert_eq!(std::fs::read(&bus_path).unwrap(), bus_before_failure);
        assert_eq!(
            projected
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            projection_count_before_failure
        );
        assert_eq!(*delivery.lock().await, delivery_before_failure);

        let formatted = "To jest tekst, który wymaga formatowania.".to_string();
        let commit = emitter
            .apply_formatter_revision(
                "formatter-session".to_string(),
                terminal.reducer_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: formatted.clone(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("applied formatter result must enter revision corridor");
        assert_eq!(commit.rendered_text, formatted);
        assert!(commit.provenance_receipt.starts_with("formatter-"));

        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), source);
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.manual_document_revisions().len(), 2);
        assert_eq!(
            ledger.manual_document_revisions()[0].provenance,
            "light-plus"
        );
        drop(ledger);
        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let version = projected
            .iter()
            .rev()
            .find(|event| event.reducer_action == "derived_projection")
            .unwrap();
        assert_eq!(version.rendered_text, source);
        assert_eq!(version.delivery_text.as_deref(), Some(formatted.as_str()));
        assert!(version.reducer_revision > commit.source_revision);
        let history = crate::presentation::transcript_bus::document_history_at(
            &bus_path,
            "formatter-session",
        )
        .unwrap();
        assert!(history.iter().any(|entry| entry.rendered_text == source));
        assert!(
            history
                .iter()
                .any(|entry| entry.revision == commit.revision && entry.rendered_text == formatted)
        );
    }

    #[tokio::test]
    async fn agent_channel_five_iwo_pcm_to_ledger_reducer_and_delivery() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("p0_b_five_iwo_manifest.json")).unwrap(),
        )
        .unwrap();
        let pcm = hound::WavReader::open(root.join("p0_b_five_iwo.wav"))
            .unwrap()
            .samples::<i16>()
            .map(|sample| f64::from(sample.unwrap()) / 32_768.0)
            .collect::<Vec<_>>();
        let mut take = live_take("agent-five-iwo");
        let calibration = EnergyCalibration::new(
            "p0-b-synthetic-energy-v1",
            manifest["minimum_energy_integral"].as_f64().unwrap(),
            manifest["minimum_valley_samples"].as_u64().unwrap(),
        );
        let bursts = manifest["bursts"].as_array().unwrap();
        assert_eq!(bursts.len(), 5);
        for (index, burst) in bursts.iter().enumerate() {
            let start = burst["sample_start"].as_u64().unwrap();
            let end = burst["sample_end"].as_u64().unwrap();
            let samples = &pcm[start as usize..end as usize];
            let energy = samples.iter().map(|sample| sample * sample).sum::<f64>();
            let rms = (energy / samples.len() as f64).sqrt();
            let peak = samples
                .iter()
                .map(|sample| sample.abs())
                .fold(0.0_f64, f64::max);
            let occurrence = OccurrenceIdentity::new("agent-five-iwo", 1, start, end);
            let (mutation, seal) = {
                let mut ledger = take.ledger.lock().unwrap();
                assert!(
                    ledger
                        .qualify(
                            &AcousticEvidence {
                                occurrence: occurrence.clone(),
                                duration_ms: (end - start) as f64 / 16.0,
                                energy_integral: energy,
                                mean_rms_dbfs: 20.0 * rms.log10(),
                                peak_dbfs: 20.0 * peak.log10(),
                                vad_open_sample: Some(start),
                                vad_close_sample: Some(end),
                                evidence_calibration_version: calibration.version.clone(),
                            },
                            &calibration
                        )
                        .is_qualified()
                );
                ledger.schedule_frontier(
                    occurrence.clone(),
                    [ObservationProducer::Apple, ObservationProducer::Whisper],
                );
                let observation = ObservationIdentity::new(
                    ObservationProducer::Apple,
                    index as u64 + 1,
                    0,
                    occurrence.clone(),
                );
                let receipt = ledger.admit(&observation, "Iwo");
                assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
                let seal = ledger.seal(&occurrence).unwrap().clone();
                (
                    EngineEvent::LedgerMutation {
                        observation,
                        label: "Iwo".into(),
                        receipt,
                    },
                    seal,
                )
            };
            take.emitter.on_event(&mutation);
            take.emitter
                .on_event(&EngineEvent::LedgerSeal { receipt: seal });
        }
        assert_eq!(
            take.ledger.lock().unwrap().qualified_occurrences().count(),
            5
        );
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "agent-five-iwo".into(),
            layer_summary: LayerSummary::default(),
        });
        take.emitter.finish().await;
        assert_eq!(take.delivery.lock().await.as_str(), "Iwo Iwo Iwo Iwo Iwo.");
        let ledger = take.ledger.lock().unwrap();
        assert_eq!(
            ledger
                .qualified_occurrences()
                .filter(|owner| ledger.is_sealed(owner))
                .count(),
            5
        );
        assert!(
            std::fs::read_to_string(&take.bus_path)
                .unwrap()
                .contains("Iwo Iwo Iwo Iwo Iwo.")
        );
    }

    #[tokio::test]
    async fn agent_channel_raw_delivery_uses_whisper_then_light_plus_without_formatter() {
        for (apple, whisper, expected) in [
            (
                "weryfikowałeś",
                "zweryfikowałeś yyy [śmiech]",
                "Zweryfikowałeś yyy [śmiech].",
            ),
            (
                "czy weryfikowałeś",
                "czy plan weryfikowałeś",
                "Czy plan weryfikowałeś.",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("agent.jsonl");
            let bus = Arc::new(
                TranscriptBus::open_at(
                    TranscriptSession {
                        session_id: "agent-raw".into(),
                        mode: TranscriptMode::Agent,
                        has_latched_target: false,
                        latched_target_is_self: false,
                        audience: Some("test-agent".into()),
                        badge_only: true,
                    },
                    path.clone(),
                    None,
                )
                .unwrap(),
            );
            let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
            let delivery = Arc::new(Mutex::new(String::new()));
            let mut emitter = PresentationEmitter::new_with_authority(
                delivery.clone(),
                None,
                None,
                Some(bus.clone()),
                Some(ledger.clone()),
                None,
            )
            .with_refinement_warnings(Arc::new(StdMutex::new(Vec::new())));
            assert!(!emitter.literal_delivery(), "Raw must run Light+");
            emitter.on_capture_opened("agent-raw", 7);
            let occurrence = OccurrenceIdentity::new("agent-raw", 7, 0, 16_000);
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, apple);
                ledger.schedule_frontier(
                    occurrence.clone(),
                    [ObservationProducer::Apple, ObservationProducer::Whisper],
                );
                assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::FrontierOpen));
                mutation
            };
            emitter.on_event(&mutation);
            let rows = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                !rows
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .any(|row| row["reducer_action"] == "record_ledger_terminal_seal"
                        || row["lifecycle_terminal"] == true)
            );
            let (corrected, seal) = {
                let mut ledger = ledger.lock().unwrap();
                let observation = ObservationIdentity::new(
                    ObservationProducer::Whisper,
                    2,
                    0,
                    occurrence.clone(),
                );
                let receipt = ledger.admit(&observation, whisper);
                assert!(receipt.grants_mutation());
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
                let seal = ledger.seal(&occurrence).unwrap().clone();
                (
                    EngineEvent::LedgerMutation {
                        observation,
                        label: whisper.into(),
                        receipt,
                    },
                    seal,
                )
            };
            emitter.on_event(&corrected);
            emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
            let tick_started = std::time::Instant::now();
            let terminal_seal = ledger
                .lock()
                .unwrap()
                .seal_terminal("agent-raw", 7)
                .unwrap();
            emitter.on_event(&EngineEvent::LedgerSeal {
                receipt: terminal_seal,
            });
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .any(|row| row["reducer_action"] == "record_ledger_terminal_seal"
                        && row["rendered_text"] == expected),
                "the agent delivery boundary itself must carry shaped Whisper Raw"
            );
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: "agent-raw".into(),
                layer_summary: LayerSummary::default(),
            });
            assert!(tick_started.elapsed() < std::time::Duration::from_secs(2));
            emitter.finish().await;
            assert_eq!(delivery.lock().await.as_str(), expected);
            let ledger = ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&occurrence), Some(whisper));
            assert!(
                ledger
                    .layer_trail()
                    .iter()
                    .all(|entry| entry.producer() != ObservationProducer::Formatter)
            );
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .any(|row| row["rendered_text"] == expected)
            );
        }
    }

    /// Light+ standard in live: an unpunctuated ledger document gains sentence
    /// shape at the terminal seal, as one `light-plus` document revision that
    /// the Bus persists, the delivery buffer carries, the formatter CAS sees
    /// as its source, and a replay of the Bus bytes reproduces. The ledger's
    /// occurrence labels stay untouched — Rust remains the only author.
    #[tokio::test]
    async fn terminal_seal_mints_light_plus_revision_before_session_end() {
        let fixture = BusFixture::new("light-plus-session", true, "light-plus.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        // Preserve the two-occurrence terminal beside the single-occurrence
        // falsifier. Explicit scope, never cardinality, identifies both.
        let occurrence = OccurrenceIdentity::new("light-plus-session", 6, 0, 16_000);
        let tail_occurrence = OccurrenceIdentity::new("light-plus-session", 6, 16_000, 32_000);
        let raw_words = "to jest tekst bez interpunkcji yyy i koniec";
        let tail_words = "a to jest ogon";
        let (mutation, tail_mutation) = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, raw_words);
            let tail_mutation =
                admitted_mutation(&mut ledger, tail_occurrence.clone(), 2, tail_words);
            for closed in [&occurrence, &tail_occurrence] {
                ledger.schedule_frontier(closed.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(closed, ObservationProducer::Apple));
            }
            (mutation, tail_mutation)
        };
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let mut emitter = fixture.emitter(
            None,
            Some(Arc::new(move |event| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        emitter.on_event(&mutation);
        emitter.on_event(&tail_mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("light-plus-session", 6)
            .expect("closed qualified occurrence must produce terminal seal");
        assert!(
            !terminal_seal.is_occurrence_seal(),
            "this test's subject is the terminal seal, not an occurrence seal"
        );
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        // Production order (controller): the session emits `SessionFinalised`
        // inside recorder stop; `publish_ended` follows in the take reset.
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "light-plus-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        emitter.finish().await;

        let shaped = "To jest tekst bez interpunkcji yyy i koniec a to jest ogon.";
        assert_eq!(delivery.lock().await.as_str(), shaped);

        // The ledger words are untouched; the shaping is a document revision.
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.text_of(&occurrence), Some(raw_words));
        assert_eq!(ledger.text_of(&tail_occurrence), Some(tail_words));
        assert_eq!(ledger.manual_document_revisions().len(), 1);
        let receipt = &ledger.manual_document_revisions()[0];
        assert_eq!(receipt.provenance, "light-plus");
        assert!(receipt.receipt_id.starts_with("light-plus-"));
        assert_eq!(receipt.rendered_text, shaped);
        assert_eq!(
            receipt.source_occurrences,
            vec![occurrence.clone(), tail_occurrence.clone()]
        );
        // Presentation can precede any acoustic seal; terminal Light+ closes it.
        assert!(!ledger.incremental_shapings().is_empty());
        drop(ledger);

        // `session_ended` copies the Light+ revision: Swift's terminal CAS
        // source is the shaped document, and so is the formatter's input.
        assert_eq!(terminal.rendered_text, shaped);
        assert_eq!(terminal.reducer_revision, receipt_revision(&projected));
        assert_eq!(
            emitter
                .terminal_revision_source("light-plus-session", terminal.reducer_revision)
                .expect("light-plus revision is the current terminal source"),
            shaped
        );

        let projected = projected.lock().unwrap_or_else(|error| error.into_inner());
        let light_plus_projection = projected
            .iter()
            .rev()
            .find(|event| event.reducer_action == "apply_manual_edit")
            .expect("light-plus revision projection callback");
        assert_eq!(light_plus_projection.rendered_text, shaped);
        assert_eq!(light_plus_projection.label, tail_words);
        assert_eq!(light_plus_projection.acoustic_receipts.len(), 2);
        for owner in [&occurrence, &tail_occurrence] {
            assert!(
                light_plus_projection
                    .acoustic_receipts
                    .iter()
                    .any(|receipt| {
                        receipt.sample_start == owner.sample_start
                            && receipt.sample_end == owner.sample_end
                    }),
                "the coalesced projection preserves each physical source receipt"
            );
        }
        let mut storage =
            crate::presentation::transcript_bus_maintenance::generation::Decoder::default();
        let mut decoded = Vec::new();
        for line in std::fs::read_to_string(&bus_path).unwrap().lines() {
            let value = serde_json::from_str::<serde_json::Value>(line).unwrap();
            for value in storage.push(value).unwrap() {
                decoded.extend(
                    crate::presentation::transcript_bus_maintenance::generation::rows(value)
                        .unwrap(),
                );
            }
        }
        assert!(!storage.incomplete(), "every stored revision is complete");
        let evidence_rows = decoded
            .into_iter()
            .filter(|row| {
                row["schema"] == "codescribe.transcript-evidence.v1"
                    && row["reducer_action"] == "apply_manual_edit"
                    && row["reducer_revision"].as_u64()
                        == Some(light_plus_projection.reducer_revision)
            })
            .map(|row| row["label"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            evidence_rows,
            vec![raw_words, tail_words],
            "every occurrence keeps its own spoken label in the durable revision"
        );
        assert!(light_plus_projection.terminal);
        assert!(
            light_plus_projection.acoustic_receipts[0]
                .manual_edit_receipt
                .as_deref()
                .is_some_and(|receipt| receipt.starts_with("light-plus-"))
        );
        drop(projected);

        let bus_bytes = std::fs::read(bus_path).unwrap();
        let mut reader = TranscriptProjectionReader::new();
        let replay = reader
            .push_bytes(&bus_bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("light-plus Bus bytes must replay");
        let replayed = replay.last().expect("replayed light-plus revision");
        assert_eq!(replayed.rendered_text, shaped);
        assert!(replayed.terminal);
    }

    fn receipt_revision(projected: &StdMutex<Vec<TranscriptBusEvidenceEvent>>) -> u64 {
        projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .expect("at least one projection")
            .reducer_revision
    }

    /// The literal contract (Ctrl-hold `force_raw`): with literal delivery
    /// declared, the terminal seal mints no Light+ revision and the words
    /// reach delivery exactly as the ledger holds them.
    #[tokio::test]
    async fn literal_delivery_keeps_terminal_words_untouched() {
        let fixture = BusFixture::new("literal-session", true, "literal.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let occurrence = OccurrenceIdentity::new("literal-session", 7, 0, 16_000);
        let raw_words = "słowa literalne bez kropki";
        let mutation = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, raw_words);
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            mutation
        };
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        emitter.set_literal_delivery(true);
        emitter.on_event(&mutation);
        let terminal_seal = ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("literal-session", 7)
            .expect("closed qualified occurrence must produce terminal seal");
        emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        emitter.finish().await;

        assert_eq!(delivery.lock().await.as_str(), raw_words);
        assert_eq!(terminal.rendered_text, raw_words);
        assert!(
            ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .is_empty()
        );
    }

    #[test]
    fn equal_labels_on_disjoint_occurrences_remain_two_document_entries() {
        let mut ledger = AcousticLedger::new();
        let first = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 7, 0, 8_000),
            1,
            "Iwo",
        );
        let second = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 7, 16_000, 24_000),
            2,
            "Iwo",
        );
        let mut reducer = TranscriptReducer::default();

        for event in [first, second] {
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = event
            else {
                unreachable!();
            };
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
        }

        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(reducer.committed_rendered_text(), "Iwo Iwo");

        // The two entries are kept apart by their PCM span, not by their text.
        // Asserting the exact keys stops a future dedup-by-string from passing
        // this test with one entry and a doubled render.
        let spans = reducer
            .document_by_occurrence
            .keys()
            .map(|occurrence| (occurrence.sample_start, occurrence.sample_end))
            .collect::<Vec<_>>();
        assert_eq!(spans, vec![(0, 8_000), (16_000, 24_000)]);
        for entry in reducer.document_by_occurrence.values() {
            assert_eq!(entry.label, "Iwo");
        }
    }

    #[test]
    fn context_marker_rendered_into_document() {
        fn reducer_with_text(text: &str) -> TranscriptReducer {
            let mut ledger = AcousticLedger::new();
            let event = admitted_mutation(
                &mut ledger,
                OccurrenceIdentity::new("session", 1, 0, 16_000),
                1,
                text,
            );
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = event
            else {
                unreachable!();
            };
            let mut reducer = TranscriptReducer::default();
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
            reducer
        }

        let mut boundary = reducer_with_text("alpha beta");
        boundary.record_context_marker(5, "{selection_1}");
        assert_eq!(
            boundary.committed_rendered_text(),
            "alpha {selection_1} beta"
        );

        let mut inside_word = reducer_with_text("bardzo mnie drażni");
        inside_word.record_context_marker(9, "{selection_1}");
        assert_eq!(
            inside_word.committed_rendered_text(),
            "bardzo mn{selection_1}ie drażni"
        );

        let mut ordered = reducer_with_text("alpha");
        ordered.record_context_marker(5, "{selection_1}");
        ordered.record_context_marker(5, "{selection_2}");
        ordered.record_context_marker(5, "{selection_3}");
        assert_eq!(
            ordered.committed_rendered_text(),
            "alpha {selection_1} {selection_2} {selection_3}"
        );

        let mut anchored_before_text = TranscriptReducer::default();
        anchored_before_text.record_context_marker(5, "{selection_1}");
        let mut ledger = AcousticLedger::new();
        let event = admitted_mutation(
            &mut ledger,
            OccurrenceIdentity::new("session", 2, 0, 16_000),
            2,
            "alpha beta",
        );
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = event
        else {
            unreachable!();
        };
        anchored_before_text.apply_ledger_mutation(&ledger, &observation, &receipt);
        assert_eq!(
            anchored_before_text.committed_rendered_text(),
            "alpha {selection_1} beta"
        );
    }

    /// A terminal seal closes committed truth. A later non-manual observation
    /// replaying the same occurrence must move neither the label nor the
    /// document, and must not mint a revision.
    #[test]
    fn sealed_occurrence_refuses_a_later_machine_observation() {
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("session", 11, 0, 16_000);
        let admitted = admitted_mutation(&mut ledger, occurrence.clone(), 1, "Iwo");
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted
        else {
            unreachable!();
        };
        let mut reducer = TranscriptReducer::default();
        let first = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("an authenticated occurrence commits");

        // A seal is only mintable once the scheduled observer frontier has
        // actually closed; there is no arbitrary text seal.
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        // Apple is the only scheduled observer, so its return is the exact
        // open -> closed transition.
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let sealed = ledger
            .seal(&occurrence)
            .expect("a closed frontier over qualified audio seals")
            .clone();
        assert!(reducer.apply_ledger_seal(&sealed).is_some());
        let revision_after_seal = reducer.revision;

        // Same occurrence, later generation, different text: refused outright.
        let replay = ObservationIdentity::new(ObservationProducer::Apple, 2, 1, occurrence.clone());
        let replay_receipt = ledger.admit(&replay, "Iwo drugie");
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &replay, &replay_receipt)
                .is_none()
        );

        assert_eq!(reducer.revision, revision_after_seal);
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
        assert_eq!(reducer.committed_rendered_text(), "Iwo");
        assert_eq!(reducer.document_by_occurrence.len(), 1);
        assert_eq!(first.entries.len(), 1);
    }

    fn unsealed_formatter_source(
        label: &str,
    ) -> (AcousticLedger, TranscriptReducer, OccurrenceIdentity) {
        let occurrence = OccurrenceIdentity::new("formatter-session", 9, 0, 16_000);
        let calibration = EnergyCalibration {
            version: "formatter-emitter-test".to_string(),
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
        let mut ledger = AcousticLedger::new();
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(
            occurrence.clone(),
            [ObservationProducer::Apple, ObservationProducer::Lexicon],
        );
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let apple_receipt = ledger.admit(&apple, label);
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let mut reducer = TranscriptReducer::default();
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &apple, &apple_receipt)
                .is_some()
        );

        let lexicon =
            ObservationIdentity::new(ObservationProducer::Lexicon, 1, 0, occurrence.clone());
        let _ = ledger.admit(&lexicon, label);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
        (ledger, reducer, occurrence)
    }

    fn sealed_formatter_source(
        label: &str,
    ) -> (AcousticLedger, TranscriptReducer, OccurrenceIdentity) {
        let (mut ledger, mut reducer, occurrence) = unsealed_formatter_source(label);
        let seal = ledger.seal(&occurrence).unwrap().clone();
        assert!(reducer.apply_ledger_seal(&seal).is_some());
        (ledger, reducer, occurrence)
    }

    /// rc-w2-composer-turn: a one-turn take is offered to the provider exactly
    /// once, and an empty turn is never offered at all.
    ///
    /// The count that matters is the number of *requests*, so this asserts the
    /// request itself rather than a call counter bolted beside it: no request
    /// means the controller issues no provider call.
    #[test]
    fn terminal_formatter_request_is_one_exact_cas_pair_for_a_nonempty_turn() {
        let (_ledger, mut reducer, _occurrence) = sealed_formatter_source("Iwo");

        assert!(
            reducer.terminal_formatter_request().is_none(),
            "a take that has not reached terminal owes no formatting"
        );

        reducer.mark_terminal_lifecycle();
        let request = reducer
            .terminal_formatter_request()
            .expect("a terminal nonempty turn is owed exactly one formatting pass");
        assert_eq!(request.session_id, "formatter-session");
        assert_eq!(request.source_revision, reducer.revision);
        assert_eq!(request.source_text, reducer.committed_rendered_text());
        assert_eq!(
            reducer
                .terminal_revision_source(&request.session_id, request.source_revision)
                .expect("the request must authenticate against the CAS pair it carries"),
            request.source_text
        );
    }

    #[test]
    fn terminal_formatter_request_is_absent_for_an_empty_turn() {
        let mut reducer = TranscriptReducer::default();
        reducer.mark_terminal_lifecycle();

        assert!(
            reducer.terminal_formatter_request().is_none(),
            "an empty turn must produce no provider call to refuse afterwards"
        );
    }

    #[test]
    fn preserve_refuse_and_empty_propose_return_formatter_without_fake_observation() {
        for (disposition, proposed_label) in [
            (LabelProposalDisposition::PreserveExisting, ""),
            (LabelProposalDisposition::Refuse, ""),
            (LabelProposalDisposition::Propose, "   "),
        ] {
            let (ledger, mut reducer, occurrence) = sealed_formatter_source("Iwo");
            let raw_revision = reducer.revision;
            let source = ledger
                .layer_trail_for(&occurrence)
                .filter(|decision| decision.decision.grants_mutation())
                .last()
                .unwrap()
                .observation
                .clone();
            let trail_before = ledger.layer_trail_for(&occurrence).count();
            let proposal = OccurrenceLabelProposal::for_existing_occurrence(
                occurrence.session.clone(),
                occurrence.capture_epoch,
                occurrence.sample_start,
                occurrence.sample_end,
                proposed_label,
                disposition,
            )
            .with_source(source, "Iwo".into(), FormattingPolicy::Smart);

            assert!(reducer.apply_occurrence_label_proposal(&ledger, &proposal));
            assert_eq!(reducer.revision, raw_revision);
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), trail_before);
            assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
            assert!(ledger.is_sealed(&occurrence));
            assert_eq!(reducer.committed_rendered_text(), "Iwo");
        }
    }

    #[test]
    fn formatter_proposal_derives_without_relabeling_any_occurrence() {
        let (ledger, mut reducer, occurrence) = sealed_formatter_source("Iwo");
        let raw_revision = reducer.revision;
        let source = ledger
            .layer_trail_for(&occurrence)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let qualified_before = ledger.qualified_occurrences().count();
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Iwo!",
            LabelProposalDisposition::Propose,
        )
        .with_source(source, "Iwo".into(), FormattingPolicy::Smart);

        assert!(reducer.apply_occurrence_label_proposal(&ledger, &proposal));
        assert_eq!(reducer.revision, raw_revision);
        assert_eq!(reducer.derived_projections()[0].rendered_text, "Iwo!");
        assert_eq!(reducer.committed_rendered_text(), "Iwo");
        assert!(
            ledger
                .layer_trail_for(&occurrence)
                .all(|decision| decision.observation.producer != ObservationProducer::Formatter)
        );
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
        assert_eq!(ledger.qualified_occurrences().count(), qualified_before);
        assert_eq!(reducer.document_by_occurrence.len(), 1);
        assert!(ledger.is_sealed(&occurrence));

        let trail_after_seal = ledger.layer_trail_for(&occurrence).count();
        assert!(!reducer.apply_occurrence_label_proposal(&ledger, &proposal));
        assert_eq!(reducer.revision, raw_revision);
        assert_eq!(
            ledger.layer_trail_for(&occurrence).count(),
            trail_after_seal
        );
        assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
        assert_eq!(ledger.qualified_occurrences().count(), qualified_before);
        assert_eq!(reducer.document_by_occurrence.len(), 1);
    }

    #[test]
    fn corrections_lexical_edits_publish_without_changing_raw_or_pcm_pins() {
        let (ledger, mut reducer, occurrence) = sealed_formatter_source("Iwo");
        let raw_revision = reducer.revision;
        let pins = ledger.committed_word_pin_ranges(&occurrence);
        let source = ledger
            .layer_trail_for(&occurrence)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let source_revision = reducer.raw_revision_by_observation[&source];
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Different word",
            LabelProposalDisposition::Propose,
        )
        .with_source(source, "Iwo".into(), FormattingPolicy::Correction);
        assert!(reducer.apply_occurrence_label_proposal(&ledger, &proposal));
        let derived = &reducer.derived_projections()[0];
        assert_eq!(derived.status, "applied");
        assert_eq!(derived.rendered_text, "Different word");
        assert_eq!(derived.source_raw_revision, source_revision);
        assert_eq!(reducer.revision, raw_revision);
        assert_eq!(reducer.committed_rendered_text(), "Iwo");
        assert_eq!(ledger.committed_word_pin_ranges(&occurrence), pins);
        assert!(
            ledger
                .layer_trail_for(&occurrence)
                .all(|decision| decision.observation.producer != ObservationProducer::Formatter)
        );
    }

    #[test]
    fn old_smart_source_is_kept_as_refused_history_without_overwriting_correction() {
        let (mut ledger, mut reducer, occurrence) = unsealed_formatter_source("Iwo");
        let source = ledger
            .layer_trail_for(&occurrence)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Old smart text",
            LabelProposalDisposition::Propose,
        )
        .with_source(source, "Iwo".into(), FormattingPolicy::Smart);
        let correction =
            ObservationIdentity::new(ObservationProducer::Whisper, 2, 2, occurrence.clone());
        let receipt = ledger.admit(&correction, "Iwona");
        reducer
            .apply_ledger_mutation(&ledger, &correction, &receipt)
            .unwrap();
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        let corrected_revision = reducer.revision;
        reducer.apply_occurrence_label_proposal(&ledger, &proposal);
        assert_eq!(reducer.derived_projections()[0].status, "stale_source");
        assert_eq!(reducer.revision, corrected_revision);
        assert_eq!(reducer.committed_rendered_text(), "Iwona");
        assert_eq!(ledger.text_of(&occurrence), Some("Iwona"));
    }

    #[test]
    fn relay_acceptance_stale_smart_cannot_overwrite_manual_human() {
        let (mut ledger, mut reducer, occurrence) = sealed_formatter_source("Iwo");
        reducer.mark_terminal_lifecycle();
        let source = ledger
            .layer_trail_for(&occurrence)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Old Smart",
            LabelProposalDisposition::Propose,
        )
        .with_source(source, "Iwo".into(), FormattingPolicy::Smart);
        let source_revision = reducer.revision;
        reducer
            .apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: occurrence.session.clone(),
                    source_revision,
                    rendered_text: "Iwona".into(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                },
            )
            .unwrap();
        let revision = reducer.revision;
        let slots = ledger.slots_of(&occurrence).unwrap().to_vec();
        reducer.apply_occurrence_label_proposal(&ledger, &proposal);
        assert_eq!(
            reducer.derived_projections().last().unwrap().status,
            "stale_source"
        );
        assert_eq!(reducer.committed_rendered_text(), "Iwona");
        assert_eq!(reducer.revision, revision);
        assert_eq!(ledger.slots_of(&occurrence).unwrap(), slots);
    }

    #[tokio::test]
    async fn smart_deadline_delivers_one_whole_corrections_version_and_keeps_raw() {
        let mut take = live_take("smart-deadline");
        let first = OccurrenceIdentity::new("smart-deadline", 21, 0, 16_000);
        let second = OccurrenceIdentity::new("smart-deadline", 21, 16_000, 32_000);
        take.admit(&first, 1, "pierwsze zdanie");
        take.admit(&second, 2, "drugie zdanie");
        take.seal(&first);
        take.seal(&second);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "smart-deadline".into(),
            layer_summary: LayerSummary::default(),
        });
        let raw = take.emitter.terminal_formatter_request().unwrap();
        // Only one span returned Smart; the other exhausted its deadline.
        {
            let source = take
                .ledger
                .lock()
                .unwrap()
                .layer_trail_for(&first)
                .filter(|decision| decision.decision.grants_mutation())
                .last()
                .unwrap()
                .observation
                .clone();
            let mut reducer = take.emitter.session_state.lock().unwrap();
            reducer.mint_derived_projection(
                "smart-deadline".into(),
                raw.source_revision,
                "pierwsze zdanie".into(),
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Smart-only fragment".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
                Some(source),
            );
        }
        let selected = take
            .emitter
            .delivery_projection(FormattingPolicy::Smart, &raw.source_text)
            .unwrap();
        assert_eq!(selected.delivered_mode, "correction");
        assert_eq!(selected.rendered_text, raw.source_text);
        assert!(!selected.rendered_text.contains("Smart-only"));
        assert_eq!(
            take.emitter
                .delivery_projection(FormattingPolicy::Smart, &raw.source_text)
                .unwrap(),
            selected
        );
        assert_eq!(take.emitter.terminal_formatter_request().unwrap(), raw);
        let payload = format!("[dictation] {}", selected.rendered_text);
        take.emitter
            .record_projection_delivery(&payload, TranscriptDelivery::CopiedToClipboard);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&take.bus_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|row: &serde_json::Value| row["schema"] == "codescribe.projection-delivery.v1")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source_raw_revision"], raw.source_revision);
        assert_eq!(rows[0]["source_raw_text"], raw.source_text);
        assert_eq!(rows[0]["requested_mode"], "smart");
        assert_eq!(rows[0]["selected_mode"], "correction");
        assert_eq!(rows[0]["payload_utf8"], payload);
        assert_eq!(rows[0]["disposition"], "copied_to_clipboard");
        // A late successful answer is retained as another available version,
        // but must not revise the already selected handoff or emit delivery.
        {
            let mut reducer = take.emitter.session_state.lock().unwrap();
            let count = reducer.derived_projections().len();
            reducer.mint_derived_projection(
                raw.session_id.clone(),
                raw.source_revision,
                raw.source_text.clone(),
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Late complete Smart".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
                None,
            );
            assert_eq!(reducer.derived_projections().len(), count + 1);
            assert_eq!(
                reducer.derived_projections().last().unwrap().status,
                "applied"
            );
        }
        assert_eq!(
            take.emitter
                .delivery_projection(FormattingPolicy::Smart, &raw.source_text)
                .unwrap(),
            selected
        );
        let deliveries = std::fs::read_to_string(&take.bus_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|row| row["schema"] == "codescribe.projection-delivery.v1")
            .count();
        assert_eq!(deliveries, 1, "late Smart emitted another delivery receipt");
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, raw.source_text);
    }

    #[tokio::test]
    async fn smart_waits_for_in_flight_occurrence_proposals_and_preserves_short_spans() {
        let take = live_take("smart-preserve-and-wait");
        let first = OccurrenceIdentity::new("smart-preserve-and-wait", 22, 0, 16_000);
        let second = OccurrenceIdentity::new("smart-preserve-and-wait", 22, 16_000, 32_000);
        take.admit(&first, 1, "tak");
        take.admit(&second, 2, "drugie zdanie bez kropki");
        take.seal(&first);
        take.seal(&second);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "smart-preserve-and-wait".into(),
            layer_summary: LayerSummary::default(),
        });
        let raw = take.emitter.terminal_formatter_request().unwrap();
        let first_source = take
            .ledger
            .lock()
            .unwrap()
            .layer_trail_for(&first)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let second_source = take
            .ledger
            .lock()
            .unwrap()
            .layer_trail_for(&second)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        take.emitter
            .on_event(&EngineEvent::OccurrenceLabelProposal {
                proposal: OccurrenceLabelProposal::for_existing_occurrence(
                    first.session.clone(),
                    first.capture_epoch,
                    first.sample_start,
                    first.sample_end,
                    "",
                    LabelProposalDisposition::PreserveExisting,
                )
                .with_source(first_source, "tak".into(), FormattingPolicy::Smart),
            });
        take.emitter.on_formatter_pending(true);
        let emitter = Arc::new(take.emitter);
        let emitter_for_job = Arc::clone(&emitter);
        let second_clone = second.clone();
        let job = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            emitter_for_job.on_event(&EngineEvent::OccurrenceLabelProposal {
                proposal: OccurrenceLabelProposal::for_existing_occurrence(
                    second_clone.session,
                    second_clone.capture_epoch,
                    second_clone.sample_start,
                    second_clone.sample_end,
                    "Drugie zdanie z kropką.",
                    LabelProposalDisposition::Propose,
                )
                .with_source(
                    second_source,
                    "drugie zdanie bez kropki".into(),
                    FormattingPolicy::Smart,
                ),
            });
            emitter_for_job.on_formatter_pending(false);
        });
        emitter
            .wait_for_occurrence_projections(
                FormattingPolicy::Smart,
                &raw.source_text,
                std::time::Duration::from_secs(1),
            )
            .await;
        job.await.unwrap();
        let selected = emitter
            .delivery_projection(FormattingPolicy::Smart, &raw.source_text)
            .unwrap();
        assert_eq!(selected.delivered_mode, "smart");
        assert_eq!(selected.status, "applied");
        assert!(selected.rendered_text.ends_with("Drugie zdanie z kropką."));
        if let Ok(mut emitter) = Arc::try_unwrap(emitter) {
            emitter.finish().await;
        }
    }

    /// Acceptance: both emitter request/acknowledgement types keep the derives
    /// their doc comments promise.
    ///
    /// An attribute inserted between a doc comment and its struct stranded
    /// `UserRevisionCommit`'s derives onto the type that followed it, which also
    /// gave that type two identical `derive` attributes. Neither defect is
    /// visible by reading either declaration alone. This exercises `Debug`,
    /// `Clone` and `PartialEq` on both, so the compiler answers the question.
    #[test]
    fn the_emitter_request_and_commit_types_keep_their_declared_derives() {
        let request = TerminalFormatterRequest {
            session_id: "derive-session".to_string(),
            source_revision: 3,
            source_text: "one turn".to_string(),
        };
        assert_eq!(request.clone(), request);
        assert!(format!("{request:?}").contains("derive-session"));

        let commit = UserRevisionCommit {
            session_id: "derive-session".to_string(),
            source_revision: 3,
            revision: 4,
            rendered_text: "one turn.".to_string(),
            provenance_receipt: "formatter-derive".to_string(),
        };
        assert_eq!(commit.clone(), commit);
        assert!(format!("{commit:?}").contains("formatter-derive"));
    }

    // -----------------------------------------------------------------------
    // Live (incremental) Light+ — shaping closed occurrences during capture
    // -----------------------------------------------------------------------

    /// One take wired exactly as the controller wires it: the reducer, the
    /// ledger it is bound to, a real Bus file and a synchronous projection
    /// observer. Nothing here is a stand-in — every assertion below runs
    /// against admitted ledger receipts and published Bus events.
    struct LiveTake {
        delivery: Arc<Mutex<String>>,
        bus: Arc<TranscriptBus>,
        bus_path: std::path::PathBuf,
        ledger: Arc<StdMutex<AcousticLedger>>,
        projected: Arc<StdMutex<Vec<TranscriptBusEvidenceEvent>>>,
        emitter: PresentationEmitter,
        _temp: tempfile::TempDir,
    }

    impl LiveTake {
        /// Qualify and admit one Apple observation, then hand it to the emitter.
        fn admit(&self, occurrence: &OccurrenceIdentity, request: u64, label: &str) {
            let mutation = {
                let mut ledger = self
                    .ledger
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                admitted_mutation(&mut ledger, occurrence.clone(), request, label)
            };
            self.emitter.on_event(&mutation);
        }

        /// Close the occurrence's frontier, seal it in the ledger, and deliver
        /// the resulting occurrence seal to the emitter. Returns the event so a
        /// test can replay the exact same observation.
        fn seal(&self, occurrence: &OccurrenceIdentity) -> EngineEvent {
            let receipt = {
                let mut ledger = self
                    .ledger
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
                ledger
                    .seal(occurrence)
                    .expect("a closed qualified occurrence must seal")
                    .clone()
            };
            assert!(
                receipt.is_occurrence_seal(),
                "this helper produces occurrence seals, never a lifecycle end"
            );
            let event = EngineEvent::LedgerSeal { receipt };
            self.emitter.on_event(&event);
            event
        }

        fn shapings(&self) -> Vec<IncrementalShapingReceipt> {
            self.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .incremental_shapings()
                .to_vec()
        }

        fn shaping_projections(&self) -> Vec<TranscriptBusEvidenceEvent> {
            self.projected
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .filter(|event| event.reducer_action == "apply_incremental_shaping")
                .cloned()
                .collect()
        }
    }

    fn live_take(session_id: &str) -> LiveTake {
        let delivery = Arc::new(Mutex::new(String::new()));
        let temp = tempfile::tempdir().unwrap();
        let bus_path = temp.path().join("incremental-light.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(dictation_session(session_id, true), bus_path.clone(), None)
                .unwrap(),
        );
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let projected = Arc::new(StdMutex::new(Vec::new()));
        let projected_for_callback = Arc::clone(&projected);
        bus.publish_started();
        let emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Some(Arc::new(move |event: &TranscriptBusEvidenceEvent| {
                projected_for_callback
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event.clone());
            })),
        );
        LiveTake {
            delivery,
            bus,
            bus_path,
            ledger,
            projected,
            emitter,
            _temp: temp,
        }
    }

    #[tokio::test]
    async fn pcm_gap_sets_live_sentence_boundary_without_sealing_occurrences() {
        let mut take = live_take("pause-session");
        let first = OccurrenceIdentity::new("pause-session", 21, 0, 16_000);
        let short_gap = OccurrenceIdentity::new("pause-session", 21, 19_200, 35_200);
        let long_gap = OccurrenceIdentity::new("pause-session", 21, 48_000, 64_000);
        take.admit(&first, 1, "pierwsze slowa");
        take.admit(&short_gap, 2, "drugie slowa");
        take.admit(&long_gap, 3, "trzecie slowa");
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze slowa drugie slowa. Trzecie slowa"
        );
        let ledger = take.ledger.lock().unwrap();
        assert!(ledger.seal_of(&first).is_none());
        assert!(ledger.seal_of(&short_gap).is_none());
        assert!(ledger.seal_of(&long_gap).is_none());
        assert_eq!(ledger.incremental_shapings().len(), 2);
        assert!(!ledger.incremental_shapings()[0].sentence_break_before);
        assert!(ledger.incremental_shapings()[1].sentence_break_before);
        assert!(
            ledger.incremental_shapings()[1]
                .source_seal_receipt
                .is_none()
        );
    }

    /// Acceptance: an actual occurrence seal, arriving long before any
    /// lifecycle end, shapes exactly those words and publishes the shaped
    /// bytes to the Bus, the projection callback and the delivery buffer —
    /// while the session stays open.
    ///
    /// This is the claim the whole cut exists for, so it is checked against the
    /// real ledger receipt (provenance, seal, source label) rather than against
    /// a string a helper produced.
    #[tokio::test]
    async fn occurrence_seal_shapes_committed_words_while_the_session_stays_open() {
        let mut take = live_take("live-session");
        let occurrence = OccurrenceIdentity::new("live-session", 11, 0, 16_000);
        let spoken = "to jest pierwsze zdanie";

        take.admit(&occurrence, 1, spoken);
        let seal_event = take.seal(&occurrence);
        let EngineEvent::LedgerSeal { receipt: seal } = &seal_event else {
            panic!("seal helper must produce a ledger seal event");
        };
        let seal_receipt_id = seal.receipt_id.clone();
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "To jest pierwsze zdanie",
            "the closed occurrence reaches delivery shaped, during capture"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1, "exactly one shaping for one seal");
        let shaping = &shapings[0];
        assert_eq!(shaping.provenance, "light-plus");
        assert_eq!(shaping.occurrence, occurrence);
        assert_eq!(shaping.source_label, spoken);
        assert_eq!(shaping.shaped_text, "To jest pierwsze zdanie");
        assert!(shaping.source_seal_receipt.is_none());
        assert_eq!(shaping.revision, shaping.source_revision + 1);

        // The acoustic label is untouched: shaping is presentation, not words.
        let ledger = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(ledger.text_of(&occurrence), Some(spoken));
        assert!(
            ledger.manual_document_revisions().is_empty(),
            "a live shape is not a whole-document edit"
        );
        drop(ledger);

        let projections = take.shaping_projections();
        assert_eq!(projections.len(), 1);
        let projection = &projections[0];
        assert_eq!(projection.rendered_text, "To jest pierwsze zdanie");
        assert_eq!(projection.label, spoken, "the projected label stays spoken");
        assert_eq!(projection.phase, TranscriptProjectionPhase::Listening);
        assert!(!projection.terminal, "a shape is not a terminal revision");
        assert!(
            !projection.lifecycle_terminal,
            "a shape never ends the session"
        );
        assert_eq!(projection.delivery, TranscriptDelivery::Unattempted);
        assert!(
            projection.acoustic_receipts[0]
                .manual_edit_receipt
                .is_none(),
            "a shape must not masquerade as a human correction"
        );
        assert_eq!(
            projection.acoustic_receipts[0].seal_receipt.as_deref(),
            None
        );
        assert_eq!(
            take.ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .map(|seal| seal.receipt_id.as_str().to_owned()),
            Some(seal_receipt_id)
        );

        let bus_bytes = std::fs::read(&take.bus_path).unwrap();
        let text = std::str::from_utf8(&bus_bytes).unwrap();
        assert!(text.contains("apply_incremental_shaping"));
        assert!(
            !text.contains("session_ended"),
            "the lifecycle is still open"
        );
    }

    /// Acceptance: the next occurrence keeps the earlier shape and carries its
    /// own exact words. No stale whole-document override, no prefix loss, and
    /// no text-based deduplication.
    #[tokio::test]
    async fn later_speech_preserves_the_earlier_shape_and_its_own_exact_words() {
        let mut take = live_take("join-session");
        let first = OccurrenceIdentity::new("join-session", 12, 0, 16_000);
        let second = OccurrenceIdentity::new("join-session", 12, 16_000, 32_000);

        take.admit(&first, 1, "to jest pierwsze zdanie");
        take.seal(&first);
        let after_first = take.shapings();

        // Later speech arrives unsealed: the shaped prefix must survive it and
        // the new words must appear exactly as the ledger holds them.
        take.admit(&second, 2, "a to jest drugie");
        let open_projection = take
            .projected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .cloned()
            .expect("the insert publishes a revision");
        assert_eq!(
            open_projection.rendered_text, "To jest pierwsze zdanie a to jest drugie",
            "shaped prefix preserved, open suffix byte-exact"
        );

        let retained = take
            .projected
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|event| event.reducer_revision == open_projection.reducer_revision)
            .flat_map(|event| event.acoustic_receipts.iter())
            .find(|receipt| {
                receipt.sample_start == first.sample_start && receipt.sample_end == first.sample_end
            })
            .unwrap()
            .presentation_receipt
            .clone()
            .unwrap();
        assert_eq!(retained.receipt_id, after_first[0].receipt_id);
        assert_eq!(retained.source_revision, after_first[0].source_revision);
        assert_eq!(retained.shaped_text, after_first[0].shaped_text);
        assert!(
            open_projection
                .acoustic_receipts
                .iter()
                .find(|receipt| receipt.sample_start == second.sample_start
                    && receipt.sample_end == second.sample_end)
                .expect("open physical suffix remains present")
                .presentation_receipt
                .is_none()
        );
        take.seal(&second);
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "To jest pierwsze zdanie a to jest drugie",
            "the second span capitalises because its left context closed"
        );

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 1);
        assert_eq!(
            shapings[0], after_first[0],
            "the first shaping receipt is never rewritten"
        );
        assert_eq!(
            take.ledger.lock().unwrap().text_of(&second),
            Some("a to jest drugie")
        );
    }

    /// Acceptance: an unsealed suffix is never shaped and never appears in a
    /// sealed source receipt. Closed geometry and seal history stay immutable.
    #[tokio::test]
    async fn an_open_suffix_is_shaped_without_claiming_a_seal() {
        let mut take = live_take("open-suffix-session");
        let closed = OccurrenceIdentity::new("open-suffix-session", 13, 0, 16_000);
        let open = OccurrenceIdentity::new("open-suffix-session", 13, 16_000, 32_000);
        let open_words = "  jeszcze mowie i nie skonczylem\n\t";

        take.admit(&closed, 1, "pierwsza czesc juz zamknieta");
        take.seal(&closed);
        take.admit(&open, 2, open_words);
        take.emitter.finish().await;

        let shapings = take.shapings();
        assert_eq!(shapings.len(), 2, "both committed occurrences were shaped");
        assert_eq!(shapings[0].occurrence, closed);
        assert!(
            shapings.iter().any(|shaping| shaping.occurrence == open),
            "an open committed occurrence has presentation authority"
        );

        let rendered = take.delivery.lock().await.clone();
        assert!(
            rendered.ends_with("jeszcze mowie i nie skonczylem"),
            "the open suffix keeps its words: {rendered}"
        );
        assert!(rendered.starts_with("Pierwsza czesc juz zamknieta"));

        // The seal the shaping cites is still the one the ledger holds.
        let ledger = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(ledger.seal_of(&closed).is_some());
        assert!(shapings[0].source_seal_receipt.is_none());
        assert!(shapings[1].source_seal_receipt.is_none());
        assert!(ledger.seal_of(&open).is_none());
    }

    /// Acceptance: equal words in distinct occurrences remain intentional
    /// repetition. Shaping is per-occurrence, so nothing can collapse them.
    #[tokio::test]
    async fn equal_words_in_distinct_occurrences_stay_two_document_entries() {
        let mut take = live_take("iwo-session");
        let first = OccurrenceIdentity::new("iwo-session", 14, 0, 16_000);
        let second = OccurrenceIdentity::new("iwo-session", 14, 16_000, 32_000);

        take.admit(&first, 1, "Iwo");
        take.seal(&first);
        take.admit(&second, 2, "Iwo");
        take.seal(&second);
        take.emitter.finish().await;

        assert_eq!(take.delivery.lock().await.as_str(), "Iwo Iwo");
        let shapings = take.shapings();
        assert!(
            shapings.is_empty(),
            "unchanged text needs no shaping receipt"
        );
        let ledger = take.ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&first), Some("Iwo"));
        assert_eq!(ledger.text_of(&second), Some("Iwo"));
        assert!(take.shaping_projections().is_empty());
    }

    /// Acceptance: a duplicate seal observation mints no second revision and no
    /// second receipt. Replaying the identical event must be a no-op.
    #[tokio::test]
    async fn a_replayed_seal_mints_no_duplicate_shaping_revision() {
        let mut take = live_take("replay-session");
        let occurrence = OccurrenceIdentity::new("replay-session", 15, 0, 16_000);

        take.admit(&occurrence, 1, "raz powiedziane zdanie");
        let seal_event = take.seal(&occurrence);
        let revision_before = take.emitter.session_state.lock().unwrap().revision;
        let callbacks_before = take.projected.lock().unwrap().len();
        let bus_before = std::fs::read(&take.bus_path).unwrap();

        take.emitter.on_event(&seal_event);
        take.emitter.on_event(&seal_event);
        take.emitter.finish().await;

        assert_eq!(
            take.emitter.session_state.lock().unwrap().revision,
            revision_before
        );
        assert_eq!(take.projected.lock().unwrap().len(), callbacks_before);
        assert_eq!(std::fs::read(&take.bus_path).unwrap(), bus_before);
        assert_eq!(take.shapings().len(), 1, "one occurrence, one shaping");
        assert_eq!(
            take.shaping_projections().len(),
            1,
            "a replayed seal publishes no second shaping"
        );
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Raz powiedziane zdanie"
        );
    }

    /// Hesitation-only Raw receives sentence casing without losing its label.
    #[tokio::test]
    async fn a_hesitation_only_occurrence_keeps_its_spoken_words() {
        let mut take = live_take("hesitation-session");
        let occurrence = OccurrenceIdentity::new("hesitation-session", 16, 0, 16_000);

        take.admit(&occurrence, 1, "yyy");
        take.seal(&occurrence);
        take.emitter.finish().await;

        assert_eq!(take.shapings().len(), 1);
        assert_eq!(take.delivery.lock().await.as_str(), "Yyy");
        assert_eq!(
            take.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .text_of(&occurrence),
            Some("yyy")
        );
    }

    /// Acceptance: the raw/literal lane stays byte-exact during capture, not
    /// only at the terminal boundary.
    #[tokio::test]
    async fn literal_delivery_keeps_live_occurrences_byte_exact() {
        let mut take = live_take("literal-live-session");
        let occurrence = OccurrenceIdentity::new("literal-live-session", 17, 0, 16_000);
        let spoken = "  surowe  słowa\n\t";

        take.emitter.set_literal_delivery(true);
        take.admit(&occurrence, 1, spoken);
        take.seal(&occurrence);
        take.emitter.finish().await;

        assert!(take.shapings().is_empty());
        assert!(take.shaping_projections().is_empty());
        assert_eq!(take.delivery.lock().await.as_str(), spoken);
    }

    /// Acceptance: a shape describes exactly the words it was taken from. When
    /// a human supersedes the sealed label, the stale shape is dropped and the
    /// human's exact words are rendered — never the old shaped bytes.
    #[tokio::test]
    async fn a_human_relabel_drops_the_stale_shape_instead_of_rendering_it() {
        let mut take = live_take("relabel-session");
        let occurrence = OccurrenceIdentity::new("relabel-session", 18, 0, 16_000);

        take.admit(&occurrence, 1, "iwo");
        take.seal(&occurrence);
        assert_eq!(take.shapings().len(), 1);

        let manual = {
            let mut ledger = take
                .ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let observation = ObservationIdentity::new(
                ObservationProducer::ManualHuman,
                9,
                0,
                occurrence.clone(),
            );
            let receipt = ledger.admit(&observation, "Iwona");
            assert!(
                receipt.grants_mutation(),
                "an explicit human edit may supersede a seal"
            );
            EngineEvent::LedgerMutation {
                observation,
                label: "Iwona".to_string(),
                receipt,
            }
        };
        take.emitter.on_event(&manual);
        take.emitter.finish().await;

        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Iwona",
            "the stale shape must not survive the words it described"
        );
        assert_eq!(
            take.shapings().len(),
            1,
            "dropping a stale shape rewrites no receipt history"
        );
    }

    /// Recovery falsifier: a real terminal is not identified by cardinality.
    /// UNRUN under W1; a whole-session seal opens edit CAS before lifecycle end.
    #[tokio::test]
    async fn a_single_occurrence_terminal_opens_the_current_shaped_cas_source() {
        let mut take = live_take("single-terminal");
        let occurrence = OccurrenceIdentity::new("single-terminal", 19, 0, 16_000);
        take.admit(&occurrence, 1, "jedno zdanie");
        take.seal(&occurrence);
        let seal = take
            .ledger
            .lock()
            .unwrap()
            .seal_terminal("single-terminal", 19)
            .unwrap();
        assert_ne!(
            seal.receipt_id,
            take.ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .unwrap()
                .receipt_id
        );
        take.emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: seal.clone(),
        });
        let revision = take.emitter.session_state.lock().unwrap().revision;
        assert_eq!(
            take.emitter
                .terminal_revision_source("single-terminal", revision),
            Ok("Jedno zdanie.".to_string()),
            "genuine terminal CAS opens before lifecycle end"
        );
        let callbacks = take.projected.lock().unwrap().len();
        take.emitter
            .on_event(&EngineEvent::LedgerSeal { receipt: seal });
        assert_eq!(
            take.emitter.session_state.lock().unwrap().revision,
            revision
        );
        assert_eq!(take.projected.lock().unwrap().len(), callbacks);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "single-terminal".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        take.emitter.finish().await;
        assert_eq!(take.delivery.lock().await.as_str(), "Jedno zdanie.");
        assert_eq!(
            take.emitter
                .terminal_revision_source("single-terminal", terminal.reducer_revision),
            Ok("Jedno zdanie.".to_string()),
            "one occurrence must permit the same authenticated terminal CAS as two"
        );
    }

    #[tokio::test]
    async fn sealed_take_formats_and_restores_before_and_after_lifecycle_end() {
        let mut take = live_take("sealed-revisions");
        let occurrence = OccurrenceIdentity::new("sealed-revisions", 19, 0, 16_000);
        take.admit(&occurrence, 1, "jedno zdanie");
        take.seal(&occurrence);
        let seal = take
            .ledger
            .lock()
            .unwrap()
            .seal_terminal("sealed-revisions", 19)
            .unwrap();
        assert!(!seal.is_occurrence_seal());
        take.emitter
            .on_event(&EngineEvent::LedgerSeal { receipt: seal });

        let source = take.emitter.terminal_formatter_request().unwrap();
        let original = source.source_text.clone();
        let before = take
            .emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                source.source_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Format before end".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("a sealed take admits Format before lifecycle end");
        assert!(before.provenance_receipt.starts_with("formatter-"));
        let restored = take
            .emitter
            .navigate_document(
                source.session_id.clone(),
                take.emitter.document_timeline(&source.session_id).0,
                0,
            )
            .expect("a sealed take admits Restore before lifecycle end");
        assert_eq!(restored.rendered_text, original);
        assert_eq!(
            take.emitter
                .terminal_revision_source(&source.session_id, before.source_revision),
            Err(UserRevisionRefusal::StaleRevision {
                expected: restored.revision,
                actual: before.source_revision
            })
        );
        assert!(restored.provenance_receipt.starts_with("navigation-"));

        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: source.session_id.clone(),
            layer_summary: LayerSummary::default(),
        });
        let ended = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Retained,
            )
            .unwrap();
        let after = take
            .emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                ended.reducer_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Format after end".to_string(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .expect("a sealed take admits Format after lifecycle end");
        assert!(after.provenance_receipt.starts_with("formatter-"));
        let restored = take
            .emitter
            .navigate_document(
                source.session_id.clone(),
                take.emitter.document_timeline(&source.session_id).0,
                0,
            )
            .expect("a sealed take admits Restore after lifecycle end");
        assert_eq!(restored.rendered_text, original);
        assert!(restored.provenance_receipt.starts_with("navigation-"));
        take.emitter.finish().await;
    }

    #[tokio::test]
    async fn late_ledger_words_after_lifecycle_end_still_commit() {
        let mut take = live_take("late-l1");
        let first = OccurrenceIdentity::new("late-l1", 19, 0, 16_000);
        let second = OccurrenceIdentity::new("late-l1", 19, 16_000, 32_000);
        take.admit(&first, 1, "pierwsze zdanie");
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "late-l1".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let before = take.emitter.session_state.lock().unwrap().revision;
        take.admit(&second, 2, "drugie zdanie");
        take.emitter.finish().await;
        assert!(take.emitter.session_state.lock().unwrap().revision > before);
        assert!(take.delivery.lock().await.contains("drugie zdanie"));
    }

    /// Acceptance: Stop delivers the current canonical shaped document exactly
    /// once. A document that live shaping already settled mints no second
    /// whole-document Light+ revision, and the terminal CAS source is the same
    /// bytes Swift already holds.
    #[tokio::test]
    async fn stop_closes_live_presentation_with_one_light_plus_revision() {
        let mut take = live_take("stop-session");
        let first = OccurrenceIdentity::new("stop-session", 19, 0, 16_000);
        let second = OccurrenceIdentity::new("stop-session", 19, 16_000, 32_000);

        take.admit(&first, 1, "pierwsze zdanie tutaj");
        take.seal(&first);
        take.admit(&second, 2, "a potem drugie");
        take.seal(&second);

        let terminal_seal = take
            .ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seal_terminal("stop-session", 19)
            .expect("every occurrence is sealed, so the epoch seals");
        assert!(!terminal_seal.is_occurrence_seal());
        take.emitter.on_event(&EngineEvent::LedgerSeal {
            receipt: terminal_seal,
        });
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "stop-session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = take
            .bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .expect("terminal projection");
        take.emitter.finish().await;

        let shaped = "Pierwsze zdanie tutaj a potem drugie.";
        assert_eq!(take.delivery.lock().await.as_str(), shaped);
        assert_eq!(terminal.rendered_text, shaped);
        assert!(terminal.terminal);
        assert_eq!(
            take.shapings().len(),
            1,
            "only changed presentation mints a live receipt"
        );
        assert!(
            take.ledger
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .manual_document_revisions()
                .len()
                == 1,
            "an already-shaped document mints no duplicate terminal revision"
        );
        assert_eq!(
            take.emitter
                .terminal_revision_source("stop-session", terminal.reducer_revision)
                .expect("the shaped document is the terminal CAS source"),
            shaped
        );
        assert_eq!(
            take.emitter
                .terminal_formatter_request()
                .expect("a non-empty terminal turn is owed one formatter pass")
                .source_text,
            shaped
        );
        assert!(
            take.bus
                .publish_ended(
                    TranscriptSessionEndReason::Completed,
                    true,
                    TranscriptDelivery::Unattempted
                )
                .is_none()
        );
        take.emitter.finish().await;
        assert_eq!(take.delivery.lock().await.as_str(), shaped);
        assert!(
            take.shaping_projections()
                .iter()
                .all(|event| !event.terminal && !event.lifecycle_terminal),
            "no live revision may claim the lifecycle end"
        );
    }

    /// Acceptance: foreign, stale and unsealed sources refuse, and a
    /// whole-document revision keeps ownership of presentation once it exists.
    /// Exercised against the real reducer and ledger, one refusal at a time.
    #[test]
    fn foreign_and_owned_documents_refuse_incremental_shaping() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("refusal-session", 20, 0, 16_000);
        let stranger = OccurrenceIdentity::new("other-session", 20, 0, 16_000);

        // Nothing committed at all.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::UnknownOccurrence)
        );

        let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "jakies slowa");
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = &mutation
        else {
            panic!("admitted_mutation must produce a ledger mutation");
        };
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .is_some()
        );

        // A committed label can be presented before acoustic finality.
        let live = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .expect("committed words can be shaped before a seal");
        assert_eq!(live.rendered_text, "Jakies slowa");

        // A foreign occurrence never reaches the ledger at all.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &stranger),
            Err(IncrementalShapingRefusal::UnknownOccurrence)
        );

        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        ledger.seal(&occurrence).expect("closed occurrence seals");
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::AlreadyShaped)
        );
        assert!(
            reducer
                .shaping_receipt_of(&occurrence)
                .is_some_and(|receipt| receipt.starts_with("light-plus-incremental-"))
        );

        // Idempotent: the same sealed label is not shaped twice.
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &occurrence),
            Err(IncrementalShapingRefusal::AlreadyShaped)
        );
    }

    /// Acceptance: Whisper's hearing of the brand never reaches the user.
    /// A Whisper word correction relabels the occurrence to "CodeScript"; the
    /// live Light+ shape that paint, stop, paste and agent delivery read
    /// presents "Codescribe", and the ledger authenticates that shape by
    /// re-running the same pass over the corrected label.
    #[test]
    fn whisper_heard_brand_is_presented_as_codescribe() {
        use codescribe_core::pipeline::acoustic_ledger::WordPin;
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("brand-session", 22, 0, 32_000);
        let mutation = admitted_mutation(&mut ledger, occurrence.clone(), 1, "pracuję nad kodem");
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = &mutation
        else {
            panic!("admitted_mutation must produce a ledger mutation");
        };
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .is_some()
        );

        record_measured_speech(&mut ledger, "brand-session", 22, 48_000, &[(1_000, 31_000)]);
        let whisper =
            ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, occurrence.clone());
        let receipt = ledger.admit_word_slots_for_tests(
            &whisper,
            &[
                WordPin::new(1_000, 9_000, "pracuję").with_decode_window(0, 48_000),
                WordPin::new(9_000, 16_000, "nad").with_decode_window(0, 48_000),
                WordPin::new(16_000, 31_000, "CodeScript").with_decode_window(0, 48_000),
            ],
        );
        assert!(matches!(receipt, MutationReceipt::Correct { .. }));
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &whisper, &receipt)
                .is_some()
        );
        assert_eq!(ledger.text_of(&occurrence), Some("pracuję nad CodeScript"));

        let shaped = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .expect("the corrected label is shaped");
        assert_eq!(shaped.rendered_text, "Pracuję nad Codescribe");
    }

    /// Acceptance: a formatter that misspells the brand still delivers
    /// "Codescribe"; the published digest covers the corrected bytes.
    #[test]
    fn formatter_versions_spell_the_brand() {
        let mut reducer = TranscriptReducer::default();
        for policy in [FormattingPolicy::Smart, FormattingPolicy::Max] {
            let projection = reducer.mint_derived_projection(
                "brand-format".into(),
                1,
                "pracuję nad kodem".into(),
                policy,
                AiFormatResult {
                    text: "Pracuję nad CodeScript, a Python script zostaje.".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
                None,
            );
            assert_eq!(
                projection.rendered_text,
                "Pracuję nad Codescribe, a Python script zostaje."
            );
            assert_eq!(projection.publication_digest, projection.digest());
        }
    }

    /// Acceptance: a terminal whole-document revision keeps ownership of the
    /// presentation, and an occurrence seal never opens the terminal corridor.
    ///
    /// The explicitly terminal epoch seal is the only door to a user edit.
    /// This two-occurrence case complements the single-occurrence falsifier. Once that edit lands, a live shape addressed
    /// at one of its source occurrences is refused rather than allowed to
    /// overwrite part of the human's document.
    #[test]
    fn a_terminal_document_revision_keeps_ownership_of_the_presentation() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        let first = OccurrenceIdentity::new("owned-session", 21, 0, 16_000);
        let second = OccurrenceIdentity::new("owned-session", 21, 16_000, 32_000);

        let mut latest_revision = 0;
        for (index, (occurrence, label)) in
            [(&first, "surowe slowa"), (&second, "i jeszcze wiecej")]
                .into_iter()
                .enumerate()
        {
            let mutation =
                admitted_mutation(&mut ledger, occurrence.clone(), index as u64 + 1, label);
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = &mutation
            else {
                panic!("admitted_mutation must produce a ledger mutation");
            };
            latest_revision = reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .expect("qualified observation commits")
                .revision;
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
        }

        // Before the terminal seal, the corridor is closed by law.
        assert_eq!(
            reducer.apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: "owned-session".to_string(),
                    source_revision: latest_revision,
                    rendered_text: "Za wczesnie.".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                }
            ),
            Err(UserRevisionRefusal::NotTerminal),
            "an open document must never admit a whole-document edit"
        );

        let terminal = ledger
            .seal_terminal("owned-session", 21)
            .expect("the epoch seals");
        assert!(!terminal.is_occurrence_seal());
        latest_revision = reducer
            .apply_ledger_seal(&terminal)
            .expect("the terminal seal projects")
            .revision;

        let committed = reducer
            .apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: "owned-session".to_string(),
                    source_revision: latest_revision,
                    rendered_text: "Slowa poprawione przez czlowieka.".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                },
            )
            .expect("a terminal document admits an authenticated user edit");
        assert_eq!(committed.rendered_text, "Slowa poprawione przez czlowieka.");

        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &first),
            Err(IncrementalShapingRefusal::DocumentRevisionOwnsPresentation),
            "a live shape must not overwrite part of a committed document edit"
        );
        assert!(ledger.incremental_shapings().is_empty());
    }

    /// Acceptance: an occurrence from another session is refused even when it
    /// somehow shares a reducer. The document's session is the one the first
    /// committed occurrence named; nothing else may shape into it.
    #[test]
    fn an_occurrence_from_a_foreign_session_is_refused() {
        let mut reducer = TranscriptReducer::default();
        let mut ledger = AcousticLedger::new();
        // `OccurrenceIdentity` orders by session first, so `aaa-` is the
        // document's session and `zzz-` is unambiguously the foreigner.
        let native = OccurrenceIdentity::new("aaa-session", 22, 0, 16_000);
        let foreign = OccurrenceIdentity::new("zzz-session", 22, 0, 16_000);

        for (index, (occurrence, label)) in [(&native, "swoje slowa"), (&foreign, "obce slowa")]
            .into_iter()
            .enumerate()
        {
            let mutation =
                admitted_mutation(&mut ledger, occurrence.clone(), index as u64 + 1, label);
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = &mutation
            else {
                panic!("admitted_mutation must produce a ledger mutation");
            };
            let admitted = reducer.apply_ledger_mutation(&ledger, observation, receipt);
            if occurrence == &native {
                assert!(admitted.is_some());
            } else {
                assert!(
                    admitted.is_none(),
                    "a qualified foreign entry is refused at reducer admission"
                );
                // Preserve the predecessor's contaminated-document falsifier
                // explicitly, without weakening production session admission.
                let mut foreign_reducer = TranscriptReducer::default();
                let foreign_revision = foreign_reducer
                    .apply_ledger_mutation(&ledger, observation, receipt)
                    .unwrap();
                reducer
                    .document_by_occurrence
                    .insert(foreign.clone(), foreign_revision.entries[0].clone());
            }
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
            ledger.seal(occurrence).expect("closed occurrence seals");
        }

        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &foreign),
            Err(IncrementalShapingRefusal::ForeignSession)
        );
        assert_eq!(
            reducer.apply_incremental_shaping(&mut ledger, &native),
            Err(IncrementalShapingRefusal::ForeignSession),
            "the entire contaminated document refuses"
        );
        reducer.document_by_occurrence.remove(&foreign);
        assert!(
            reducer
                .apply_incremental_shaping(&mut ledger, &native)
                .is_ok(),
            "the document's own session still shapes"
        );
        let shapings = ledger.incremental_shapings();
        assert_eq!(shapings.len(), 1);
        assert_eq!(shapings[0].occurrence, native);
    }
    #[tokio::test]
    async fn late_predecessor_closure_shapes_in_document_order_with_immutable_context() {
        let mut take = live_take("closure-order");
        let first = OccurrenceIdentity::new("closure-order", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("closure-order", 1, 16_000, 32_000);
        take.admit(&first, 1, "pierwsze zdanie");
        take.admit(&second, 2, "drugie zdanie");
        take.seal(&second);
        assert!(
            take.shapings().len() == 1,
            "an open predecessor does not veto committed presentation"
        );
        take.seal(&first);
        take.emitter.finish().await;
        let receipts = take.shapings();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].occurrence, first);
        assert_eq!(
            take.ledger.lock().unwrap().text_of(&second),
            Some("drugie zdanie")
        );
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie drugie zdanie"
        );
    }

    #[tokio::test]
    async fn late_predecessor_insertion_invalidates_retained_shape_without_rewriting_receipt() {
        let mut take = live_take("late-insert");
        let first = OccurrenceIdentity::new("late-insert", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("late-insert", 1, 16_000, 32_000);
        take.admit(&second, 2, "drugie zdanie");
        take.seal(&second);
        let original = take.shapings()[0].clone();
        take.admit(&first, 1, "pierwsze zdanie");
        let revision = take.projected.lock().unwrap().last().unwrap().clone();
        assert_eq!(revision.rendered_text, "Pierwsze zdanie drugie zdanie");
        assert!(
            revision
                .acoustic_receipts
                .iter()
                .find(|receipt| receipt.sample_start == second.sample_start
                    && receipt.sample_end == second.sample_end)
                .expect("the invalidated successor is not dropped")
                .presentation_receipt
                .is_none()
        );
        take.seal(&first);
        take.emitter.finish().await;
        assert_eq!(
            take.shapings()[0],
            original,
            "historical provenance never changes"
        );
        let receipts = take.shapings();
        let current = receipts.last().unwrap();
        assert_eq!(current.occurrence, first);
        assert_ne!(current.receipt_id, original.receipt_id);
        assert!(current.left_context.is_empty());
        assert_eq!(
            take.delivery.lock().await.as_str(),
            "Pierwsze zdanie drugie zdanie"
        );
    }

    #[tokio::test]
    async fn unchanged_shape_and_late_observer_do_not_mint_publication() {
        let mut take = live_take("unchanged-observer");
        let occurrence = OccurrenceIdentity::new("unchanged-observer", 1, 0, 16_000);
        take.admit(&occurrence, 1, "Already shaped.");
        let seal = take.seal(&occurrence);
        let before = take.emitter.session_state.lock().unwrap().revision;
        let rows = take.projected.lock().unwrap().len();
        let bytes = std::fs::read(&take.bus_path).unwrap();
        let (observation, receipt) = {
            let mut ledger = take.ledger.lock().unwrap();
            let observation =
                ObservationIdentity::new(ObservationProducer::Whisper, 99, 0, occurrence);
            let receipt = ledger.admit(&observation, "late rewrite");
            assert!(!receipt.grants_mutation());
            (observation, receipt)
        };
        take.emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            receipt,
            label: "late rewrite".to_string(),
        });
        take.emitter.on_event(&seal);
        take.emitter.finish().await;
        assert_eq!(take.emitter.session_state.lock().unwrap().revision, before);
        assert_eq!(take.projected.lock().unwrap().len(), rows);
        assert_eq!(std::fs::read(&take.bus_path).unwrap(), bytes);
        assert!(take.shapings().is_empty());
        assert_eq!(take.delivery.lock().await.as_str(), "Already shaped.");
    }

    #[tokio::test]
    async fn forged_snapshot_never_reaches_the_actual_delivery_worker() {
        let mut take = live_take("delivery-forgery");
        let occurrence = OccurrenceIdentity::new("delivery-forgery", 1, 0, 16_000);
        take.admit(&occurrence, 1, "real words");
        take.seal(&occurrence);
        let mut revision = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .record_context_marker(0, "context")
            .unwrap();
        revision.rendered_text = "forged delivery".to_string();
        let rows = take.projected.lock().unwrap().len();
        take.emitter.publish_revision(revision);
        take.emitter.finish().await;
        assert_eq!(take.projected.lock().unwrap().len(), rows);
        assert_eq!(take.delivery.lock().await.as_str(), "Real words");
    }

    #[tokio::test]
    async fn bus_refusal_cannot_update_only_the_delivery_buffer() {
        let mut take = live_take("ended-publication");
        let occurrence = OccurrenceIdentity::new("ended-publication", 1, 0, 16_000);
        take.admit(&occurrence, 1, "real words");
        take.seal(&occurrence);
        take.bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        let revision = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .record_context_marker(0, "late context")
            .unwrap();
        let callbacks = take.projected.lock().unwrap().len();
        take.emitter.publish_revision(revision);
        take.emitter.finish().await;
        assert_eq!(take.projected.lock().unwrap().len(), callbacks);
        assert_eq!(take.delivery.lock().await.as_str(), "Real words");
    }

    /// Unanchored overlap stays on the visible projection at its PCM position,
    /// leaves the neighbour's committed label alone, and does not become a
    /// second document token. A pin wholly inside an admitted range is counted
    /// and is not painted a second time.
    #[test]
    fn unanchored_overlap_is_visible_without_replacing_or_duplicating_a_neighbour() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let beta = OccurrenceIdentity::new("overlap-visible", 1, 24_000, 48_000);
        let gamma = OccurrenceIdentity::new("overlap-visible", 1, 48_000, 72_000);
        for (request, occurrence, label) in [(1, beta.clone(), "beta"), (2, gamma.clone(), "gamma")]
        {
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = admitted_mutation(&mut ledger, occurrence, request, label)
            else {
                unreachable!()
            };
            assert!(
                reducer
                    .apply_ledger_mutation(&ledger, &observation, &receipt)
                    .is_some()
            );
        }
        let straddling = OccurrenceIdentity::new("overlap-visible", 1, 40_000, 56_000);
        let straddle_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, straddling.clone());
        let straddle = ledger.admit(&straddle_observation, "przez granice");
        assert!(matches!(
            &straddle,
            MutationReceipt::KeepVisibleUnanchored { label, occurrence, .. }
                if label == "przez granice" && occurrence == &straddling
        ));
        assert!(!straddle.grants_mutation());
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &straddle_observation, &straddle)
                .is_none()
        );
        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(
            reducer.document_by_occurrence.get(&beta).unwrap().label,
            "beta"
        );
        assert_eq!(
            reducer.document_by_occurrence.get(&gamma).unwrap().label,
            "gamma"
        );
        assert_eq!(
            reducer.visible_projection(),
            "beta przez granice gamma",
            "the straddling phrase stays visible between its neighbours"
        );
        assert_eq!(ledger.text_of(&beta), Some("beta"));
        assert_eq!(ledger.text_of(&gamma), Some("gamma"));

        let covered = OccurrenceIdentity::new("overlap-visible", 1, 32_000, 40_000);
        let covered_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 4, 0, covered);
        let duplicate = ledger.admit(&covered_observation, "beta");
        assert!(matches!(
            duplicate,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &covered_observation, &duplicate)
                .is_none()
        );
        assert_eq!(reducer.document_by_occurrence.len(), 2);
        assert_eq!(
            reducer.visible_projection(),
            "beta przez granice gamma",
            "a word already admitted on the overlapping range is not painted twice"
        );
        let tally = ledger.conservation();
        assert_eq!(tally.observations_in, tally.receipts_out);
        assert_eq!(tally.kept_visible_unanchored, 2);
        assert_eq!(tally.occurrences_held, 2);
    }

    #[test]
    fn forensic_unresolved_word_slot_candidate_remains_visible_as_evidence() {
        use codescribe_core::pipeline::acoustic_ledger::WordPin;
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let owner = OccurrenceIdentity::new("candidate-evidence", 1, 0, 48_000);
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted_mutation(&mut ledger, owner.clone(), 1, "pierwsze słowa")
        else {
            unreachable!()
        };
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .is_some()
        );
        let offered = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, owner.clone());
        let refusal = ledger
            .admit_word_slots_for_tests(&offered, &[WordPin::new(16_000, 24_000, "dodatkowe")]);
        assert!(
            !refusal.grants_mutation(),
            "this witness requires genuine unresolved admission"
        );
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .any(|alternative| alternative.candidate == "dodatkowe"),
            "the real admission must retain its candidate"
        );
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &offered, &refusal)
                .is_none()
        );
        assert_eq!(ledger.text_of(&owner), Some("pierwsze słowa"));
        assert!(
            reducer
                .unanchored_evidence("candidate-evidence", 1)
                .iter()
                .any(|evidence| evidence
                    .text
                    .split_whitespace()
                    .any(|word| word == "dodatkowe")),
            "retaining a candidate only in the ledger trail does not expose it to the human"
        );
    }

    #[test]
    fn forensic_owner_seal_does_not_erase_unresolved_interior_evidence() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let owner = OccurrenceIdentity::new("seal-evidence", 1, 0, 48_000);
        let EngineEvent::LedgerMutation {
            observation,
            receipt,
            ..
        } = admitted_mutation(&mut ledger, owner.clone(), 1, "pierwsze słowa")
        else {
            unreachable!()
        };
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .is_some()
        );
        let interior = OccurrenceIdentity::new("seal-evidence", 1, 16_000, 32_000);
        let offered = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, interior);
        let evidence = ledger.admit(&offered, "dodatkowe dowody");
        assert!(matches!(
            &evidence,
            MutationReceipt::KeepVisibleUnanchored { .. }
        ));
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &offered, &evidence)
                .is_none()
        );
        let before = reducer.unanchored_evidence("seal-evidence", 1);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].text, "dodatkowe dowody");
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Apple));
        let seal = ledger
            .seal(&owner)
            .expect("the original qualified source can seal")
            .clone();
        assert!(reducer.apply_ledger_seal(&seal).is_some());
        assert_eq!(ledger.text_of(&owner), Some("pierwsze słowa"));
        let after = reducer.unanchored_evidence("seal-evidence", 1);
        assert_eq!(
            after.len(),
            1,
            "containing a candidate's PCM range does not account for its words"
        );
        assert_eq!(after[0].text, "dodatkowe dowody");
    }

    /// A later committed paint keeps unanchored evidence that no committed
    /// token covers. Delivery stays the committed words.
    #[tokio::test]
    async fn later_committed_paint_keeps_uncovered_unanchored_evidence() {
        let paints = Arc::new(StdMutex::new(Vec::new()));
        let observed = Arc::clone(&paints);
        let session = "overlap-paint";
        let fixture = BusFixture::new(session, true, "paint.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        bus.publish_started();
        let mut emitter =
            fixture
                .emitter(None, None)
                .with_cursor_observer(Arc::new(move |projection| {
                    observed.lock().unwrap().push(projection.text.clone());
                }));
        emitter.on_capture_opened(session, 1);
        let beta = OccurrenceIdentity::new(session, 1, 24_000, 48_000);
        let gamma = OccurrenceIdentity::new(session, 1, 48_000, 72_000);
        let delta = OccurrenceIdentity::new(session, 1, 80_000, 96_000);
        for (request, occurrence, label) in [(1, beta.clone(), "beta"), (2, gamma.clone(), "gamma")]
        {
            let event = {
                let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                admitted_mutation(&mut ledger, occurrence, request, label)
            };
            emitter.on_event(&event);
        }
        let straddling = OccurrenceIdentity::new(session, 1, 40_000, 56_000);
        let straddle_observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 3, 0, straddling);
        let straddle = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            ledger.admit(&straddle_observation, "przez granice")
        };
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation: straddle_observation,
            label: "przez granice".into(),
            receipt: straddle,
        });
        let later = {
            let mut ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
            admitted_mutation(&mut ledger, delta, 4, "delta")
        };
        emitter.on_event(&later);
        let painted = paints.lock().unwrap().last().cloned().unwrap_or_default();
        assert!(
            painted.split_whitespace().any(|word| word == "przez"),
            "later paint dropped uncovered evidence: {painted}"
        );
        emitter.finish().await;
        let delivered = delivery.lock().await.clone();
        assert!(
            !delivered.split_whitespace().any(|word| word == "przez"),
            "delivery must stay committed-only, got {delivered}"
        );
        assert!(delivered.split_whitespace().any(|word| word == "delta"));
    }

    /// Counterexample A (Roman, 2026-09-24): a refused whole-span Whisper
    /// replacement keeps its exclusive text as `KeepVisibleUnanchored` wholly
    /// inside the Apple occurrence. The text differs from the Apple label, so
    /// hiding it is not duplicate suppression — the receipt exists and the
    /// words must reach a paint. They never reach the canvas string, the Bus,
    /// or delivery.
    #[tokio::test]
    async fn refused_whole_span_whisper_text_reaches_a_paint_as_evidence() {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;

        let session = "refused-span";
        let whisper = "Whisper mówi inaczej";
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let deltas = Arc::new(RecordingDeltaSink::default());
        let projected = Arc::new(StdMutex::new(Vec::<TranscriptBusEvidenceEvent>::new()));
        let projected_for_callback = Arc::clone(&projected);
        let fixture = BusFixture::new(session, true, "refused-span.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        let bus_path = fixture.path.clone();
        bus.publish_started();
        let mut emitter = fixture
            .emitter(
                Some(Arc::clone(&deltas) as Arc<dyn DeltaSink>),
                Some(Arc::new(move |event: &TranscriptBusEvidenceEvent| {
                    projected_for_callback.lock().unwrap().push(event.clone());
                })),
            )
            .with_cursor_observer(Arc::new(move |projection| {
                observed.lock().unwrap().push(projection.clone());
            }));
        emitter.on_capture_opened(session, 1);
        let apple = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let committed = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(&mut ledger, apple.clone(), 1, "Apple mówi tak")
        };
        emitter.on_event(&committed);
        let pin = OccurrenceIdentity::new(session, 1, 16_000, 32_000);
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 2, 1_000, pin.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            whisper,
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: whisper.into(),
            receipt,
        });
        emitter.finish().await;

        let compact = paints
            .lock()
            .unwrap()
            .iter()
            .map(|paint| serde_json::to_string(paint).unwrap())
            .collect::<Vec<_>>();
        let painted = deltas
            .deltas
            .lock()
            .unwrap()
            .iter()
            .map(|delta| delta.delta.clone())
            .collect::<String>();
        assert!(
            compact.iter().any(|json| json.contains(whisper)),
            "the refused Whisper text never reached a paint.\ncompact paints: {compact:#?}\n\
             canvas deltas: {painted:?}"
        );
        assert!(
            !painted.contains(whisper),
            "evidence must stay off the canvas string: {painted:?}"
        );
        assert!(
            projected
                .lock()
                .unwrap()
                .iter()
                .all(|event| !event.rendered_text.contains(whisper)),
            "evidence must never reach a Bus projection"
        );
        let bus_bytes = std::fs::read_to_string(&bus_path).unwrap();
        assert!(
            !bus_bytes.contains(whisper),
            "evidence entered the Bus file"
        );
        assert_eq!(delivery.lock().await.as_str(), "Apple mówi tak");
        assert_eq!(
            ledger.lock().unwrap().text_of(&apple),
            Some("Apple mówi tak")
        );

        let last = paints.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.evidence,
            vec![super::UnanchoredEvidence {
                sample_start: 16_000,
                sample_end: 32_000,
                text: whisper.into(),
                reason: "exclusive_tail_awaiting_whole_span".into(),
            }],
            "the evidence names its PCM range and why it has no authority"
        );
        assert_eq!(
            last.text, "Apple mówi tak",
            "the canvas tail stays canvas-only"
        );
    }

    #[tokio::test]
    async fn forensic_merge_five_decoded_iwo_survive_through_delivery() {
        let mut lost = Vec::new();
        for (fragmented_ingress, contracted) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let session = match (fragmented_ingress, contracted) {
                (false, false) => "B45-coarse-windows",
                (true, false) => "B45-coarse-fragmented",
                (false, true) => "B45-pinned-windows",
                (true, true) => "B45-pinned-fragmented",
            };
            let trace = codescribe_core::pipeline::streaming::forensic_word_conservation_trace(
                fragmented_ingress,
                contracted,
            );
            assert_eq!(trace.len(), 4);
            let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
            let source_ranges = trace[1]
                .0
                .slots_of(&owner)
                .expect("first actual Word-grain completion")
                .iter()
                .map(|word| (word.sample_start, word.sample_end))
                .collect::<Vec<_>>();
            assert_eq!(
                source_ranges.len(),
                5,
                "five independently admitted PCM pins"
            );
            let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
            let observed = Arc::clone(&paints);
            let fixture = BusFixture::new(session, true, "real-iwo-evidence.jsonl", "");
            let mut emitter =
                fixture
                    .emitter(None, None)
                    .with_cursor_observer(Arc::new(move |projection| {
                        observed
                            .lock()
                            .expect("actual paints")
                            .push(projection.clone());
                    }));
            emitter.on_capture_opened(session, 1);
            fixture.bus.publish_started();
            for (snapshot, events) in trace {
                assert_eq!(snapshot.conservation().residue(), 0);
                *fixture.ledger.lock().expect("authoritative snapshot") = snapshot;
                for event in events {
                    emitter.on_event(&event);
                }
            }
            emitter.finish().await;
            let last = paints
                .lock()
                .expect("captured paints")
                .last()
                .cloned()
                .expect("actual compact projection");
            let (raw, sealed) = {
                let ledger = fixture.ledger.lock().expect("actual ledger");
                let final_ranges = ledger
                    .slots_of(&owner)
                    .expect("committed physical sources")
                    .iter()
                    .map(|word| (word.sample_start, word.sample_end))
                    .collect::<Vec<_>>();
                assert_eq!(
                    final_ranges, source_ranges,
                    "{session}: each established PCM pin survives, not just a matching word count"
                );
                (
                    ledger.text_of(&owner).expect("committed Words").to_owned(),
                    ledger.is_sealed(&owner),
                )
            };
            assert!(
                !sealed,
                "{session}: the cancelled EOF residual cannot supply seal evidence"
            );

            let delivered = fixture.delivery.lock().await.clone();

            assert_eq!(
                delivered, last.text,
                "actual emitter delivery must match recorded compact text"
            );

            let count = raw.split_whitespace().filter(|word| *word == "Iwo").count();
            if count != 5 {
                lost.push((fragmented_ingress, contracted, count));
            }
            assert_eq!(
                last.text, "Iwo Iwo Iwo Iwo Iwo",
                "{session}: actual projection"
            );
            assert_eq!(
                delivered, "Iwo Iwo Iwo Iwo Iwo",
                "{session}: actual delivery"
            );
        }
        assert!(
            lost.is_empty(),
            "a completed broad-word window dropped established physical Words: {lost:?}"
        );
    }

    fn late_apple_into(
        emitter: &PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        pin: &OccurrenceIdentity,
        request: u64,
        text: &str,
    ) {
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, request, request, pin.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            text,
            super::NoAuthorityReason::LateAppleWordNotCurrent,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: text.into(),
            receipt,
        });
    }

    /// Replay the same authenticated observation/receipt event, not a new
    /// request that happens to contain the same spelling and PCM range.
    fn replay_late_apple_into(
        emitter: &PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        request: u64,
    ) {
        let event = {
            let ledger = ledger.lock().unwrap();
            let decision = ledger
                .layer_trail()
                .iter()
                .find(|decision| {
                    decision.observation.producer == ObservationProducer::Apple
                        && decision.observation.request == request
                })
                .expect("original authenticated Apple evidence");
            EngineEvent::LedgerMutation {
                observation: decision.observation.clone(),
                label: decision.candidate_label.clone(),
                receipt: decision.decision.clone(),
            }
        };
        emitter.on_event(&event);
    }

    #[tokio::test]
    async fn late_apple_evidence_uses_only_committed_slot_midpoints() {
        for mode in [
            "owner_only",
            "slot",
            "slot_end",
            "omission",
            "preserved_label",
        ] {
            let (mut emitter, ledger) = mirror_session("late");
            let owner = OccurrenceIdentity::new("late", 7, 0, 64_000);
            admit_into(&emitter, &ledger, &owner, 1, "document");
            let pins = [
                (4_000, 8_000, "alpha"),
                (20_000, 24_000, "beta"),
                (36_000, 40_000, "gamma"),
            ];
            for (index, &(start, end, text)) in pins.iter().enumerate() {
                late_apple_into(
                    &emitter,
                    &ledger,
                    &OccurrenceIdentity::new("late", 7, start, end),
                    index as u64 + 2,
                    text,
                );
            }
            let before = emitter.begin_stop_canvas().unwrap();
            assert_eq!(before.text, "document");
            assert_eq!(before.late_apple_word_counts(), (0, 0));
            assert_eq!(
                emitter.paint_commands.lock().unwrap().last(),
                Some(&before.text)
            );
            // Replaying the actual observation event creates no new evidence
            // and leaves the frozen provenance unchanged.
            replay_late_apple_into(&emitter, &ledger, 2);
            assert_eq!(emitter.visible_canvas_snapshot().unwrap(), before);
            if mode != "owner_only" {
                let observation =
                    ObservationIdentity::new(ObservationProducer::Whisper, 30, 30, owner.clone());
                let receipt = {
                    let mut ledger = ledger.lock().unwrap();
                    match mode {
                        // Midpoint 22_000 is covered by this half-open slot.
                        "slot" => ledger.admit_word_slots_for_tests(
                            &observation,
                            &[codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                                22_000, 23_000, "heard",
                            )],
                        ),
                        "slot_end" => ledger.admit_word_slots_for_tests(
                            &observation,
                            &[codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                                20_000, 22_000, "heard",
                            )],
                        ),
                        "preserved_label" => ledger.admit_word_slots_for_tests(
                            &observation,
                            &[codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                                21_000, 33_000, "document",
                            )],
                        ),
                        _ => ledger.admit(&observation, "Whisper omitted them"),
                    }
                };
                if mode == "preserved_label" {
                    assert!(
                        !receipt.grants_mutation(),
                        "partial identical wording has no new authority"
                    );
                    assert_eq!(ledger.lock().unwrap().text_of(&owner), Some("document"));
                }
                emitter.on_event(&EngineEvent::LedgerMutation {
                    observation,
                    label: String::new(),
                    receipt,
                });
            }
            let after = emitter.visible_canvas_snapshot().unwrap();
            assert_eq!(
                emitter.paint_commands.lock().unwrap().last(),
                Some(&after.text)
            );
            assert_eq!(after.late_apple_word_counts(), (0, 0));
            for word in ["alpha", "beta", "gamma"] {
                assert!(!after.text.split_whitespace().any(|text| text == word));
            }
            let evidence = emitter
                .session_state
                .lock()
                .unwrap()
                .unanchored_evidence("late", 7);
            assert_eq!(
                evidence
                    .iter()
                    .filter(|entry| {
                        entry.reason == super::NoAuthorityReason::LateAppleWordNotCurrent.as_str()
                    })
                    .map(|entry| (entry.sample_start, entry.sample_end, entry.text.as_str()))
                    .collect::<Vec<_>>(),
                pins
            );
            // A separate refused measured-word proposal is also reviewable.
            // It cannot be mistaken for another Apple pin or pasted text.
            for item in evidence.iter().filter(|entry| {
                entry.reason != super::NoAuthorityReason::LateAppleWordNotCurrent.as_str()
            }) {
                let offered = match mode {
                    "slot" | "slot_end" => "heard",
                    "preserved_label" => "document",
                    "omission" => "Whisper omitted them",
                    _ => panic!("no additional producer was offered"),
                };
                assert_eq!(item.text, offered);
                assert_eq!((item.sample_start, item.sample_end), (0, 64_000));
                assert!(
                    ledger
                        .lock()
                        .unwrap()
                        .slot_alternatives()
                        .iter()
                        .any(|alternative| {
                            alternative.candidate == offered
                                && alternative.observation.occurrence == owner
                        }),
                    "the extra evidence belongs to an actual refused producer candidate"
                );
            }
            let missing = before.missing_words_from(&after);
            assert!(!missing.iter().any(|word| word.reason == "unaccounted"));
            assert!(missing.iter().all(|word| word.word != "beta"));
            // Partial pins in this fixture have no completed decode window.
            // A pending recovery cannot be forced into a seal for a paint test.
            if ledger.lock().unwrap().text_recovery_pending(&owner) {
                let mut ledger = ledger.lock().unwrap();
                ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&owner, ObservationProducer::Apple));
                assert_eq!(
                    ledger.seal(&owner).unwrap_err(),
                    codescribe_core::pipeline::acoustic_ledger::SealRefusal::TextRecoveryPending
                );
                assert!(!ledger.is_sealed(&owner));
            } else {
                seal_into(&emitter, &ledger, &owner);
            }
            replay_late_apple_into(&emitter, &ledger, 2);
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: "late".into(),
                layer_summary: LayerSummary::default(),
            });
            let terminal = emitter.finish_stop_canvas().unwrap();
            assert_eq!(terminal.text, after.text);
            assert_eq!(
                terminal.late_apple_word_counts(),
                after.late_apple_word_counts()
            );
            assert_eq!(
                emitter
                    .session_state
                    .lock()
                    .unwrap()
                    .unanchored_evidence("late", 7),
                evidence
            );
            assert_eq!(ledger.lock().unwrap().len(), 1);
            emitter.finish().await;
        }
    }

    #[tokio::test]
    async fn late_apple_evidence_five_iwo_and_same_pin_alternatives() {
        let (mut emitter, ledger) = mirror_session("five-late");
        let owner = OccurrenceIdentity::new("five-late", 7, 0, 100_000);
        admit_into(&emitter, &ledger, &owner, 1, "document");
        for i in 0..5 {
            late_apple_into(
                &emitter,
                &ledger,
                &OccurrenceIdentity::new("five-late", 7, 4_000 + i * 16_000, 8_000 + i * 16_000),
                i + 2,
                "Iwo",
            );
        }
        let before = emitter.begin_stop_canvas().unwrap();
        assert_eq!(before.text, "document");
        assert_eq!(before.late_apple_word_counts(), (0, 0));
        replay_late_apple_into(&emitter, &ledger, 2);
        assert_eq!(emitter.finish_stop_canvas().unwrap(), before);
        late_apple_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new("five-late", 7, 4_000, 8_000),
            21,
            "Ewa",
        );
        let after = emitter.visible_canvas_snapshot().unwrap();
        assert_eq!(after.late_apple_word_counts(), (0, 0));
        assert_eq!(
            after
                .text
                .split_whitespace()
                .filter(|word| *word == "Iwo")
                .count(),
            0
        );
        assert_eq!(
            emitter
                .session_state
                .lock()
                .unwrap()
                .unanchored_evidence("five-late", 7)
                .len(),
            6
        );
        emitter.finish().await;
    }

    #[tokio::test]
    async fn evidence_without_late_apple_keeps_existing_bytes() {
        for reason in [
            super::NoAuthorityReason::OverlapWithoutWordPins,
            super::NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
            super::NoAuthorityReason::ZeroWidth,
        ] {
            let (mut emitter, ledger) = mirror_session("unchanged");
            let owner = OccurrenceIdentity::new("unchanged", 7, 0, 64_000);
            admit_into(&emitter, &ledger, &owner, 1, "Document, exactly.");
            for (start, end, text) in [(4_000, 8_000, "covered"), (80_000, 88_000, "uncovered")] {
                let end = if reason == super::NoAuthorityReason::ZeroWidth {
                    start
                } else {
                    end
                };
                let observation = ObservationIdentity::new(
                    ObservationProducer::Apple,
                    start,
                    0,
                    OccurrenceIdentity::new("unchanged", 7, start, end),
                );
                let receipt =
                    ledger
                        .lock()
                        .unwrap()
                        .keep_visible_unanchored(&observation, text, reason);
                emitter.on_event(&EngineEvent::LedgerMutation {
                    observation,
                    label: text.into(),
                    receipt,
                });
            }
            let frozen = emitter.begin_stop_canvas().unwrap();
            assert_eq!(frozen.text, "Document, exactly. uncovered");
            assert_eq!(frozen.preview_only_words, 1);
            assert_eq!(frozen.late_apple_word_counts(), (0, 0));
            let missing = frozen.missing_words_from(&frozen);
            if reason == super::NoAuthorityReason::OverlapWithoutWordPins {
                assert!(
                    missing.is_empty(),
                    "ambiguous overlap has no delivery accounting"
                );
                assert!(
                    !frozen
                        .visible_words
                        .iter()
                        .any(|word| word.word == "covered")
                );
            } else {
                assert_eq!(
                    missing,
                    vec![super::MissingVisibleWord {
                        word: "covered".into(),
                        reason: format!("covered_by_committed occurrence={owner:?}"),
                    }]
                );
            }
            let unresolved = emitter
                .session_state
                .lock()
                .unwrap()
                .unanchored_evidence("unchanged", 7);
            assert_eq!(unresolved.len(), 2);
            assert_eq!(emitter.finish_stop_canvas().unwrap(), frozen);
            seal_into(&emitter, &ledger, &owner);
            assert_eq!(
                emitter
                    .session_state
                    .lock()
                    .unwrap()
                    .unanchored_evidence("unchanged", 7),
                unresolved,
                "a containing seal is not a disposition of unresolved evidence"
            );
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: "unchanged".into(),
                layer_summary: LayerSummary::default(),
            });
            assert_eq!(
                emitter
                    .session_state
                    .lock()
                    .unwrap()
                    .unanchored_evidence("unchanged", 7),
                unresolved,
                "terminal lifecycle is not source accounting"
            );
            emitter.finish().await;
        }
    }

    /// Qualify, admit and hand one Apple occurrence to `emitter`.
    fn admit_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        occurrence: &OccurrenceIdentity,
        request: u64,
        label: &str,
    ) {
        let event = {
            let mut ledger = ledger.lock().unwrap();
            admitted_mutation(&mut ledger, occurrence.clone(), request, label)
        };
        emitter.on_event(&event);
    }

    /// Close the Apple frontier, seal the occurrence, deliver the receipt.
    fn seal_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        occurrence: &OccurrenceIdentity,
    ) {
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
            ledger
                .seal(occurrence)
                .expect("closed qualified occurrence")
                .clone()
        };
        emitter.on_event(&EngineEvent::LedgerSeal { receipt });
    }

    /// Keep one Whisper pin visible without authority and hand it to `emitter`.
    fn keep_visible_into(
        emitter: &super::PresentationEmitter,
        ledger: &Arc<StdMutex<AcousticLedger>>,
        pin: &OccurrenceIdentity,
        request: u64,
        text: &str,
    ) {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, request, 1_000, pin.clone());
        let receipt = ledger.lock().unwrap().keep_visible_unanchored(
            &observation,
            text,
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: text.into(),
            receipt,
        });
    }

    /// Evidence survives later L0 and committed paints, a containing seal,
    /// and the terminal phase without gaining document or delivery authority.
    #[tokio::test]
    async fn unresolved_evidence_survives_seals_and_terminal_without_delivery_authority() {
        let session = "take";
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let fixture = BusFixture::new(session, true, "evidence-life.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        let delivery = Arc::clone(&fixture.delivery);
        bus.publish_started();
        let mut emitter =
            fixture
                .emitter(None, None)
                .with_cursor_observer(Arc::new(move |projection| {
                    observed.lock().unwrap().push(projection.clone());
                }));
        // Literal takes mint no live shape: retained unresolved evidence must
        // remain reviewable without depending on another committed paint.
        emitter.set_literal_delivery(true);
        emitter.on_capture_opened(session, 7);
        let evidence_texts = || {
            paints
                .lock()
                .unwrap()
                .last()
                .map(|paint| {
                    paint
                        .evidence
                        .iter()
                        .map(|item| item.text.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let apple = OccurrenceIdentity::new(session, 7, 0, 48_000);
        let later = OccurrenceIdentity::new(session, 7, 96_000, 112_000);
        admit_into(&emitter, &ledger, &apple, 1, "pierwsze zdanie");
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new(session, 7, 16_000, 32_000),
            2,
            "inna wersja",
        );
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new(session, 7, 60_000, 72_000),
            3,
            "między tokenami",
        );
        assert_eq!(evidence_texts(), ["inna wersja", "między tokenami"]);

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "dalej mówię".into(),
            pin: PreviewPin::open_occurrence(TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 112_000,
                sample_end: 128_000,
            }),
        });
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "an L0 paint keeps the evidence"
        );
        admit_into(&emitter, &ledger, &later, 4, "drugie zdanie");
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "a committed paint keeps the evidence"
        );
        seal_into(&emitter, &ledger, &later);
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "a seal elsewhere does not close it"
        );
        seal_into(&emitter, &ledger, &apple);
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "a containing seal does not settle these independent observations"
        );

        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: session.into(),
            layer_summary: LayerSummary::default(),
        });
        assert_eq!(
            evidence_texts(),
            ["inna wersja", "między tokenami"],
            "unresolved evidence remains available in terminal review"
        );
        emitter.finish().await;
        let delivered = delivery.lock().await.clone();
        assert!(
            !delivered.contains("inna") && !delivered.contains("między"),
            "evidence never reaches delivery: {delivered}"
        );
    }

    /// Recognizer receipts are diagnostic only; the mirror owns visible words.
    #[tokio::test]
    async fn preview_receipt_logs_without_painting_or_exposing_words() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("preview.log");
        let log_file = std::fs::File::create(&log_path).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || log_file.try_clone().unwrap())
            .finish();
        let mut emitter = PresentationEmitter::new(Arc::new(Mutex::new(String::new())), None, None);
        emitter.on_capture_opened("traced", 2);
        let before = emitter.visible_canvas_snapshot().unwrap();
        tracing::subscriber::with_default(subscriber, || {
            emitter.on_event(&EngineEvent::Preview {
                rev: 3,
                text: "tajne słowa".into(),
                pin: PreviewPin::from_segments(TailSampleRange {
                    session: "traced".into(),
                    capture_epoch: 2,
                    sample_start: 4_000,
                    sample_end: 12_000,
                }),
            });
        });
        let log = std::fs::read_to_string(log_path).unwrap();
        assert!(log.contains("Recognizer preview receipt"));
        assert!(!log.contains("tajne"));
        assert_eq!(emitter.visible_canvas_snapshot().unwrap(), before);
        emitter.finish().await;
    }

    #[tokio::test]
    async fn zero_width_mirror_word_is_visible_and_pasted_without_document_authority() {
        let delivery = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(Arc::clone(&delivery), None, None);
        emitter.on_capture_opened("collapsed", 3);
        emitter.on_event(&mirror(
            1,
            mirror_words(
                "preview only",
                400,
                400,
                UnadmittedAppleWordSource::OpenPartial {
                    rev: 1,
                    phrase_id: 1,
                },
            ),
        ));
        let frozen = emitter.begin_stop_canvas().unwrap();
        assert_eq!(frozen.text, "preview only");
        assert_eq!(frozen.preview_only_words, 2);
        assert!(!frozen.has_committed_document);
        emitter.on_event(&EngineEvent::NoSpeech {
            reason: "lane lost".into(),
        });
        assert_eq!(emitter.finish_stop_canvas().unwrap(), frozen);
        emitter.finish().await;
        assert!(delivery.lock().await.is_empty());
    }

    /// Evidence painted under one capture never carries another capture's
    /// ranges: the range would name different audio.
    #[tokio::test]
    async fn evidence_paint_is_bound_to_the_opened_capture() {
        let paints = Arc::new(StdMutex::new(Vec::<super::CompactProjection>::new()));
        let observed = Arc::clone(&paints);
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let emitter =
            emitter_on_ledger(&ledger).with_cursor_observer(Arc::new(move |projection| {
                observed.lock().unwrap().push(projection.clone());
            }));
        emitter.on_capture_opened("take", 7);
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new("take", 8, 0, 16_000),
            1,
            "inna epoka",
        );
        keep_visible_into(
            &emitter,
            &ledger,
            &OccurrenceIdentity::new("take", 7, 0, 16_000),
            2,
            "ta epoka",
        );
        let last = paints.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.evidence
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["ta epoka"]
        );
    }

    // ── A6: per-word confidence → uncertain spans on the projection ──

    use codescribe_core::pipeline::acoustic_ledger::WordPin;
    use codescribe_core::pipeline::word_confidence::{WordConfidence, WordConfidenceSource};

    fn whisper_confidence(value: f32) -> WordConfidence {
        WordConfidence::new(WordConfidenceSource::WhisperTokenLogprob, value, 1)
    }

    /// Qualify the occurrence and admit the pins as one Whisper word-slot
    /// observation, mirroring `admitted_mutation`'s authentication path.
    fn whisper_slot_mutation(
        ledger: &mut AcousticLedger,
        occurrence: OccurrenceIdentity,
        request: u64,
        pins: &[WordPin],
    ) -> (ObservationIdentity, MutationReceipt) {
        let calibration = EnergyCalibration {
            version: "emitter-test".to_string(),
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
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, request, 0, occurrence);
        let receipt = ledger.admit_word_slots_for_tests(&observation, pins);
        (observation, receipt)
    }

    #[test]
    fn authenticated_no_speech_deletion_can_empty_the_document() {
        use codescribe_core::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use codescribe_core::pipeline::acoustic_ledger::SlotTarget;
        use codescribe_core::pipeline::acoustic_ledger::word_verdict::adjudicate_word_pcm;

        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("word-delete", 1, 0, 1_000);
        let (first, admitted) = whisper_slot_mutation(
            &mut ledger,
            occurrence.clone(),
            1,
            &[WordPin::new(0, 1_000, "phantom")],
        );
        assert_eq!(
            reducer
                .apply_ledger_mutation(&ledger, &first, &admitted)
                .unwrap()
                .rendered_text,
            "phantom"
        );
        let target = SlotTarget::from(&ledger.slots_of(&occurrence).unwrap()[0]);
        let speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("word-delete", 1),
            "silero_boundaries",
            AcousticAvailability::Observed {
                observed_samples: 1_000,
            },
            vec![],
        );
        let verdict = adjudicate_word_pcm(&occurrence, &occurrence, &[0.0; 1_000], &speech);
        let deletion =
            ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, occurrence.clone());
        ledger
            .remove_word_with_verdict(&deletion, &target, &verdict)
            .unwrap();
        let receipt = ledger.layer_trail().last().unwrap().decision.clone();
        let revision = reducer
            .apply_ledger_mutation(&ledger, &deletion, &receipt)
            .unwrap();
        assert_eq!(revision.rendered_text, "");
        assert!(revision.uncertain_spans.is_empty());
        assert!(ledger.slots_of(&occurrence).unwrap().is_empty());
    }

    /// The Five-Iwo falsifier for confidence: five identical surface words,
    /// only the third unsure. Text must not be the key — exactly one span,
    /// on the third pin's PCM and at the third word's UTF-16 range.
    #[test]
    fn uncertain_span_tracks_the_third_of_five_identical_words_by_pcm() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("confidence-live", 1, 0, 80_000);
        let pins = (0..5)
            .map(|index| {
                let value = if index == 2 { -1.9 } else { -0.2 };
                WordPin::new(index * 16_000, (index + 1) * 16_000, "Iwo")
                    .with_confidence(whisper_confidence(value))
            })
            .collect::<Vec<_>>();
        let (observation, receipt) = whisper_slot_mutation(&mut ledger, occurrence, 1, &pins);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();

        assert_eq!(revision.rendered_text, "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(revision.uncertain_spans.len(), 1);
        let span = &revision.uncertain_spans[0];
        assert_eq!((span.utf16_start, span.utf16_end), (8, 11));
        assert_eq!(
            (span.slot_sample_start, span.slot_sample_end),
            (32_000, 48_000)
        );
        assert_eq!(span.source, WordConfidenceSource::WhisperTokenLogprob);
        assert!(!span.surface_rewritten);
        let utf16: Vec<u16> = revision.rendered_text.encode_utf16().collect();
        let painted: String =
            String::from_utf16_lossy(&utf16[span.utf16_start as usize..span.utf16_end as usize]);
        assert_eq!(painted, "Iwo");
    }

    #[tokio::test]
    async fn five_iwo_formatter_revision_keeps_reducer_document_and_delivery() {
        let (mut emitter, delivery, ledger) = opened_delivery_take();
        let occurrence = OccurrenceIdentity::new("take", 7, 0, 80_000);
        let pins = (0..5)
            .map(|index| WordPin::new(index * 16_000, (index + 1) * 16_000, "Iwo"))
            .collect::<Vec<_>>();
        let (observation, receipt) =
            whisper_slot_mutation(&mut ledger.lock().unwrap(), occurrence.clone(), 1, &pins);
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: "Iwo Iwo Iwo Iwo Iwo".into(),
            receipt,
        });
        let formatter =
            ObservationIdentity::new(ObservationProducer::Formatter, 2, 0, occurrence.clone());
        let receipt = ledger.lock().unwrap().admit(&formatter, "Iwo");
        assert!(matches!(
            receipt,
            MutationReceipt::Refuse {
                reason: codescribe_core::pipeline::acoustic_ledger::RefuseReason::AuthorityConflict,
                ..
            }
        ));
        emitter.on_event(&EngineEvent::LedgerMutation {
            observation: formatter,
            label: "Iwo".into(),
            receipt,
        });
        assert_eq!(
            ledger.lock().unwrap().slots_of(&occurrence).unwrap().len(),
            5
        );
        assert_eq!(
            emitter.visible_canvas_snapshot().unwrap().text,
            "Iwo Iwo Iwo Iwo Iwo"
        );
        emitter.finish().await;
        assert_eq!(delivery.lock().await.as_str(), "Iwo Iwo Iwo Iwo Iwo");
    }

    /// (c) A lexicon-rewritten surface keeps its acoustic confidence; the
    /// span is marked `surface_rewritten`, not dropped (d5).
    #[test]
    fn lexicon_rewritten_word_keeps_its_acoustic_confidence_span() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("confidence-live", 1, 0, 32_000);
        let pins = vec![
            WordPin::new(0, 16_000, "Żółw")
                .with_confidence(whisper_confidence(-1.9))
                .surface_rewritten(),
            WordPin::new(16_000, 32_000, "stoi").with_confidence(whisper_confidence(-0.1)),
        ];
        let (observation, receipt) = whisper_slot_mutation(&mut ledger, occurrence, 1, &pins);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();

        assert_eq!(revision.rendered_text, "Żółw stoi");
        assert_eq!(revision.uncertain_spans.len(), 1);
        let span = &revision.uncertain_spans[0];
        assert!(span.surface_rewritten);
        // Polish diacritics stay inside the BMP: UTF-16 offsets track chars.
        assert_eq!((span.utf16_start, span.utf16_end), (0, 4));
        assert_eq!(span.value(), -1.9);
    }

    /// (d) A manual whole-document revision retires every span (d6): the
    /// ledger has no text→PCM alignment for the rewritten document.
    #[test]
    fn manual_revision_retires_uncertain_spans() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("confidence-live", 1, 0, 16_000);
        let pins = vec![WordPin::new(0, 16_000, "Iwo").with_confidence(whisper_confidence(-1.9))];
        let (observation, receipt) =
            whisper_slot_mutation(&mut ledger, occurrence.clone(), 1, &pins);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .unwrap();
        assert_eq!(revision.uncertain_spans.len(), 1);
        // A user edit authenticates against a terminal (sealed) source revision.
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
        ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
        let terminal = ledger
            .seal_terminal("confidence-live", 1)
            .expect("the epoch seals");
        let source_revision = reducer
            .apply_ledger_seal(&terminal)
            .expect("terminal seal projects")
            .revision;

        let revised = reducer
            .apply_user_revision(
                &mut ledger,
                &super::UserRevisionIntent {
                    session_id: "confidence-live".to_string(),
                    source_revision,
                    rendered_text: "Iwo poprawione ręcznie".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                },
            )
            .expect("manual revision commits");
        assert_eq!(revised.rendered_text, "Iwo poprawione ręcznie");
        assert!(revised.uncertain_spans.is_empty());
    }

    /// (b) at the projection seam: the same numeric value classifies per
    /// source, and an Apple slot never paints live (d4 + d7).
    #[test]
    fn projection_classifies_per_source_and_apple_never_paints_live() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("confidence-live", 1, 0, 16_000);
        let pins = vec![
            WordPin::new(0, 16_000, "chmura").with_confidence(WordConfidence::new(
                WordConfidenceSource::VendorWordProbability,
                0.3,
                1,
            )),
        ];
        let calibration = EnergyCalibration {
            version: "emitter-test".to_string(),
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
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let cloud =
            ObservationIdentity::new(ObservationProducer::CloudLive, 1, 0, occurrence.clone());
        let receipt = ledger.admit_word_slots_for_tests(&cloud, &pins);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &cloud, &receipt)
            .unwrap();
        assert_eq!(revision.uncertain_spans.len(), 1);
        assert_eq!(
            revision.uncertain_spans[0].source,
            WordConfidenceSource::VendorWordProbability
        );
    }

    /// The same 0.3 as an Apple segment confidence does not paint: Apple has
    /// no threshold (d4) and Apple slots do not paint live (d7) — but the
    /// metric still travels in the ledger slot (producer truth).
    #[test]
    fn apple_slot_carries_confidence_without_painting_live() {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let occurrence = OccurrenceIdentity::new("confidence-live", 1, 0, 16_000);
        let calibration = EnergyCalibration {
            version: "emitter-test".to_string(),
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
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, occurrence.clone());
        let pins = vec![
            WordPin::new(0, 16_000, "jabłko").with_confidence(WordConfidence::new(
                WordConfidenceSource::AppleSegmentConfidence,
                0.3,
                1,
            )),
        ];
        let receipt = ledger.admit_word_slots_for_tests(&apple, &pins);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &apple, &receipt)
            .unwrap();
        assert!(revision.uncertain_spans.is_empty());
        let slots = ledger.slots_of(&occurrence).unwrap();
        assert!(slots.iter().any(|slot| slot.confidence.is_some_and(
            |confidence| confidence.source == WordConfidenceSource::AppleSegmentConfidence
        )));
    }
    #[test]
    fn w0_falsifier_live_formatter_cannot_drop_a_word() {
        let (ledger, mut reducer, occurrence) = sealed_formatter_source("Iwo plan");
        let source = ledger
            .layer_trail_for(&occurrence)
            .filter(|decision| decision.decision.grants_mutation())
            .last()
            .unwrap()
            .observation
            .clone();
        let source_revision = reducer.raw_revision_by_observation[&source];
        assert_eq!(reducer.committed_rendered_text(), "Iwo plan");
        let raw_revision = reducer.revision;
        let slots = ledger.slots_of(&occurrence).unwrap().to_vec();
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            "Iwo",
            LabelProposalDisposition::Propose,
        )
        .with_source(source, "Iwo plan".into(), FormattingPolicy::Smart);
        reducer.apply_occurrence_label_proposal(&ledger, &proposal);
        let derived = reducer.derived_projections().last().unwrap();
        assert_eq!(derived.status, "applied", "exercise an accepted proposal");
        assert_eq!(derived.rendered_text, "Iwo");
        assert_eq!(derived.source_raw_revision, source_revision);
        assert_eq!(reducer.committed_rendered_text(), "Iwo plan");
        assert_eq!(reducer.revision, raw_revision);
        assert_eq!(ledger.slots_of(&occurrence).unwrap(), slots);
    }

    #[tokio::test]
    async fn max_group_and_terminal_light_plus_leave_raw_independent_and_select_max() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let mut take = live_take("max-raw");
        let mut members = Vec::new();
        for (index, text) in ["Iwo yyy [laugh]", "plan"].into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(
                "max-raw",
                19,
                index as u64 * 16_000,
                (index as u64 + 1) * 16_000,
            );
            take.admit(&occurrence, index as u64, text);
            take.seal(&occurrence);
            let ledger = take.ledger.lock().unwrap();
            members.push(ConsultationPresentationMember {
                occurrence: occurrence.clone(),
                source_label: text.into(),
                seal_receipt: ledger.seal_of(&occurrence).unwrap().receipt_id.clone(),
            });
        }
        let before = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .committed_rendered_text();
        let before_revision = take.emitter.session_state.lock().unwrap().revision;
        let projection = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .apply_consultation_presentation(
                &mut take.ledger.lock().unwrap(),
                ConsultationPresentationInput {
                    consultation_id: "Max",
                    turn_id: "max-group",
                    source_revision: 0,
                    revision: 0,
                    members: &members,
                    rendered_text: "Max version",
                },
            )
            .unwrap();
        assert_eq!(projection.source_raw_revision, before_revision);
        assert_eq!(
            take.emitter.session_state.lock().unwrap().revision,
            before_revision
        );
        assert_eq!(
            take.emitter
                .session_state
                .lock()
                .unwrap()
                .committed_rendered_text(),
            before
        );
        take.emitter.publish_derived_paint(&projection);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "max-raw".into(),
            layer_summary: LayerSummary::default(),
        });
        let raw = take.emitter.terminal_formatter_request().unwrap();
        assert_eq!(raw.source_text, "Iwo yyy [laugh] plan.");
        let selected = take
            .emitter
            .delivery_projection(FormattingPolicy::Max, &raw.source_text)
            .unwrap();
        assert_eq!(selected.rendered_text, "Max version");
        assert_eq!(selected.source_raw_text, raw.source_text);
        assert_eq!(selected.source_raw_revision, raw.source_revision);
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, raw.source_text);
    }

    #[tokio::test]
    async fn formatter_queued_during_light_plus_preparation_rejects_stale_source() {
        use codescribe_core::pipeline::light_plus::TickBudget;
        use std::sync::atomic::{AtomicBool, Ordering};
        let take = live_take("tick-formatter-race");
        take.emitter.set_literal_delivery(true);
        let occurrence = OccurrenceIdentity::new("tick-formatter-race", 19, 0, 16_000);
        take.admit(&occurrence, 1, "Iwo yyy [laugh] plan");
        take.seal(&occurrence);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: occurrence.session.clone(),
            layer_summary: LayerSummary::default(),
        });
        take.emitter.set_literal_delivery(false);
        let source = take.emitter.terminal_formatter_request().unwrap();
        let emitter = Arc::new(take.emitter);
        let target = Arc::downgrade(&emitter);
        let source_revision = source.source_revision;
        let started = Arc::new(AtomicBool::new(false));
        let observed = started.clone();
        let formatter = Arc::new(std::sync::Mutex::new(None));
        let pending = formatter.clone();
        let budget = TickBudget::schedule(Arc::new(move || {
            if std::thread::current().name() == Some("light-plus-tick")
                && !observed.swap(true, Ordering::SeqCst)
            {
                let emitter = target.upgrade().unwrap();
                *pending.lock().unwrap() = Some(std::thread::spawn(move || {
                    emitter.apply_formatter_revision(
                        "tick-formatter-race".into(),
                        source_revision,
                        FormattingPolicy::Smart,
                        AiFormatResult {
                            text: "Smart version".into(),
                            reasoning_text: None,
                            status: AiFormatStatus::Applied,
                        },
                    )
                }));
            }
            std::time::Duration::ZERO
        }));
        {
            // A formatter now joins the same ledger-before-reducer corridor;
            // it must wait for this tick, rather than mutate the reducer alone.
            let mut ledger = take.ledger.lock().unwrap();
            assert!(
                emitter
                    .run_light_plus_tick(&mut ledger, &budget, true)
                    .is_some()
            );
            assert_eq!(ledger.manual_document_revisions().len(), 1);
        }
        assert!(started.load(Ordering::SeqCst));
        let result = formatter.lock().unwrap().take().unwrap().join().unwrap();
        assert_eq!(
            result,
            Err(UserRevisionRefusal::StaleRevision {
                expected: source_revision + 1,
                actual: source_revision,
            })
        );
        assert_eq!(
            emitter.document_timeline("tick-formatter-race").0,
            source_revision + 1
        );
        assert!(
            emitter
                .session_state
                .lock()
                .unwrap()
                .derived_projections
                .is_empty()
        );
        let mut emitter = Arc::try_unwrap(emitter).ok().unwrap();
        emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, "Iwo yyy [laugh] plan.");
    }

    #[tokio::test]
    async fn light_plus_deadline_includes_commit_and_rolls_back_before_publication() {
        use codescribe_core::pipeline::light_plus::TickBudget;
        use std::sync::atomic::{AtomicU64, Ordering};
        let mut take = live_take("tick-commit");
        take.emitter.set_literal_delivery(true);
        let occurrence = OccurrenceIdentity::new("tick-commit", 19, 0, 16_000);
        take.admit(&occurrence, 1, "Iwo yyy [laugh] plan");
        take.seal(&occurrence);
        take.emitter.set_literal_delivery(false);
        let main = std::thread::current().id();
        let checks = Arc::new(AtomicU64::new(0));
        let seen = checks.clone();
        let clock = Arc::new(move || {
            let step = if std::thread::current().id() == main {
                seen.fetch_add(1, Ordering::SeqCst)
            } else {
                0
            };
            std::time::Duration::from_millis(if step >= 6 { 2_001 } else { 0 })
        });
        let budget = TickBudget::schedule(clock);
        let before = take.emitter.session_state.lock().unwrap().revision;
        {
            let mut ledger = take.ledger.lock().unwrap();
            assert!(
                take.emitter
                    .run_light_plus_tick(&mut ledger, &budget, true)
                    .is_none()
            );
            assert!(checks.load(Ordering::SeqCst) >= 7);
            assert!(ledger.manual_document_revisions().is_empty());
            assert_eq!(take.emitter.session_state.lock().unwrap().revision, before);
        }
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, "Iwo yyy [laugh] plan");
    }

    #[tokio::test]
    async fn terminal_smart_projection_delivers_selected_bytes_and_preserves_raw_history() {
        let mut take = live_take("overlay-smart-raw");
        let occurrence = OccurrenceIdentity::new("overlay-smart-raw", 19, 0, 16_000);
        take.admit(&occurrence, 1, "Iwo yyy [laugh] plan");
        take.seal(&occurrence);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: occurrence.session.clone(),
            layer_summary: LayerSummary::default(),
        });
        let source = take.emitter.terminal_formatter_request().unwrap();
        assert_eq!(
            take.emitter.apply_user_revision(UserRevisionIntent {
                session_id: source.session_id.clone(),
                source_revision: source.source_revision,
                rendered_text: "Attempt to replace Raw".into(),
                provenance: DocumentRevisionProvenance::Formatter,
            }),
            Err(UserRevisionRefusal::LedgerRefusal(
                "formatter_requires_derived_projection"
            ))
        );
        let commit = take
            .emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                source.source_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Smart version".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .unwrap();
        let current = take.emitter.terminal_formatter_request().unwrap();
        assert_eq!(current.source_revision, source.source_revision + 1);
        assert_eq!(current.source_text, source.source_text);
        assert!(commit.revision >= (1_u64 << 63));
        assert_eq!(
            take.emitter
                .delivery_projection(FormattingPolicy::Smart, &source.source_text)
                .unwrap()
                .rendered_text,
            "Smart version"
        );
        {
            let events = take.projected.lock().unwrap();
            let overlay = events
                .iter()
                .rev()
                .find(|event| event.reducer_action == "derived_projection")
                .unwrap();
            assert_eq!(overlay.rendered_text, source.source_text);
            assert_eq!(overlay.delivery_text.as_deref(), Some("Smart version"));
            assert!(!overlay.lifecycle_terminal);
        }
        let history = crate::presentation::transcript_bus::document_history_at(
            &take.bus_path,
            &source.session_id,
        )
        .unwrap();
        assert!(
            history
                .iter()
                .any(|entry| entry.rendered_text == source.source_text)
        );
        assert!(
            history
                .iter()
                .any(|entry| entry.rendered_text == "Smart version")
        );
        take.emitter.finish().await;
        assert_eq!(*take.delivery.lock().await, source.source_text);
    }

    #[tokio::test]
    async fn light_plus_expired_queue_retains_exact_raw_on_live_and_terminal_paths() {
        use codescribe_core::pipeline::light_plus::TickBudget;
        use std::sync::atomic::{AtomicU64, Ordering};
        for terminal in [false, true] {
            let mut take = live_take("tick-queue");
            take.emitter.set_literal_delivery(true);
            let occurrence = OccurrenceIdentity::new("tick-queue", 19, 0, 16_000);
            take.admit(&occurrence, 1, "Iwo yyy [laugh] plan");
            take.seal(&occurrence);
            take.emitter.set_literal_delivery(false);
            let time = Arc::new(AtomicU64::new(0));
            let clock_time = time.clone();
            let budget = TickBudget::schedule(Arc::new(move || {
                std::time::Duration::from_millis(clock_time.load(Ordering::SeqCst))
            }));
            time.store(2_001, Ordering::SeqCst);
            let before = take.emitter.session_state.lock().unwrap().revision;
            {
                let mut ledger = take.ledger.lock().unwrap();
                take.emitter
                    .run_light_plus_tick(&mut ledger, &budget, terminal);
                assert!(ledger.manual_document_revisions().is_empty());
                assert!(ledger.incremental_shapings().is_empty());
            }
            let rows = std::fs::read_to_string(&take.bus_path).unwrap();
            assert!(rows.contains("codescribe.light-plus-tick.v1"));
            assert!(rows.contains("deadline_exceeded"));
            {
                let events = take.projected.lock().unwrap();
                let receipt = events
                    .iter()
                    .rev()
                    .find(|event| event.reducer_action == "light_plus_tick_deadline")
                    .unwrap();
                assert_eq!(receipt.rendered_text, "Iwo yyy [laugh] plan");
            }
            assert_eq!(take.emitter.session_state.lock().unwrap().revision, before);
            assert_eq!(
                take.emitter
                    .session_state
                    .lock()
                    .unwrap()
                    .committed_rendered_text(),
                "Iwo yyy [laugh] plan"
            );
            take.emitter.finish().await;
            assert_eq!(*take.delivery.lock().await, "Iwo yyy [laugh] plan");
        }
    }

    #[tokio::test]
    async fn light_plus_discards_late_work_without_authority_and_commits_timely_work() {
        use codescribe_core::pipeline::light_plus::TickBudget;
        use std::sync::atomic::{AtomicU64, Ordering};
        for terminal in [false, true] {
            let mut take = live_take("tick-compute");
            take.emitter.set_literal_delivery(true);
            let occurrence = OccurrenceIdentity::new("tick-compute", 19, 0, 16_000);
            take.admit(&occurrence, 1, "Iwo yyy [laugh] plan");
            take.seal(&occurrence);
            take.emitter.set_literal_delivery(false);
            let worker_checks = Arc::new(AtomicU64::new(0));
            let elapsed = Arc::new(AtomicU64::new(0));
            let time = elapsed.clone();
            let budget = TickBudget::schedule(Arc::new(move || {
                if std::thread::current().name() == Some("light-plus-tick")
                    && worker_checks.fetch_add(1, Ordering::SeqCst) >= 2
                {
                    time.store(2_001, Ordering::SeqCst);
                }
                std::time::Duration::from_millis(time.load(Ordering::SeqCst))
            }));
            {
                let mut ledger = take.ledger.lock().unwrap();
                take.emitter
                    .run_light_plus_tick(&mut ledger, &budget, terminal);
                assert!(ledger.incremental_shapings().is_empty());
                assert!(ledger.manual_document_revisions().is_empty());
                assert_eq!(
                    take.emitter
                        .session_state
                        .lock()
                        .unwrap()
                        .committed_rendered_text(),
                    "Iwo yyy [laugh] plan"
                );
                // A new, timely tick is admitted normally; timed-out work owns no
                // live references and cannot replace this version afterwards.
                let timely = TickBudget::default();
                take.emitter
                    .run_light_plus_tick(&mut ledger, &timely, terminal);
                assert_eq!(ledger.text_of(&occurrence), Some("Iwo yyy [laugh] plan"));
            }
            take.emitter.finish().await;
            assert_eq!(
                *take.delivery.lock().await,
                if terminal {
                    "Iwo yyy [laugh] plan."
                } else {
                    "Iwo yyy [laugh] plan"
                }
            );
        }
    }

    #[tokio::test]
    async fn w0_falsifier_smart_cannot_replace_raw_projection() {
        let mut take = live_take("w0-smart-raw");
        let occurrence = OccurrenceIdentity::new("w0-smart-raw", 19, 0, 16_000);
        take.admit(&occurrence, 1, "Iwo plan");
        take.seal(&occurrence);
        take.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: occurrence.session.clone(),
            layer_summary: LayerSummary::default(),
        });
        let source = take.emitter.terminal_formatter_request().unwrap();
        let before = take
            .ledger
            .lock()
            .unwrap()
            .slots_of(&occurrence)
            .unwrap()
            .to_vec();
        take.emitter
            .apply_formatter_revision(
                source.session_id.clone(),
                source.source_revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: "Smart result".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .unwrap();
        let raw_after = take
            .emitter
            .session_state
            .lock()
            .unwrap()
            .committed_rendered_text();
        let selected = take
            .emitter
            .delivery_projection(FormattingPolicy::Smart, &source.source_text)
            .expect("successful Smart must remain selectable");
        take.emitter.finish().await;
        assert_eq!(
            take.ledger.lock().unwrap().slots_of(&occurrence).unwrap(),
            before
        );
        assert_eq!(
            raw_after, source.source_text,
            "Smart changed the common Raw projection despite unchanged slots"
        );
        assert_eq!(selected.delivered_mode, "smart");
        assert_eq!(selected.rendered_text, "Smart result");
        assert_eq!(selected.source_raw_revision, source.source_revision + 1);
        assert_eq!(selected.source_raw_text, source.source_text);
        assert_eq!(*take.delivery.lock().await, source.source_text);
    }

    #[test]
    fn relay_acceptance_max_cannot_replace_raw_or_later_speech() {
        use codescribe_core::pipeline::acoustic_ledger::ConsultationPresentationMember;
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let mut members = Vec::new();
        let mut sources = Vec::new();
        for (index, label) in ["Iwo", "plan", "dalsze słowa"].into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(
                "relay-max",
                1,
                index as u64 * 16_000,
                (index as u64 + 1) * 16_000,
            );
            let EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } = admitted_mutation(&mut ledger, occurrence.clone(), index as u64, label)
            else {
                unreachable!()
            };
            reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .unwrap();
            sources.push((
                occurrence.clone(),
                ledger.slots_of(&occurrence).unwrap().to_vec(),
            ));
            if index < 2 {
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
                let seal = ledger.seal(&occurrence).unwrap().clone();
                reducer.apply_ledger_seal(&seal).unwrap();
                members.push(ConsultationPresentationMember {
                    occurrence,
                    source_label: label.into(),
                    seal_receipt: seal.receipt_id,
                });
            }
        }
        let raw = reducer.committed_rendered_text();
        let raw_revision = reducer.revision;
        let answer = reducer
            .apply_consultation_presentation(
                &mut ledger,
                ConsultationPresentationInput {
                    consultation_id: "Max",
                    turn_id: "relay-max-answer",
                    source_revision: raw_revision,
                    revision: raw_revision + 1,
                    members: &members,
                    rendered_text: "Gotowe polecenie",
                },
            )
            .expect("Max must produce a usable derived answer, not be disabled");
        assert!(answer.rendered_text.contains("Gotowe polecenie"));
        assert!(answer.rendered_text.contains("dalsze słowa"));
        for (occurrence, slots) in sources {
            assert_eq!(ledger.slots_of(&occurrence).unwrap(), slots);
        }
        assert_eq!(reducer.committed_rendered_text(), raw, "Max changed Raw");
        assert_eq!(reducer.revision, raw_revision, "Max advanced Raw revision");
    }
    #[test]
    fn w0_saved_decisions_replay_through_production_ledger_and_reducer() {
        use codescribe_core::pipeline::trail::{
            TrailAdmission, TrailDecision, TrailEvent, TrailRecord,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = AcousticLedger::new();
        let mut original = TranscriptReducer::default();
        let mut rows = Vec::new();
        let wrap = |event| TrailRecord {
            schema: "codescribe.decision-trail.v1".into(),
            session: "w0-replay-iwo".into(),
            capture_epoch: 7,
            observed_ns: 0,
            event,
        };
        rows.push(wrap(TrailEvent::Start {
            sample_clock: "capture_samples".into(),
        }));
        for index in 0..5 {
            let occurrence =
                OccurrenceIdentity::new("w0-replay-iwo", 7, index * 4000, index * 4000 + 3000);
            let calibration = EnergyCalibration::new("w0-replay", 1.0, 1);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 187.5,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(occurrence.sample_start),
                vad_close_sample: Some(occurrence.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            rows.push(wrap(TrailEvent::Qualification {
                evidence,
                calibration,
                capture_rate_hz: Some(16_000),
            }));
            let observation =
                ObservationIdentity::new(ObservationProducer::Apple, index, 0, occurrence.clone());
            let verdict = ledger.admit(&observation, "Iwo");
            original.apply_ledger_mutation(&ledger, &observation, &verdict);
            let receipt = ledger.layer_trail().last().unwrap();
            rows.push(wrap(TrailEvent::Decision {
                decision: Box::new(TrailDecision {
                    ordinal: receipt.ordinal,
                    receipt_id: receipt.receipt_id.clone(),
                    producer: "apple".into(),
                    layer: "apple".into(),
                    observation,
                    candidate_label: "Iwo".into(),
                    candidate_tokens: vec!["Iwo".into()],
                    verdict,
                    predecessor_ordinal: None,
                    clock_lie: receipt.clock_lie,
                    clock_lie_blocker: receipt.clock_lie_blocker.clone(),
                    input: Some(TrailAdmission {
                        source_slots: vec![],
                        offered_slots: None,
                        slot_revision: false,
                        capture_rate_hz: None,
                        non_mutating: None,
                    }),
                    result_slots: ledger.slots_of(&occurrence).unwrap().to_vec(),
                    revision_before: None,
                    revision_after: None,
                    scheduled_ns: None,
                    returned_ns: None,
                    transferred_ns: None,
                }),
            }));
        }
        rows.push(wrap(TrailEvent::End {
            dropped_records: 0,
            persistence_complete: true,
            evidence_complete: false,
        }));
        let path = dir.path().join("synthetic.trail.jsonl");
        let bytes = rows
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, bytes).unwrap();
        let saved = codescribe_core::pipeline::trail::read_trail(&path).unwrap();
        let mut replayed = TranscriptReducer::default();
        let replay_ledger = codescribe_core::pipeline::trail::replay_decisions(
            &saved,
            |ledger, observation, receipt| {
                replayed.apply_ledger_mutation(ledger, observation, receipt);
            },
        )
        .unwrap();
        assert_eq!(replay_ledger.len(), 5);
        assert_eq!(replayed.document_by_occurrence.len(), 5);
        assert_eq!(
            replayed.committed_rendered_text(),
            original.committed_rendered_text()
        );
        assert_eq!(
            replayed
                .committed_rendered_text()
                .split_whitespace()
                .count(),
            5
        );
        assert_eq!(
            codescribe_core::pipeline::trail::render_trace(&saved, Some("Iwo"))
                .lines()
                .filter(|line| line.contains(" introduced:"))
                .count(),
            5
        );
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn linear_live_attempts_persist_four_undo_redo_and_preserve_five_pcm_occurrences() {
        use crate::controller::live_archive::LiveArchiveMirror;
        use codescribe_core::state::history;
        let root = tempfile::TempDir::new().unwrap();
        let _env = codescribe_core::test_isolation::EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            root.path().to_str().unwrap(),
        );
        let session = "linear-five-iwo";
        let fixture = BusFixture::new(session, true, "linear-versions.jsonl", "");
        let bus = Arc::clone(&fixture.bus);
        let ledger = Arc::clone(&fixture.ledger);
        bus.publish_started();
        let mut emitter = fixture.emitter(None, None);
        for index in 0..5 {
            let occurrence =
                OccurrenceIdentity::new(session, 7, index * 16_000, (index + 1) * 16_000);
            let mutation = {
                let mut ledger = ledger.lock().unwrap();
                let mutation = admitted_mutation(&mut ledger, occurrence.clone(), index + 1, "Iwo");
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                mutation
            };
            emitter.on_event(&mutation);
        }
        let seal = ledger.lock().unwrap().seal_terminal(session, 7).unwrap();
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: session.into(),
            layer_summary: LayerSummary::default(),
        });
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        let raw = emitter
            .terminal_revision_source(session, terminal.reducer_revision)
            .unwrap();
        assert_eq!(raw.split_whitespace().count(), 5);
        assert_eq!(ledger.lock().unwrap().len(), 5);
        let day = root.path().join("transcriptions/2026-10-10");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("010000_linear_raw.txt");
        std::fs::write(&path, &raw).unwrap();
        let mut mirror = LiveArchiveMirror::link(session.into(), path.clone(), 0);
        let (_, initial) = emitter.document_timeline(session);
        assert!(
            initial.steps().is_empty(),
            "plain Stop defers anchoring until first operation"
        );
        mirror.sync(initial.steps(), initial.cursor()).unwrap();
        let mut revision = terminal.reducer_revision;
        for _ in 0..3 {
            let accepted = emitter
                .apply_user_revision(UserRevisionIntent {
                    session_id: session.into(),
                    source_revision: revision,
                    rendered_text: raw.clone(),
                    provenance: DocumentRevisionProvenance::Retranscribe,
                })
                .unwrap();
            revision = accepted.revision;
            let (_, timeline) = emitter.document_timeline(session);
            mirror.sync(timeline.steps(), timeline.cursor()).unwrap();
        }
        let before_format = revision;
        emitter
            .apply_formatter_revision(
                session.into(),
                revision,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: raw.clone(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .unwrap();
        let (next_revision, timeline) = emitter.document_timeline(session);
        revision = next_revision;
        assert!(revision > before_format);
        assert_eq!(timeline.steps().len(), 5);
        mirror.sync(timeline.steps(), timeline.cursor()).unwrap();
        assert!(matches!(
            emitter.apply_formatter_revision(
                session.into(),
                before_format,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: raw.clone(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied
                }
            ),
            Err(UserRevisionRefusal::StaleRevision { .. })
        ));
        for target in (0..4).rev().chain(1..5) {
            emitter
                .navigate_document(session.into(), revision, target)
                .unwrap();
            let (next_revision, timeline) = emitter.document_timeline(session);
            revision = next_revision;
            mirror.sync(timeline.steps(), timeline.cursor()).unwrap();
            let reopened = history::read_archived_document(&path).unwrap();
            assert_eq!(reopened.timeline().steps.len(), 5);
            assert_eq!(reopened.timeline().cursor, target);
            assert_eq!(reopened.head_text(), raw);
            assert_eq!(
                ledger.lock().unwrap().len(),
                5,
                "navigation never creates/deletes PCM identities"
            );
        }
        emitter
            .navigate_document(session.into(), revision, 1)
            .unwrap();
        let (source, timeline) = emitter.document_timeline(session);
        mirror.sync(timeline.steps(), timeline.cursor()).unwrap();
        emitter
            .apply_formatter_revision(
                session.into(),
                source,
                FormattingPolicy::Smart,
                AiFormatResult {
                    text: raw.clone(),
                    reasoning_text: None,
                    status: AiFormatStatus::Applied,
                },
            )
            .unwrap();
        let (_, timeline) = emitter.document_timeline(session);
        mirror.sync(timeline.steps(), timeline.cursor()).unwrap();
        assert_eq!(timeline.steps().len(), 3);
        assert_eq!(
            history::read_archived_document(&path)
                .unwrap()
                .timeline()
                .steps
                .len(),
            3
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let document = history::read_archived_document(&path).unwrap();
        history::commit_archived_revision(
            &path,
            document.head_revision(),
            "external edit",
            history::ArchiveRevisionProvenance::UserEdit,
            None,
        )
        .unwrap();
        let bytes = std::fs::read(format!("{}.revisions.jsonl", path.display())).unwrap();
        assert!(mirror.sync(timeline.steps(), timeline.cursor()).is_err());
        assert_eq!(
            mirror.diverged(),
            Some(crate::controller::ArchiveRefusal::ArchiveChangedOutside),
            "the refusal crosses the bridge as a code, never as the error text"
        );
        assert!(mirror.sync(timeline.steps(), timeline.cursor()).is_err());
        assert_eq!(
            std::fs::read(format!("{}.revisions.jsonl", path.display())).unwrap(),
            bytes
        );
        emitter.finish().await;
    }
