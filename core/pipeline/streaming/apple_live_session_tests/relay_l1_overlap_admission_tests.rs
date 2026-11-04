/// Relay L1 overlap admission. Phrase grain is one segment range, the shape
/// real decodes return (`4.0–12.0`). A word pin is a segment the recognizer
/// actually returned; this module does not split phrase text into words.
///
/// Phrase grain is one segment. A pin wholly inside the exclusive tail may
/// relabel that member. A pin wholly inside an already admitted range is
/// `replayed_range_identity`. A pin that still straddles stays whole,
/// visible, and without a second token.
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        AcousticEvidence, EnergyCalibration, MutationReceipt, ObservationIdentity,
        ObservationProducer, OccurrenceIdentity,
    };
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailProviderPayload, TailTimingQuality,
    };
    use tokio::sync::mpsc;

    const RATE: u32 = 16_000;
    /// The capture device's calibrated existence floor. Fixture speech is
    /// 0.2 RMS and fixture silence is digital zero, so the floor reads the
    /// fixture's own PCM the way a calibrated take reads the room.
    const EXISTENCE_FLOOR_RMS: f32 = 0.01;
    /// Room tone between spoken words, about -86 dBFS: under the existence
    /// floor and the active-speech floor and not digital zero, so every
    /// observer measures it quiet. A voice gate's exact zeros measure nothing
    /// (204c8e03).
    const ROOM_TONE_RMS: f32 = 5.0e-5;

    struct Lane {
        state: AppleSealState,
        tx: mpsc::UnboundedSender<EngineEvent>,
        rx: mpsc::UnboundedReceiver<EngineEvent>,
        tail_rx: mpsc::Receiver<TailPatchRequest>,
    }

    // Root-owned decoder/owner boundary controls: measured PCM, real requests.
    // Supplied word hypotheses test admission; no ASR/model-quality claim.
    fn forensic_boundary_capture(
        session: &str,
        owner_start: u64,
        total: u64,
        voiced: &[(u64, u64)],
    ) -> (Lane, OccurrenceIdentity, Vec<TailPatchRequest>, Vec<f32>) {
        let mut lane = open(session);
        let mut pcm = vec![0.0_f32; total as usize];
        for &(start, end) in voiced {
            pcm[start as usize..end as usize].fill(0.2);
        }
        lane.state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&lane.state.capture_energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        lane.state.energy_calibration = Some(EnergyCalibration {
            version: "forensic-boundary-measured-pcm".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        });
        let owner = OccurrenceIdentity::new(session, 1, owner_start, total);
        assert!(qualify_owned_occurrence(&lane.state, &owner));
        let speech = coverage_speech_evidence(&lane.state);
        assert_eq!(speech.availability().observed_samples(), Some(total));
        assert_eq!(
            speech.producer(),
            crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER
        );
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            ledger.record_speech_evidence(&speech);
            assert!(ledger.require_text_recovery(&owner));
        }
        lane.state.pending_events.insert(
            1,
            PendingAppleSeal {
                occurrence: owner.clone(),
                raw_text: String::new(),
                layer1_baseline: String::new(),
                start_ts: owner_start as f32 / RATE as f32,
                end_ts: total as f32 / RATE as f32,
                segments: Vec::new(),
            },
        );
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &owner));
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert!(!requests.is_empty());
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            assert_eq!(request.member_occurrences, vec![(1, owner.clone())]);
        }
        let _ = drain(&mut lane.rx);
        (lane, owner, requests, pcm)
    }

    #[test]
    fn forensic_unrouted_edge_pin_retains_debt_and_late_apple_is_not_finality() {
        // d152 geometry scaled from 48kHz to this fixture's 16kHz. These
        // supplied hypotheses test accounting, not model recognition.
        let session = "unrouted-edge-late-apple";
        let (mut lane, owner, _requests, _) =
            forensic_boundary_capture(session, 14_592, 198_144, &[(0, 198_144)]);
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 71,
                sample_start: 0,
                sample_end: 128_000,
            },
            &[(1, owner.clone())],
            &[word_pin(session, "Opowiedz", 0, 26_880)],
            LedgerObservationProducer::Whisper,
        );
        assert!(
            routes[0].exclusive.is_empty(),
            "the midpoint is before the owner"
        );
        {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            let diagnostic = ledger.recovery_diagnostic(&owner);
            let rejected = diagnostic
                .rejected_pins
                .iter()
                .find(|pin| pin.sample_start == 0 && pin.sample_end == 26_880)
                .expect("the actual rejected edge pin must survive routing");
            assert_eq!(rejected.decode, Some((0, 128_000)));
            assert_eq!(rejected.observation.request, 71);
            assert!(!rejected.accepted_observation);
            assert!(
                rejected
                    .refusal_reasons
                    .iter()
                    .any(|r| r == "decode_window_clipped")
            );
            assert!(ledger.slots_of(&owner).unwrap_or(&[]).is_empty());
            assert!(!ledger.is_sealed(&owner));
        }
        let late = FusionWord {
            confidence: None,
            text: "Opowiedz".into(),
            sample_start: 18_944,
            sample_end: 25_184,
        };
        let current = TranscriptSegment {
            confidence: None,
            text: late.text.clone(),
            start_ts: late.sample_start as f32 / RATE as f32,
            end_ts: late.sample_end as f32 / RATE as f32,
        };
        let outcome = admit_late_apple_words(&mut lane.state, &lane.tx, 72, &[late], &[current]);
        assert_eq!(outcome.admitted_into.get(&owner), Some(&1));
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Opowiedz"));
        assert!(
            ledger.text_recovery_pending(&owner),
            "one Apple label cannot certify all recovery"
        );
        assert_eq!(
            ledger.seal_terminal(session, 1),
            Err(crate::pipeline::acoustic_ledger::SealRefusal::TextRecoveryPending)
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_unrouted_edge_pin_requires_measured_intersection_energy() {
        for measured in [false, true] {
            let session = format!("unrouted-edge-no-energy-{measured}");
            let mut lane = open(&session);
            let owner = OccurrenceIdentity::new(&session, 1, 14_592, 198_144);
            stage(&mut lane, 1, owner.clone(), "mi dalej");
            if measured {
                record_energy(&lane, &[vec![0.0; 198_144]]);
            }
            lane.state.route_overlap_pins(
                &lane.tx,
                AdmitWindow {
                    request_id: 71,
                    sample_start: 0,
                    sample_end: 128_000,
                },
                &[(1, owner.clone())],
                &[word_pin(&session, "Opowiedz", 0, 26_880)],
                LedgerObservationProducer::Whisper,
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.recovery_diagnostic(&owner).rejected_pins.is_empty());
            assert_eq!(ledger.text_of(&owner), Some("mi dalej"));
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_unrouted_edge_pin_does_not_grant_phrase_or_other_backend_authority() {
        for phrase in [false, true] {
            for producer in [
                LedgerObservationProducer::Whisper,
                LedgerObservationProducer::CloudLive,
            ] {
                if !phrase && producer == LedgerObservationProducer::Whisper {
                    continue;
                }
                let session = format!("unrouted-edge-source-{phrase}-{producer:?}");
                let (mut lane, owner, _requests, _) =
                    forensic_boundary_capture(&session, 14_592, 198_144, &[(0, 198_144)]);
                let pin = if phrase {
                    segment(&session, "Opowiedz mi", 0, 26_880)
                } else {
                    word_pin(&session, "Opowiedz", 0, 26_880)
                };
                lane.state.route_overlap_pins(
                    &lane.tx,
                    AdmitWindow {
                        request_id: 71,
                        sample_start: 0,
                        sample_end: 128_000,
                    },
                    &[(1, owner.clone())],
                    &[pin],
                    producer,
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(ledger.recovery_diagnostic(&owner).rejected_pins.is_empty());
                assert!(ledger.slots_of(&owner).unwrap_or(&[]).is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_boundary_owner_straddle_inside_actual_decoder_keeps_word() {
        for mode in BufferMode::BOTH {
            let session = mode.session("forensic-owner-not-decoder-edge");
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 96_768, 144_000, &[(96_000, 140_000)]);
            assert_eq!(requests.len(), 1);
            assert!(requests[0].provider_request.identity.range.sample_start <= 96_000);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(&session, "tak", 96_000, 126_720),
                        word_pin(&session, "poza", 126_720, 140_000),
                    ],
                ),
                9.0,
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("tak poza"));
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 2);
            assert_eq!(
                (slots[0].sample_start, slots[0].sample_end),
                (96_768, 126_720)
            );
            assert_eq!(
                (slots[1].sample_start, slots[1].sample_end),
                (126_720, 140_000)
            );
            assert!(
                slots
                    .iter()
                    .all(|slot| slot.producer == ObservationProducer::Whisper)
            );
            assert_eq!(ledger.conservation().residue(), 0);
            assert!(ledger.word_deletions().is_empty());
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .all(|alternative| alternative.reason != "contradictory_decode_bounds")
            );
        }
    }

    #[test]
    fn forensic_boundary_decoder_straddle_is_diagnostic_without_poisoning_sibling() {
        for mode in BufferMode::BOTH {
            for malformed in [true, false] {
                let session = format!(
                    "{}-{malformed}",
                    mode.session("forensic-decoder-not-owner-edge")
                );
                // The complete sibling and edge-cut word have independent measured
                // bursts. A flat uninterrupted burst cannot prove this partition.
                let (mut lane, owner, requests, _) = forensic_boundary_capture(
                    &session,
                    0,
                    152_000,
                    &[(59_840, 70_080), (120_000, 152_000)],
                );
                assert!(requests.len() >= 2);
                let end = requests[0].provider_request.identity.range.sample_end;
                assert!(end < owner.sample_end);
                let stub_start = end - 24_000;
                let stub_end = if malformed { end + 768 } else { end };
                assert_eq!(end, 144_000);
                let valid_start = 59_840;
                let valid_end = 70_080;
                let completed = completion(
                    &requests[0],
                    vec![
                        word_pin(&session, "poza", valid_start, valid_end),
                        word_pin(&session, "tak", stub_start, stub_end),
                    ],
                );
                assert_eq!(
                    completed.payload.as_ref().unwrap().validate().is_ok(),
                    !malformed
                );
                lane.state.complete_whisper_window(&lane.tx, completed, 9.0);
                let events = drain(&mut lane.rx);
                assert!(lane.state.tail_patch_awaiting_completion() > 0);
                assert!(!lane.state.stop_document_settled());
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                if malformed {
                    assert!(warning_lines(&events).contains("untrusted_evidence"));
                    assert!(ledger.slots_of(&owner).unwrap_or(&[]).is_empty());
                    assert!(whisper_mutations(&events).is_empty());
                } else {
                    assert_eq!(ledger.text_of(&owner), Some("poza"));
                    let slots = ledger.slots_of(&owner).unwrap();
                    assert_eq!(slots.len(), 1);
                    assert_eq!(
                        (slots[0].sample_start, slots[0].sample_end),
                        (valid_start, valid_end)
                    );
                    assert!(!unanchored_label(&events, "tak"));
                    assert!(
                        lane.state.pending_whisper_stubs.is_empty(),
                        "context has no publication authority"
                    );
                }
                assert!(ledger.text_recovery_pending(&owner));
                assert!(!ledger.is_sealed(&owner));
                assert_eq!(ledger.conservation().residue(), 0);
                assert!(ledger.word_deletions().is_empty());
            }
        }
    }

    #[test]
    fn forensic_boundary_honest_overlap_keeps_normalized_word_and_distinct_short_word() {
        for mode in BufferMode::BOTH {
            let session = mode.session("forensic-honest-overlap");
            // Both modes receive the same PCM. Their actual request ranges
            // choose the overlap tested below, without changing policy knobs.
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 0, 152_000, &[(10_000, 140_000)]);
            assert!(requests.len() >= 2);
            let first = &requests[0].provider_request.identity.range;
            let second = &requests[1].provider_request.identity.range;
            let start = second.sample_start + 1_000;
            let end = start + 5_000;
            assert!(first.sample_start <= start && end < first.sample_end);
            assert!(second.sample_start <= start && end < second.sample_end);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(&session, "Szew,", start, end),
                        word_pin(&session, "w", end + 2_000, end + 3_000),
                        word_pin(&session, "dalej", end + 3_000, end + 9_000),
                    ],
                ),
                9.5,
            );
            let _ = drain(&mut lane.rx);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[1],
                    vec![
                        word_pin(&session, "szew", start, end),
                        word_pin(&session, "w", end + 2_000, end + 3_000),
                        word_pin(&session, "dalej", end + 3_000, end + 9_000),
                    ],
                ),
                9.5,
            );
            let events = drain(&mut lane.rx);
            assert!(
                !replay_refusal(&events, "szew"),
                "out-of-band context is ignored"
            );
            assert!(!replay_refusal(&events, "w"));
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 3);
            assert_eq!(
                crate::pipeline::acoustic_ledger::normalize_word_token(&slots[0].text),
                "szew"
            );
            assert_eq!((slots[0].sample_start, slots[0].sample_end), (start, end));
            assert_eq!(slots[1].text, "w");
            assert_eq!(
                (slots[1].sample_start, slots[1].sample_end),
                (end + 2_000, end + 3_000)
            );
            assert_eq!(slots[2].text, "dalej");
            assert_eq!(ledger.conservation().residue(), 0);
            assert!(ledger.word_deletions().is_empty());
        }
    }

    fn assert_replaced_slot_evidence(
        lane: &Lane,
        occurrence: &OccurrenceIdentity,
        losers: &[&str],
    ) {
        let ledger = lane.state.acoustic_ledger.lock().expect("ledger");
        for loser in losers {
            assert!(
                ledger.layer_trail_for(occurrence).any(|entry| {
                    entry.candidate_label == *loser
                        && matches!(
                            entry.decision,
                            MutationReceipt::Refuse {
                                reason: RefuseReason::ReplacedByWhisper,
                                ..
                            }
                        )
                }),
                "missing replacement receipt for {loser}"
            );
            assert!(
                ledger
                    .slots_of(occurrence)
                    .unwrap()
                    .iter()
                    .all(|slot| slot.text != *loser)
            );
        }
        ledger.assert_slot_labels();
    }

    fn open(session: &str) -> Lane {
        let (tx, rx) = mpsc::unbounded_channel();
        let (tail_tx, tail_rx) = mpsc::channel(8);
        let mut state = AppleSealState::new_for_session(RATE, session.to_string(), 1);
        state.tail_patch = Some(tail_tx);
        state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
        Lane {
            state,
            tx,
            rx,
            tail_rx,
        }
    }

    fn stage(lane: &mut Lane, utterance_id: u64, occurrence: OccurrenceIdentity, label: &str) {
        let calibration = EnergyCalibration {
            version: "relay-l1-overlap".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: occurrence.sample_len() as f64 * 1_000.0 / f64::from(RATE),
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .qualify(&evidence, &calibration);
        lane.state.energy_calibration = Some(calibration);
        assert!(
            admit_ledger_label(
                &mut lane.state,
                &lane.tx,
                LabelAdmission {
                    observation: ObservationIdentity::new(
                        ObservationProducer::Apple,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
        lane.state.pending_events.insert(
            utterance_id,
            PendingAppleSeal {
                start_ts: occurrence.sample_start as f32 / RATE as f32,
                end_ts: occurrence.sample_end as f32 / RATE as f32,
                occurrence,
                raw_text: label.to_string(),
                layer1_baseline: label.to_string(),
                segments: Vec::new(),
            },
        );
    }

    struct RelayPiecePcm {
        audio: Vec<f32>,
    }

    fn piece(_utterance_id: u64, occurrence: &OccurrenceIdentity, _text: &str) -> RelayPiecePcm {
        RelayPiecePcm {
            audio: vec![0.2; occurrence.sample_len() as usize],
        }
    }

    fn close_lexicon(
        lane: &mut Lane,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
        label: &str,
    ) {
        assert!(
            admit_ledger_label(
                &mut lane.state,
                &lane.tx,
                LabelAdmission {
                    observation: ObservationIdentity::new(
                        ObservationProducer::Lexicon,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
    }

    fn segment(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
        TimedTailSegment {
            confidence: None,
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            text: text.to_string(),
            range: crate::stt::tail_provider::TailSampleRange {
                session: session.to_string(),
                capture_epoch: 1,
                sample_start: start,
                sample_end: end,
            },
        }
    }

    fn word_pin(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
        let mut pin = segment(session, text, start, end);
        pin.grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        pin
    }

    fn completion(
        request: &TailPatchRequest,
        segments: Vec<TimedTailSegment>,
    ) -> TailPatchCompletion {
        let text = segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let segment_grain = if !segments.is_empty()
            && segments
                .iter()
                .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word)
        {
            crate::stt::tail_provider::TailSegmentGrain::Word
        } else {
            crate::stt::tail_provider::TailSegmentGrain::Phrase
        };
        TailPatchCompletion {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: Some(TailProviderPayload {
                identity: request.provider_request.identity.clone(),
                text,
                segments,
                avg_logprob: Some(-0.2),
                compression_ratio: Some(1.1),
                provider_id: TailProviderId::Fake,
                elapsed_ms: 1,
                evidence: TailProviderEvidence {
                    segment_grain,
                    source: TailEvidenceSource::Whisper,
                    revision: Some("relay-l1-phrase-grain".into()),
                    stability: TailEvidenceStability::Final,
                    timing_quality: TailTimingQuality::ExactSampleRange,
                    avg_logprob: Some(-0.2),
                    vad_selected_samples: None,
                },
            }),
            member_occurrences: request.member_occurrences.clone(),
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn take_requests(rx: &mut mpsc::Receiver<TailPatchRequest>) -> Vec<TailPatchRequest> {
        let mut requests = Vec::new();
        while let Ok(request) = rx.try_recv() {
            requests.push(request);
        }
        requests
    }

    fn whisper_mutations(events: &[EngineEvent]) -> Vec<(String, MutationReceipt)> {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                } if observation.producer == ObservationProducer::Whisper => {
                    Some((label.clone(), receipt.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn unanchored_label(events: &[EngineEvent], text: &str) -> bool {
        whisper_mutations(events)
            .into_iter()
            .any(|(label, receipt)| {
                label == text
                    && matches!(receipt, MutationReceipt::KeepVisibleUnanchored { .. })
                    && !receipt.grants_mutation()
            })
    }

    fn replay_refusal(events: &[EngineEvent], text: &str) -> bool {
        whisper_mutations(events)
            .into_iter()
            .any(|(label, receipt)| {
                label == text
                    && matches!(
                        receipt,
                        MutationReceipt::Refuse { reason, .. }
                            if reason.as_str() == "replayed_range_identity"
                    )
            })
    }

    fn named_refusal(events: &[EngineEvent], reason: &str) -> bool {
        whisper_mutations(events).into_iter().any(|(_, receipt)| {
            matches!(
                receipt,
                MutationReceipt::Refuse { reason: got, .. } if got.as_str() == reason
            )
        })
    }

    fn mutation_count(events: &[EngineEvent]) -> usize {
        whisper_mutations(events)
            .into_iter()
            .filter(|(_, receipt)| receipt.grants_mutation())
            .count()
    }

    fn assert_conserved(lane: &Lane, reason: Option<&str>) {
        let tally = lane
            .state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .conservation();
        assert_eq!(
            tally.observations_in, tally.receipts_out,
            "conservation counts offered observations and issued receipts apart"
        );
        assert!(tally.receipts_out > 0);
        if let Some(reason) = reason {
            assert!(
                tally.refusals_by_reason.get(reason).copied().unwrap_or(0) >= 1,
                "named refusal {reason} missing from {:?}",
                tally.refusals_by_reason
            );
        }
    }

    fn held_count(lane: &Lane) -> usize {
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .occurrences()
            .count()
    }

    fn held_text(lane: &Lane, occurrence: &OccurrenceIdentity) -> Option<String> {
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .text_of(occurrence)
            .map(str::to_owned)
    }

    fn measured_relay_capture(lane: &mut Lane, total: u64) {
        let captured = lane.state.audio.session_sample_end();
        assert!(captured <= total);
        if captured < total {
            let pcm = vec![0.2; (total - captured) as usize];
            lane.state.audio.push(&pcm);
            record_energy(
                lane,
                &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
            );
        }
        assert_eq!(lane.state.audio.session_sample_end(), total);
        assert_eq!(
            coverage_speech_evidence(&lane.state)
                .availability()
                .observed_samples(),
            Some(total)
        );
    }

    fn launch_long(lane: &mut Lane, text: &str) -> (OccurrenceIdentity, Vec<TailPatchRequest>) {
        launch_long_owner(lane, Some(text))
    }

    /// [`launch_long`] for an owner Apple left without text: the occurrence is
    /// qualified and owes its text, and only the capture windows label it.
    fn launch_long_unlabelled(lane: &mut Lane) -> (OccurrenceIdentity, Vec<TailPatchRequest>) {
        launch_long_owner(lane, None)
    }

    fn launch_long_owner(
        lane: &mut Lane,
        text: Option<&str>,
    ) -> (OccurrenceIdentity, Vec<TailPatchRequest>) {
        // Three real requests need thirteen seconds on the capture-owned 9/3 grid.
        // The fixture supplies and measures every sample instead of forcing a policy.
        let total = 13 * u64::from(RATE);
        measured_relay_capture(lane, total);
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, total);
        match text {
            Some(text) => stage(lane, 1, occurrence.clone(), text),
            None => qualify_unlabelled(lane, &occurrence),
        }
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &occurrence));
        if let Some(text) = text {
            close_lexicon(lane, 1, &occurrence, text);
        }
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            requests.len(),
            3,
            "thirteen seconds yield three capture windows"
        );
        let windows = [(0, 144_000), (48_000, 192_000), (96_000, total)];
        for (index, request) in requests.iter().enumerate() {
            let range = &request.provider_request.identity.range;
            assert_eq!((range.sample_start, range.sample_end), windows[index]);
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                windows[index]
            );
            assert_eq!(
                request.audio,
                vec![0.2; (range.sample_end - range.sample_start) as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        (occurrence, requests)
    }

    fn launch_shared_owner_capture(
        lane: &mut Lane,
    ) -> (Vec<OccurrenceIdentity>, Vec<TailPatchRequest>) {
        let session = lane.state.session_id.clone();
        let mut pcm = vec![0.2; 10 * RATE as usize];
        pcm[9 * RATE as usize..].fill(0.0);
        lane.state.audio.push(&pcm);
        record_energy(
            lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let spans = [
            (0, 48_000, "alfa"),
            (48_000, 96_000, "beta"),
            (96_000, 144_000, "gamma"),
        ];
        let mut occurrences = Vec::new();
        for (index, (start, end, text)) in spans.into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(session.clone(), 1, start, end);
            let id = index as u64 + 1;
            stage(lane, id, occurrence.clone(), text);
            assert!(lane.state.register_whisper_owner(&lane.tx, id, &occurrence));
            close_lexicon(lane, id, &occurrence, text);
            occurrences.push(occurrence);
        }
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            requests.len(),
            2,
            "two measured frames share the held owners"
        );
        for (request, (start, end)) in requests.iter().zip([(0, 144_000), (48_000, 160_000)]) {
            let range = &request.provider_request.identity.range;
            assert_eq!((range.sample_start, range.sample_end), (start, end));
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                (start, end)
            );
            assert_eq!(request.audio, pcm[start as usize..end as usize]);
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        assert_eq!(requests[0].member_occurrences.len(), 3);
        assert_eq!(requests[1].member_occurrences.len(), 2);
        (occurrences, requests)
    }

    /// (a) One phrase pin across a long-occurrence slice boundary.
    ///
    /// Contract: step 3 (unanchored, never dropped), founding invariant
    /// (no word pins → read-only evidence, no duplicate token), forbidden
    /// `drop_acoustic_observation_without_receipt`, required receipt
    /// "observations unanchored (kept, no mutation right)".
    #[test]
    fn phrase_grain_segment_crossing_decoder_start_stays_visible_unanchored() {
        let mut lane = open("relay-long-phrase");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let phrase = "od czwartej do siodmej";
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![segment(
                    "relay-long-phrase",
                    phrase,
                    requests[1].provider_request.identity.range.sample_start - 768,
                    requests[1].provider_request.identity.range.sample_start + 48_000,
                )],
            ),
            7.0,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, phrase),
            "a phrase pin crossing the actual decoder start stays visible without mutation authority"
        );
        assert_eq!(
            held_count(&lane),
            1,
            "unanchored overlap must not mint a second token"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
    }

    /// (b) One phrase pin across the coalesced prefix/admit boundary.
    ///
    /// Same contract lines as (a), plus Whisper truth: the overlap carries
    /// context and must not duplicate canvas content.
    #[test]
    fn phrase_grain_segment_straddling_a_coalesced_prefix_stays_visible_unanchored() {
        let mut lane = open("relay-coalesced-phrase");
        let (occurrences, requests) = launch_shared_owner_capture(&mut lane);
        let phrase = "przez granice";
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![segment("relay-coalesced-phrase", phrase, 80_000, 112_000)],
            ),
            10.0,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, phrase),
            "a phrase across the prefix/admit boundary stays visible without mutation authority"
        );
        assert_eq!(
            held_count(&lane),
            3,
            "the phrase must not become a fourth token"
        );
        assert_eq!(held_text(&lane, &occurrences[1]).as_deref(), Some("beta"));
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
    }

    /// (c) Later-window range already covered by an earlier admitted identity.
    ///
    /// Contract: step 4 `replayed_range_identity`. The refusal must not depend
    /// on the two strings matching. Required receipt: structural replays rejected.
    #[test]
    fn overlap_replay_covered_by_an_earlier_identity_is_refused() {
        let mut lane = open("relay-replay");
        let (occurrences, requests) = launch_shared_owner_capture(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![segment("relay-replay", "beta raz", 48_000, 96_000)],
            ),
            3.0,
        );
        let _ = drain(&mut lane.rx);
        assert_eq!(held_text(&lane, &occurrences[1]).as_deref(), Some("beta"));

        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![segment("relay-replay", "powtorka", 64_000, 96_000)],
            ),
            10.0,
        );
        let events = drain(&mut lane.rx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .any(|alternative| alternative.candidate == "powtorka"
                    && alternative.reason == "phrase_without_word_targets")
        );
        assert!(
            ledger
                .layer_trail()
                .iter()
                .filter(|entry| entry.candidate_label == "powtorka")
                .all(|entry| !entry.decision.grants_mutation())
        );
        assert_eq!(ledger.conservation().residue(), 0);
        drop(ledger);
        assert!(
            !whisper_mutations(&events)
                .iter()
                .any(|(label, receipt)| label == "powtorka" && receipt.grants_mutation())
        );
        assert_eq!(
            held_count(&lane),
            3,
            "replay must not mint a duplicate token"
        );
        assert_eq!(held_text(&lane, &occurrences[1]).as_deref(), Some("beta"));
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
    }

    /// One window of word pins does not relabel a longer occurrence.
    ///
    /// Contract step 7: bounded replacement addresses the whole span or nothing.
    /// A single exclusive slice, beside pins that straddle the admit bounds,
    /// is not that span. The Apple label stands and the straddling text stays
    /// unanchored.
    #[test]
    fn phrase_pins_on_one_window_do_not_relabel_the_long_occurrence() {
        let mut lane = open("relay-long-words");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-long-words", "krawedz", 48_000, 52_000),
                    segment("relay-long-words", "srodek", 52_000, 90_000),
                    segment("relay-long-words", "dalej", 90_000, 110_000),
                ],
            ),
            7.0,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, "krawedz"),
            "a contained phrase stays unanchored when the owner extends before the frame"
        );
        assert!(
            unanchored_label(&events, "dalej"),
            "a contained phrase cannot prove the remainder of a longer owner"
        );
        assert!(
            unanchored_label(&events, "srodek"),
            "the exclusive slice stays visible and does not become the span label"
        );
        assert_eq!(
            mutation_count(&events),
            0,
            "step 7: one window must not replace the whole occurrence"
        );
        assert!(
            named_refusal(&events, "intersecting_pin_not_exclusive"),
            "a straddling pin refuses the replacement by name"
        );
        assert_eq!(held_count(&lane), 1);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// Coalesced prefix. A wholly covered word is replay. A pin that straddles
    /// into the later member blocks replacement of that member. The earlier
    /// member, wholly inside its own exclusive admit, may still be relabelled.
    ///
    /// Contract: step 4, step 7.
    #[test]
    fn phrase_pins_do_not_relabel_a_coalesced_member_across_a_straddle() {
        let mut lane = open("relay-coalesced-words");
        let (occurrences, requests) = launch_shared_owner_capture(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![segment("relay-coalesced-words", "beta raz", 48_000, 96_000)],
            ),
            3.0,
        );
        let _ = drain(&mut lane.rx);

        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-coalesced-words", "przez", 80_000, 92_000),
                    segment("relay-coalesced-words", "krawedz", 92_000, 104_000),
                    segment("relay-coalesced-words", "ogon", 104_000, 112_000),
                ],
            ),
            10.0,
        );
        let events = drain(&mut lane.rx);
        assert_eq!(
            mutation_count(&events),
            0,
            "the straddle blocks both members"
        );
        assert!(
            unanchored_label(&events, "przez"),
            "a contained phrase beside the straddle is visible without relabelling beta"
        );
        assert!(
            unanchored_label(&events, "krawedz"),
            "a word pin across the admit boundary is kept whole, without a duplicate token"
        );
        assert!(
            unanchored_label(&events, "ogon"),
            "the exclusive tail stays visible when the member is not wholly proven"
        );
        assert!(
            named_refusal(&events, "intersecting_pin_not_exclusive"),
            "a pin straddling into the member refuses that member's replacement"
        );
        assert_eq!(
            held_text(&lane, &occurrences[2]).as_deref(),
            Some("gamma"),
            "step 7: the later member keeps its label"
        );
        assert_eq!(held_text(&lane, &occurrences[1]).as_deref(), Some("beta"));
        assert_eq!(held_count(&lane), 3);
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (i) Three windows, every intersecting pin an exclusive tail.
    ///
    /// Contract step 7: the held text is the exclusive slices joined in PCM
    /// order, admitted once.
    #[test]
    fn three_windows_admit_words_independently() {
        let mut lane = open("relay-three-exclusive");
        // One provisional lexical position can be corrected by one timed word.
        let pcm = vec![0.2_f32; 208_000];
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 208_000);
        stage(&mut lane, 1, occurrence.clone(), "cale");
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &occurrence));
        close_lexicon(&mut lane, 1, &occurrence, "cale");
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 3);
        let speech = coverage_speech_evidence(&lane.state);
        assert_eq!(
            speech.availability().observed_samples(),
            Some(pcm.len() as u64)
        );
        lane.state
            .acoustic_ledger
            .lock()
            .unwrap()
            .record_speech_evidence(&speech);
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        let session = "relay-three-exclusive";
        let windows = [
            vec![word_pin(session, "raz", 8_000, 40_000)],
            vec![word_pin(session, "dwa", 104_000, 120_000)],
            vec![word_pin(session, "trzy", 160_000, 180_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert_eq!(
            mutation_count(&events),
            3,
            "each window admits its own words"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("raz dwa trzy")
        );
        assert_eq!(held_count(&lane), 1);
        assert_conserved(&lane, None);
    }
    /// Long occurrence, overlapping windows, word pins.
    ///
    /// Contract step 4: a word whose range lies wholly outside this window's
    /// admit and inside an already admitted identity is `replayed_range_identity`.
    /// Contract step 5: the decision is the range, so the replayed word's text
    /// may differ from the word that owns that range in the later window.
    /// Contract step 7: words wholly inside each exclusive remainder join the
    /// occurrence once. The overlap must not mint a second token.
    #[test]
    fn distinct_pad_words_are_admitted_to_their_owner() {
        for corroborated in [false, true] {
            let mut lane = open("relay-word-join");
            // Three independent capture windows share physical word pins; no context mode is selected.
            let pcm = vec![0.2_f32; 208_000];
            lane.state.audio.push(&pcm);
            record_energy(
                &lane,
                &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
            );
            // The windows' routing is measured on an owner Apple left without
            // text. A coarse two-word Apple label over the whole owner keeps its
            // words under D5: no single window's words cover all of its speech.
            let (occurrence, requests) = launch_long_unlabelled(&mut lane);
            let speech = coverage_speech_evidence(&lane.state);
            assert_eq!(
                speech.availability().observed_samples(),
                Some(pcm.len() as u64)
            );
            for request in &requests {
                let frame = &request.provider_request.identity.range;
                assert_eq!(
                    request.audio,
                    pcm[frame.sample_start as usize..frame.sample_end as usize]
                );
                request
                    .provider_request
                    .validate_pcm(&request.audio)
                    .unwrap();
            }
            let session = "relay-word-join";
            let windows = if corroborated {
                // All three real frames cover the whole displaced Word, not just its new pin.
                [
                    vec![
                        word_pin(session, "raz", 8_000, 40_000),
                        word_pin(session, "stary", 100_000, 112_000),
                        word_pin(session, "ogon", 122_000, 136_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 100_000, 112_000),
                        word_pin(session, "dwa", 114_000, 120_000),
                        word_pin(session, "trzy", 122_000, 136_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 100_000, 112_000),
                        word_pin(session, "dwa", 114_000, 120_000),
                        word_pin(session, "trzy", 122_000, 136_000),
                    ],
                ]
            } else {
                [
                    vec![
                        word_pin(session, "raz", 8_000, 40_000),
                        word_pin(session, "stary", 50_000, 62_000),
                        word_pin(session, "dwa", 70_000, 90_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 50_000, 62_000),
                        word_pin(session, "dwa", 70_000, 90_000),
                        word_pin(session, "ogon", 100_000, 110_000),
                    ],
                    vec![word_pin(session, "trzy", 100_000, 150_000)],
                ]
            };
            let mut events = Vec::new();
            for (request, segments) in requests.iter().zip(windows) {
                lane.state
                    .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
                events.extend(drain(&mut lane.rx));
            }
            assert!(
                !replay_refusal(&events, "stary"),
                "step 4: the earlier window's word in the shared second is replay, whatever its text"
            );
            assert!(
                !replay_refusal(&events, "ogon"),
                "step 4: a word past this window's admit is replay, not a second token"
            );
            if corroborated {
                assert_eq!(
                    held_text(&lane, &occurrence).as_deref(),
                    Some("raz nowy dwa trzy")
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(
                    ledger
                        .slots_of(&occurrence)
                        .unwrap()
                        .iter()
                        .all(|s| s.text != "stary" && s.text != "ogon")
                );
                assert!(
                    ledger.word_deletions().is_empty(),
                    "ignored context never became a slot"
                );
            } else {
                assert_eq!(
                    held_text(&lane, &occurrence).as_deref(),
                    Some("raz stary dwa ogon")
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(
                    ledger
                        .slot_alternatives()
                        .iter()
                        .all(|a| a.candidate != "nowy" && a.candidate != "trzy"),
                    "ignored context is not an independent adjudication vote"
                );
            }
            assert_eq!(
                held_count(&lane),
                1,
                "replay must not mint a duplicate token"
            );
            assert_conserved(&lane, None);
        }
    }

    #[test]
    fn capture_grid_preserves_overlap_words_without_separate_context_windows() {
        for corroborated in [false, true] {
            let mut lane = open("relay-word-context");
            let pcm = vec![0.2_f32; 208_000];
            lane.state.audio.push(&pcm);
            record_energy(
                &lane,
                &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
            );
            let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 208_000);
            // The windows' routing is measured on an owner Apple left without
            // text. A coarse two-word Apple label over the whole owner keeps its
            // words under D5: no single window's words cover all of its speech.
            qualify_unlabelled(&mut lane, &occurrence);
            assert!(lane.state.register_whisper_owner(&lane.tx, 1, &occurrence));
            lane.state.capture_stopping = true;
            lane.state.pump_capture_windows(&lane.tx);
            let _ = drain(&mut lane.rx);
            let requests = take_requests(&mut lane.tail_rx);
            assert_eq!(requests.len(), 3);
            let decoder = [(0, 144_000), (48_000, 192_000), (96_000, 208_000)];
            let owned = decoder;
            for (index, request) in requests.iter().enumerate() {
                let frame = &request.provider_request.identity.range;
                assert_eq!((frame.sample_start, frame.sample_end), decoder[index]);
                assert_eq!(
                    (request.admit_sample_start, request.admit_sample_end),
                    owned[index]
                );
                assert_eq!(
                    request.audio.len() as u64,
                    frame.sample_end - frame.sample_start
                );
            }
            let speech = coverage_speech_evidence(&lane.state);
            assert_eq!(
                speech.availability().observed_samples(),
                Some(pcm.len() as u64)
            );
            for request in &requests {
                let frame = &request.provider_request.identity.range;
                assert_eq!(
                    request.audio,
                    pcm[frame.sample_start as usize..frame.sample_end as usize]
                );
                request
                    .provider_request
                    .validate_pcm(&request.audio)
                    .unwrap();
            }
            let session = "relay-word-context";
            let windows = if corroborated {
                // All three real frames cover the whole displaced Word, not just its new pin.
                [
                    vec![
                        word_pin(session, "raz", 8_000, 40_000),
                        word_pin(session, "stary", 100_000, 112_000),
                        word_pin(session, "ogon", 122_000, 136_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 100_000, 112_000),
                        word_pin(session, "dwa", 114_000, 120_000),
                        word_pin(session, "trzy", 122_000, 136_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 100_000, 112_000),
                        word_pin(session, "dwa", 114_000, 120_000),
                        word_pin(session, "trzy", 122_000, 136_000),
                    ],
                ]
            } else {
                [
                    vec![
                        word_pin(session, "raz", 8_000, 40_000),
                        word_pin(session, "stary", 50_000, 62_000),
                        word_pin(session, "dwa", 70_000, 90_000),
                    ],
                    vec![
                        word_pin(session, "nowy", 50_000, 62_000),
                        word_pin(session, "dwa", 70_000, 90_000),
                        word_pin(session, "ogon", 100_000, 110_000),
                    ],
                    vec![word_pin(session, "trzy", 100_000, 150_000)],
                ]
            };
            let mut events = Vec::new();
            for (request, segments) in requests.iter().zip(windows) {
                lane.state
                    .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
                events.extend(drain(&mut lane.rx));
            }
            assert!(
                !replay_refusal(&events, "stary"),
                "step 4: the earlier window's word in the shared second is replay, whatever its text"
            );
            assert!(
                !replay_refusal(&events, "ogon"),
                "step 4: a word past this window's admit is replay, not a second token"
            );
            if corroborated {
                assert_eq!(
                    held_text(&lane, &occurrence).as_deref(),
                    Some("raz nowy dwa trzy")
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(
                    ledger
                        .slots_of(&occurrence)
                        .unwrap()
                        .iter()
                        .all(|s| s.text != "stary" && s.text != "ogon")
                );
                assert!(
                    ledger.word_deletions().is_empty(),
                    "ignored context never became a slot"
                );
            } else {
                assert_eq!(
                    held_text(&lane, &occurrence).as_deref(),
                    Some("raz stary dwa ogon")
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(
                    ledger
                        .slot_alternatives()
                        .iter()
                        .all(|a| a.candidate != "nowy" && a.candidate != "trzy"),
                    "ignored context is not an independent adjudication vote"
                );
            }
            assert_eq!(
                held_count(&lane),
                1,
                "replay must not mint a duplicate token"
            );
            assert_conserved(&lane, None);
        }
    }

    /// A word returned only by the later window but owned by the earlier
    /// window is replay. It does not veto the other exclusive pins.

    #[test]
    fn later_window_admits_novel_words_inside_its_decoder_range() {
        let mut lane = open("relay-word-straddle");
        // Capture precedes planning; default left context contains the returned seam pin.
        let pcm = vec![0.2_f32; 208_000];
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 208_000);
        stage(&mut lane, 1, occurrence.clone(), "krawedz");
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &occurrence));
        close_lexicon(&mut lane, 1, &occurrence, "krawedz");
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 3);
        let decoder = [(0, 144_000), (48_000, 192_000), (96_000, 208_000)];
        let owned = decoder;
        for (index, request) in requests.iter().enumerate() {
            let frame = &request.provider_request.identity.range;
            assert_eq!((frame.sample_start, frame.sample_end), decoder[index]);
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                owned[index]
            );
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        let session = "relay-word-straddle";
        let windows = [
            vec![
                word_pin(session, "raz", 8_000, 40_000),
                word_pin(session, "krawedz", 50_000, 52_000),
                word_pin(session, "dwa", 52_000, 90_000),
            ],
            vec![
                word_pin(session, "krawedz", 50_000, 52_000),
                word_pin(session, "dwa", 52_000, 90_000),
                word_pin(session, "trzy", 100_000, 150_000),
            ],
            vec![word_pin(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            let frame = &request.provider_request.identity.range;
            assert!(
                segments
                    .iter()
                    .all(|segment| segment.range.sample_start >= frame.sample_start
                        && segment.range.sample_end <= frame.sample_end),
                "a returned word must lie inside its actual decode request"
            );
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        let warnings = warning_lines(&events);
        assert!(!replay_refusal(&events, "krawedz"), "{warnings}");
        assert!(
            !named_refusal(&events, "intersecting_pin_not_exclusive"),
            "word grain: the straddle does not refuse the span"
        );
        assert_eq!(mutation_count(&events), 2);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("raz krawedz dwa trzy")
        );
        assert_eq!(held_count(&lane), 1);
        assert_conserved(&lane, None);
    }

    /// (ii) Three windows, one pin straddles an admit boundary.
    ///
    /// Contract step 7 and step 3: the held label is unchanged, the replacement
    /// is a named refusal, and the straddling text stays unanchored.
    #[test]
    fn one_straddling_pin_among_three_windows_refuses_the_span() {
        let mut lane = open("relay-three-straddle");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-three-straddle";
        let windows = [
            vec![segment(session, "raz", 8_000, 40_000)],
            vec![
                segment(session, "krawedz", 48_000, 52_000),
                segment(session, "dwa", 52_000, 90_000),
            ],
            vec![segment(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert_eq!(mutation_count(&events), 0);
        assert!(unanchored_label(&events, "krawedz"));
        assert!(named_refusal(&events, "intersecting_pin_not_exclusive"));
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (iii) Two of three windows return, then stop drains the accumulator.
    ///
    /// Contract step 10: no partial label. One named receipt. The returned
    /// texts stay visible.
    #[test]
    fn stop_with_two_of_three_windows_keeps_the_label() {
        let mut lane = open("relay-stop-partial");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-stop-partial";
        let mut events = Vec::new();
        for (request, segments) in requests.iter().take(2).zip([
            vec![segment(session, "raz", 8_000, 40_000)],
            vec![segment(session, "dwa", 52_000, 90_000)],
        ]) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        events.extend(drain(&mut lane.rx));
        assert_eq!(mutation_count(&events), 0);
        assert!(unanchored_label(&events, "raz"));
        assert!(unanchored_label(&events, "dwa"));
        assert!(named_refusal(&events, "intersecting_pin_not_exclusive"));
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (iv) A coalesced member keeps its label when a pin straddles into it.
    ///
    /// Contract step 7: the exclusive word in that member is not a licence to
    /// replace the member.
    #[test]
    fn coalesced_member_with_a_straddling_pin_keeps_its_label() {
        let mut lane = open("relay-member-straddle");
        let (occurrences, requests) = launch_shared_owner_capture(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-member-straddle", "krawedz", 92_000, 104_000),
                    segment("relay-member-straddle", "ogon", 104_000, 112_000),
                ],
            ),
            10.0,
        );
        let events = drain(&mut lane.rx);
        assert!(unanchored_label(&events, "krawedz"));
        assert!(unanchored_label(&events, "ogon"));
        assert!(named_refusal(&events, "intersecting_pin_not_exclusive"));
        assert_eq!(mutation_count(&events), 0);
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
        assert_eq!(held_count(&lane), 3);
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    fn record_energy(lane: &Lane, blocks: &[Vec<f32>]) {
        let mut writer = CaptureLevelAccumulator::bound_to(&lane.state.capture_energy);
        for block in blocks {
            writer.push_samples(block);
        }
    }

    fn record_silero(lane: &mut Lane, observed_end: u64, speech: Option<(u64, u64)>) {
        use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};

        let mut fusion = SileroIngress::new(RATE, lane.state.session_id.clone(), 1);
        assert!(
            fusion.vad_available(),
            "the fixture needs a measuring Silero"
        );
        fusion.note_observed_pcm(observed_end, observed_end);
        if let Some((start, end)) = speech {
            fusion.observe_boundaries(&[
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechStart,
                    sample: start,
                    speech_probability: 0.9,
                },
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechEnd,
                    sample: end,
                    speech_probability: 0.1,
                },
            ]);
        }
        lane.state.fusion = Some(fusion);
    }

    fn energy_lookups(lane: &Lane) -> u64 {
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .energy_lookups_without_voiced_hop()
    }

    fn route_one_pin(lane: &mut Lane, text: &str) -> (Vec<MemberPinRoute>, Vec<EngineEvent>) {
        let end = 48_000;
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, end);
        let pin = word_pin(&lane.state.session_id, text, 26_000, 46_000);
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 1,
                sample_start: 0,
                sample_end: end,
            },
            &[(1, occurrence)],
            &[pin],
            LedgerObservationProducer::Whisper,
        );
        let events = drain(&mut lane.rx);
        (routes, events)
    }

    #[test]
    fn forensic_authority_routed_word_retains_original_decode_clock_at_both_owner_edges() {
        for (case, current, start, end) in
            [("right", 0, 20_000, 26_000), ("left", 1, 22_000, 30_000)]
        {
            let session = format!("original-routed-{case}");
            let mut lane = open(&session);
            let owners = vec![
                (1, OccurrenceIdentity::new(&session, 1, 0, 24_000)),
                (2, OccurrenceIdentity::new(&session, 1, 24_000, 48_000)),
            ];
            for (id, owner) in &owners {
                stage(&mut lane, *id, owner.clone(), "hipoteza");
            }
            let segments = vec![word_pin(&session, "granica", start, end)];
            let routes = lane.state.route_overlap_pins(
                &lane.tx,
                AdmitWindow {
                    request_id: 9,
                    sample_start: 0,
                    sample_end: 60_000,
                },
                &owners,
                &segments,
                LedgerObservationProducer::Whisper,
            );
            assert_eq!(routes[current].exclusive.len(), 1);
            assert!(routes[1 - current].exclusive.is_empty());
            let assignments = neighbour_pin_assignments(&owners, &routes, &segments, current);
            let (id, owner) = &owners[current];
            assert!(
                lane.state.admit_routed_words_with_decode_window(
                    &lane.tx,
                    *id,
                    owner,
                    (9, Some((0, 60_000))),
                    RoutedWords {
                        pins: &routes[current].exclusive,
                        neighbours: &assignments
                    },
                    LedgerObservationProducer::Whisper,
                ),
                "a complete decoded word reaches its qualified midpoint owner"
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            let words = ledger.slots_of(owner).unwrap();
            assert_eq!(words.len(), 1);
            assert_eq!(words[0].text, "granica");
            let physical_sources = ledger.slot_source_ranges(&words[0]);
            assert!(
                physical_sources
                    .iter()
                    .any(|source| source.sample_start == start && source.sample_end == end),
                "routing discarded original {case} clock {start}..{end}; remaining lineage={physical_sources:?}"
            );
            assert_eq!(words[0].sample_start, start.max(owner.sample_start));
            assert_eq!(words[0].sample_end, end.min(owner.sample_end));
        }
    }

    fn final_fence_lane(
        session: &str,
        speech_end: u64,
    ) -> (Lane, OccurrenceIdentity, TailPatchRequest) {
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let mut audio = vec![ROOM_TONE_RMS; 48_000];
        audio[6_000..speech_end as usize].fill(0.2);
        record_energy(
            &lane,
            &audio.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 48_000, Some((6_000, speech_end)));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .require_text_recovery(&owner)
        );
        lane.state.audio.push(&audio);
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &owner));
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let mut requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 1, "one complete bounded audio request");
        let request = requests.pop().unwrap();
        assert_eq!(
            request.audio, audio,
            "capture energy and decoder consume identical PCM"
        );
        assert_eq!(
            (
                request.provider_request.identity.range.sample_start,
                request.provider_request.identity.range.sample_end
            ),
            (0, 48_000)
        );
        (lane, owner, request)
    }

    #[test]
    fn forensic_terminal_silent_fence_keeps_successful_decode_receipt() {
        let session = "forensic-final-silent-fence";
        let (mut lane, owner, request) = final_fence_lane(session, 16_000);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(&request, vec![word_pin(session, "Iwo", 4_000, 48_000)]),
            3.0,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert!(
            !ledger.text_recovery_pending(&owner),
            "successful PCM plus silent final fence must survive deferred-word admission"
        );
        assert_eq!(
            ledger.frontier_of(&owner).unwrap().open_producers(),
            vec![ObservationProducer::Lexicon],
            "the fixture's scheduled L2 has not returned yet"
        );
        assert!(!ledger.is_sealed(&owner), "an open L2 must prevent sealing");
        drop(ledger);
        close_lexicon(&mut lane, 1, &owner, "Iwo");
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_sealed(&owner));
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert_eq!(
            ledger.slots_of(&owner).unwrap()[0].producer,
            ObservationProducer::Whisper
        );
    }

    #[test]
    fn forensic_terminal_voiced_fence_keeps_recovery_open() {
        let session = "forensic-final-voiced-fence";
        let (mut lane, owner, request) = final_fence_lane(session, 48_000);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(&request, vec![word_pin(session, "Iwo", 4_000, 48_000)]),
            3.0,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.text_recovery_pending(&owner));
        assert!(
            !ledger.is_sealed(&owner),
            "speech reaching the actual fence cannot certify complete audio"
        );
    }

    fn assert_separate_spoken_repetitions_order(reverse: bool) {
        let session = if reverse {
            "forensic-two-orders-reverse"
        } else {
            "forensic-two-orders-forward"
        };
        // Obtain the window clock from the existing scheduler before
        // constructing this lane's one shared capture/provider PCM.
        let (_, _, clock_requests) = forensic_lane("forensic-repeat-clock", &[]);
        let end = clock_requests[0].provider_request.identity.range.sample_end;
        let first = (end - 3_500, end - 500);
        let second = (end + 2_000, end + 30_000);
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
        qualify_unlabelled(&mut lane, &owner);
        let mut input = piece(1, &owner, "");
        input.audio.fill(0.0);
        input.audio[first.0 as usize..first.1 as usize].fill(0.2);
        input.audio[second.0 as usize..second.1 as usize].fill(0.2);
        record_energy(
            &lane,
            &input
                .audio
                .chunks(1_000)
                .map(<[f32]>::to_vec)
                .collect::<Vec<_>>(),
        );
        let pcm = input.audio.clone();
        lane.state.audio.push(&pcm);
        assert_eq!(lane.state.audio.session_sample_end(), pcm.len() as u64);
        lane.state.pump_capture_windows(&lane.tx);
        let mut requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), clock_requests.len());
        assert!(
            requests.len() >= 2,
            "the fixture needs both physical windows"
        );
        assert_eq!(requests[0].provider_request.identity.range.sample_end, end);
        for request in &requests {
            let range = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize],
                "decode bounds must describe the same measured capture samples"
            );
        }
        use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
        let mut fusion = SileroIngress::new(RATE, session.to_string(), 1);
        assert!(fusion.vad_available());
        fusion.note_observed_pcm(200_000, 200_000);
        fusion.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: first.0,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: first.1,
                speech_probability: 0.1,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: second.0,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: second.1,
                speech_probability: 0.1,
            },
        ]);
        lane.state.fusion = Some(fusion);
        // The second spoken pin belongs to the final 6..12.5s EOF frame.
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        requests.extend(take_requests(&mut lane.tail_rx));
        assert_eq!(requests.len(), 3);
        let canonical = [
            word_pin(session, "Iwo", end - 4_000, end),
            word_pin(session, "Iwo", end + 1_000, end + 36_000),
        ];
        let order = if reverse { [2, 1, 0] } else { [0, 1, 2] };
        for index in order {
            let frame = &requests[index].provider_request.identity.range;
            let words = canonical
                .iter()
                .filter(|word| frame.contains(&word.range))
                .cloned()
                .collect();
            lane.state
                .complete_whisper_window(&lane.tx, completion(&requests[index], words), 12.5);
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger.text_of(&owner),
            Some("Iwo Iwo"),
            "independent measured spoken ranges must survive order reverse={reverse}"
        );
        let words = ledger.slots_of(&owner).unwrap();
        assert_eq!(words.len(), 2);
        assert!(words[0].sample_end < words[1].sample_start);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_separate_spoken_repetitions_forward() {
        assert_separate_spoken_repetitions_order(false);
    }

    #[test]
    fn forensic_separate_spoken_repetitions_reverse() {
        assert_separate_spoken_repetitions_order(true);
    }

    #[test]
    fn forensic_completed_window_settles_measured_coarse_recovery() {
        let session = "completed-debt-clock";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 24_000);
        record_voiced_spans(&lane, 48_000, &[(6_000, 16_000)]);
        record_silero(&mut lane, 48_000, Some((6_000, 16_000)));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.is_qualified(&owner));
            assert!(ledger.require_text_recovery(&owner));
            ledger.schedule_frontier(
                owner.clone(),
                [ObservationProducer::Apple, ObservationProducer::Whisper],
            );
            assert!(!ledger.note_frontier_return(&owner, ObservationProducer::Apple));
            assert_eq!(
                ledger.frontier_of(&owner).unwrap().open_producers(),
                vec![ObservationProducer::Whisper]
            );
        }
        let segments = [word_pin(session, "Iwo", 4_000, 18_000)];
        let members = [(1, owner.clone())];
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 12,
                sample_start: 0,
                sample_end: 48_000,
            },
            &members,
            &segments,
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert!(lane.state.admit_routed_words_with_decode_window(
            &lane.tx,
            1,
            &owner,
            (12, Some((0, 48_000))),
            RoutedWords {
                pins: &routes[0].exclusive,
                neighbours: &[]
            },
            LedgerObservationProducer::Whisper,
        ));
        let speech = coverage_speech_evidence(&lane.state);
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert!(
            !ledger.text_recovery_pending(&owner),
            "the returned complete word window covers the measured speech"
        );
        let coverage = ledger.assess_seal_coverage(session, 1, &speech, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert!(ledger.record_seal_coverage(coverage));
        assert!(ledger.seal_terminal(session, 1).is_ok());
    }

    #[test]
    fn forensic_leading_silence_proves_capture_origin_word_recovery() {
        let session = "forensic_leading_silence_proves_capture_origin_word_recovery";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        record_voiced_capture(&mut lane, 48_000, &[(6_000, 16_000)]);
        record_silero(&mut lane, 48_000, Some((6_000, 16_000)));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.is_qualified(&owner));
            assert!(ledger.require_text_recovery(&owner));
            ledger.schedule_frontier(
                owner.clone(),
                [ObservationProducer::Apple, ObservationProducer::Whisper],
            );
            assert!(!ledger.note_frontier_return(&owner, ObservationProducer::Apple));
            assert_eq!(
                ledger.frontier_of(&owner).unwrap().open_producers(),
                vec![ObservationProducer::Whisper]
            );
        }
        let segments = [word_pin(session, "Iwo", 0, 18_000)];
        let members = [(1, owner.clone())];
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 12,
                sample_start: 0,
                sample_end: 48_000,
            },
            &members,
            &segments,
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert!(lane.state.admit_routed_words_with_decode_window(
            &lane.tx,
            1,
            &owner,
            (12, Some((0, 48_000))),
            RoutedWords {
                pins: &routes[0].exclusive,
                neighbours: &[]
            },
            LedgerObservationProducer::Whisper,
        ));
        let speech = coverage_speech_evidence(&lane.state);
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert!(
            !ledger.text_recovery_pending(&owner),
            "the returned complete word window covers the measured speech"
        );
        let coverage = ledger.assess_seal_coverage(session, 1, &speech, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert!(ledger.record_seal_coverage(coverage));
        assert!(ledger.seal_terminal(session, 1).is_ok());
    }

    #[test]
    fn forensic_trailing_silence_proves_decode_end_word_recovery() {
        let session = "forensic_trailing_silence_proves_decode_end_word_recovery";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        record_voiced_capture(&mut lane, 48_000, &[(6_000, 16_000)]);
        record_silero(&mut lane, 48_000, Some((6_000, 16_000)));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.is_qualified(&owner));
            assert!(ledger.require_text_recovery(&owner));
            ledger.schedule_frontier(
                owner.clone(),
                [ObservationProducer::Apple, ObservationProducer::Whisper],
            );
            assert!(!ledger.note_frontier_return(&owner, ObservationProducer::Apple));
            assert_eq!(
                ledger.frontier_of(&owner).unwrap().open_producers(),
                vec![ObservationProducer::Whisper]
            );
        }
        let segments = [word_pin(session, "Iwo", 4_000, 48_000)];
        let members = [(1, owner.clone())];
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 12,
                sample_start: 0,
                sample_end: 48_000,
            },
            &members,
            &segments,
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert!(lane.state.admit_routed_words_with_decode_window(
            &lane.tx,
            1,
            &owner,
            (12, Some((0, 48_000))),
            RoutedWords {
                pins: &routes[0].exclusive,
                neighbours: &[]
            },
            LedgerObservationProducer::Whisper,
        ));
        let speech = coverage_speech_evidence(&lane.state);
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert!(
            !ledger.text_recovery_pending(&owner),
            "the returned complete word window covers the measured speech"
        );
        let coverage = ledger.assess_seal_coverage(session, 1, &speech, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert!(ledger.record_seal_coverage(coverage));
        assert!(ledger.seal_terminal(session, 1).is_ok());
    }

    #[test]
    fn forensic_decode_fence_with_speech_remains_unsettled() {
        assert_voiced_decode_fence_unsettled(
            "voiced-leading-fence",
            (0, 16_000),
            (0, 18_000),
            true,
        );
    }

    #[test]
    fn forensic_trailing_decode_fence_with_speech_remains_unsettled() {
        assert_voiced_decode_fence_unsettled(
            "voiced-trailing-fence",
            (6_000, 48_000),
            (4_000, 48_000),
            false,
        );
    }

    fn assert_voiced_decode_fence_unsettled(
        session: &str,
        speech_range: (u64, u64),
        word_range: (u64, u64),
        expected_admitted: bool,
    ) {
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        record_voiced_capture(&mut lane, 48_000, &[speech_range]);
        record_silero(&mut lane, 48_000, Some(speech_range));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.require_text_recovery(&owner));
        ledger.record_speech_evidence(&coverage_speech_evidence(&lane.state));
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 19, 0, owner.clone());
        lane.state
            .record_decode_fence_measurement(&mut ledger, Some((0, 48_000)));
        let before = ledger.slots_of(&owner).unwrap_or(&[]).to_vec();
        let receipt = ledger.admit_word_slots(
            &observation,
            &[
                crate::pipeline::acoustic_ledger::WordPin::new(word_range.0, word_range.1, "Iwo")
                    .with_decode_window(0, 48_000),
            ],
        );
        assert_eq!(
            receipt.grants_mutation(),
            expected_admitted,
            "{session}: {receipt:?}"
        );
        if !expected_admitted {
            assert!(matches!(
                receipt,
                MutationReceipt::Refuse {
                    reason: RefuseReason::SlotAdmissionRejected,
                    ..
                }
            ));
            assert_eq!(ledger.slots_of(&owner).unwrap_or(&[]), before.as_slice());
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .any(|alternative| alternative.reason == "live_edge_cut_pending")
            );
        }
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        assert!(
            ledger.text_recovery_pending(&owner),
            "{session}: speech at decode fence cannot prove a whole word"
        );
        assert_eq!(
            ledger.seal_terminal(session, 1),
            Err(crate::pipeline::acoustic_ledger::SealRefusal::TextRecoveryPending)
        );
    }

    #[test]
    fn forensic_rejected_decode_window_cannot_discharge_recovery() {
        use crate::pipeline::acoustic_ledger::{SealRefusal, WordPin};
        for distinct_context in [false, true] {
            let session = "refused-window-debt";
            let mut lane = open(session);
            let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
            record_voiced_spans(&lane, 48_000, &[(6_000, 16_000)]);
            record_silero(&mut lane, 48_000, Some((6_000, 16_000)));
            stage(&mut lane, 1, owner.clone(), "hipoteza");
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            ledger.record_speech_evidence(&coverage_speech_evidence(&lane.state));
            assert!(ledger.require_text_recovery(&owner));
            ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
            let initial =
                ObservationIdentity::new(ObservationProducer::Whisper, 91, 1, owner.clone());
            let receipt = ledger.admit_word_slots(
                &initial,
                &[WordPin::new(4_000, 18_000, "Iwo").with_decode_window(0, 24_000)],
            );
            assert!(receipt.grants_mutation(), "initial: {receipt:?}");
            assert!(ledger.text_recovery_pending(&owner));
            let stale =
                ObservationIdentity::new(ObservationProducer::Whisper, 92, 0, owner.clone());
            let before = ledger.slots_of(&owner).unwrap().to_vec();
            // Option A pays a debt with any complete window that heard the
            // range, stale or not (agreed with Lena, 2026-10-21): the debt is
            // about hearing the PCM, not about admitting the text. This stale
            // window hears only half the owner, so the debt stays open.
            let rejected = ledger.admit_word_slots(
                &stale,
                &[WordPin::new(4_000, 18_000, "inny").with_decode_window(0, 24_000)],
            );
            assert!(!rejected.grants_mutation(), "stale: {rejected:?}");
            assert_eq!(ledger.slots_of(&owner).unwrap(), before);
            assert!(ledger.text_recovery_pending(&owner));
            let hypothesis =
                ObservationIdentity::new(ObservationProducer::Whisper, 93, 2, owner.clone());
            let pins = [WordPin::new(4_000, 18_000, "Kamil").with_decode_window(0, 24_000)];
            ledger.admit_word_slots(&hypothesis, &pins);
            let trial = ledger
                .next_word_trial(true)
                .expect("lexical correction requires a trial");
            let returned = ledger.next_word_observation(ObservationProducer::Whisper, 94, &owner);
            let context_end = if distinct_context { 32_000 } else { 24_000 };
            let candidate =
                [WordPin::new(4_000, 18_000, "Kamil").with_decode_window(0, context_end)];
            let decision = ledger.admit_word_trial(&trial, &returned, &candidate, &candidate);
            let reason = &ledger.word_choices().last().unwrap().reason;
            assert_eq!(
                decision.grants_mutation(),
                distinct_context,
                "context={distinct_context}, reason={reason}: {decision:?}"
            );
            assert_eq!(
                reason,
                if distinct_context {
                    "trial_confirmed"
                } else {
                    "trial_unresolved"
                }
            );
            assert_eq!(
                ledger.text_of(&owner),
                Some(if distinct_context { "Kamil" } else { "Iwo" })
            );
            assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
            assert!(
                ledger.text_recovery_pending(&owner),
                "neither returned decode covered the remaining PCM"
            );
            assert_eq!(
                ledger.seal_terminal(session, 1),
                Err(SealRefusal::TextRecoveryPending)
            );
        }
    }

    #[test]
    fn forensic_authority_formatter_observation_cannot_replace_a_qualified_raw_source() {
        let mut lane = open("raw-formatter-authority");
        let owner = OccurrenceIdentity::new("raw-formatter-authority", 1, 0, 48_000);
        stage(&mut lane, 1, owner.clone(), "źródłowe słowo");
        let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_qualified(&owner));
        let original = ledger.slots_of(&owner).unwrap().to_vec();
        let offered = ObservationIdentity::new(ObservationProducer::Formatter, 2, 1, owner.clone());
        let receipt = ledger.admit_pinned_label(&offered, "modelowe zdanie", &[]);
        assert!(
            !receipt.grants_mutation(),
            "formatter is a derived presentation observer"
        );
        assert_eq!(
            ledger.slots_of(&owner).unwrap(),
            original,
            "an authenticated acoustic source must survive a formatter observation"
        );
        assert_eq!(ledger.text_of(&owner), Some("źródłowe słowo"));
    }

    #[test]
    fn energy_silence_with_silero_speech_routes_the_whisper_pin() {
        let mut lane = open("relay-silero-speech");
        record_energy(&lane, &[vec![0.0; 48_000]]);
        record_silero(&mut lane, 48_000, Some((25_000, 47_000)));

        let (routes, events) = route_one_pin(&mut lane, "słowo");
        assert_eq!(routes[0].exclusive.len(), 1);
        assert_eq!(routes[0].exclusive[0].text, "słowo");
        assert!(events.is_empty(), "the pin must reach its exclusive route");
        assert_eq!(energy_lookups(&lane), 0);
    }

    #[test]
    fn unavailable_or_short_silero_extent_cannot_join_an_energy_silence_veto() {
        let mut unavailable = open("relay-silero-unavailable");
        record_energy(&unavailable, &[vec![0.0; 48_000]]);
        record_silero(&mut unavailable, 0, None);
        let (routes, events) = route_one_pin(&mut unavailable, "słowo");
        assert_eq!(routes[0].exclusive.len(), 1);
        assert!(
            events.is_empty(),
            "unavailable Silero cannot refuse the pin"
        );

        let mut short = open("relay-silero-short");
        record_energy(&short, &[vec![0.0; 48_000]]);
        record_silero(&mut short, 40_000, None);
        let (routes, events) = route_one_pin(&mut short, "słowo");
        assert_eq!(routes[0].exclusive.len(), 1);
        assert!(
            events.is_empty(),
            "a short Silero extent cannot refuse the pin"
        );
    }

    #[test]
    fn silero_distance_is_the_gap_to_the_nearest_speech_once_its_radius_was_heard() {
        let ms = |samples: u64| SileroSpeechDistance::Ms(samples * 1_000 / u64::from(RATE));

        let mut lane = open("silero-distance");
        assert_eq!(
            silero_speech_distance(&lane.state, 44_000, 45_000),
            SileroSpeechDistance::Unmeasured,
            "no Silero, no distance"
        );
        record_silero(&mut lane, 160_000, Some((40_000, 48_000)));
        let speech = observed_silero(&lane.state).expect("observed").ranges()[0].clone();
        let state = &lane.state;
        assert_eq!(silero_speech_distance(state, 44_000, 45_000), ms(0));
        assert_eq!(
            silero_speech_distance(state, speech.sample_end + 4_800, speech.sample_end + 6_000),
            ms(4_800),
            "inside the radius the distance needs no further audio"
        );
        assert_eq!(
            silero_speech_distance(state, 20_000, 24_000),
            ms(speech.sample_start - 24_000),
            "speech after the word counts too"
        );
        assert_eq!(
            silero_speech_distance(state, 100_000, 104_000),
            ms(100_000 - speech.sample_end)
        );
        assert_eq!(
            silero_speech_distance(state, 150_000, 154_000),
            SileroSpeechDistance::Unmeasured,
            "the radius after the word reaches past what Silero heard"
        );
        assert_eq!(
            silero_speech_distance(state, 170_000, 172_000),
            SileroSpeechDistance::Unmeasured,
            "the word itself lies past what Silero heard"
        );

        lane.state.capture_stopping = true;
        assert_eq!(
            silero_speech_distance(&lane.state, 150_000, 154_000),
            ms(150_000 - speech.sample_end),
            "a stopped capture has no audio after its end, so the radius is closed"
        );

        let mut quiet = open("silero-distance-quiet");
        record_silero(&mut quiet, 160_000, None);
        assert_eq!(
            silero_speech_distance(&quiet.state, 100_000, 104_000),
            SileroSpeechDistance::NoSpeechHeard
        );
    }

    fn direct_payload(
        session: &str,
        segments: Vec<TimedTailSegment>,
        grain: TailSegmentGrain,
    ) -> TailProviderPayload {
        let text = segments
            .iter()
            .map(|segment| segment.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        TailProviderPayload {
            identity: TailRequestIdentity {
                request_id: 1,
                range: crate::stt::tail_provider::TailSampleRange {
                    session: session.to_string(),
                    capture_epoch: 1,
                    sample_start: 0,
                    sample_end: 160_000,
                },
            },
            text,
            segments,
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.1),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain: grain,
                source: TailEvidenceSource::Whisper,
                revision: Some("direct-over-silence".into()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        }
    }

    #[test]
    fn direct_word_text_drops_only_words_silero_heard_no_speech_near() {
        let session = "direct-over-silence";
        let mut lane = open(session);
        record_energy(
            &lane,
            &[vec![0.0; 40_000], vec![0.2; 8_000], vec![0.0; 112_000]],
        );
        record_silero(&mut lane, 160_000, Some((40_000, 48_000)));
        let speech_end = observed_silero(&lane.state).expect("observed").ranges()[0].sample_end;
        let words = || {
            vec![
                word_pin(session, "mowa", 41_000, 47_000),
                word_pin(session, "dziękuję", 120_000, 126_000),
            ]
        };

        let payload = direct_payload(session, words(), TailSegmentGrain::Word);
        let window = measure_whisper_window_speech(&lane.state, &payload.segments);
        // Silero observed the span and no speech island touches it, so the
        // gate refuses it as span_without_own_speech (96b602fa, c28d375a);
        // the word's measurement still carries what lay under it and how far
        // the nearest speech was.
        assert_eq!(
            window.decisions[1].verdict,
            WhisperWordSpeechVerdict::Refuse(RefuseReason::SpanWithoutOwnSpeech)
        );
        assert_eq!(
            (
                window.facts[1].silence_under_span,
                window.facts[1].silero_speech_distance
            ),
            (
                SilenceUnderSpan::DigitalZero,
                SileroSpeechDistance::Ms((120_000 - speech_end) * 1_000 / u64::from(RATE)),
            ),
            "the word over digital zero, far from speech, carries its class and distance"
        );
        assert!(!matches!(
            window.decisions[0].verdict,
            WhisperWordSpeechVerdict::Refuse(_)
        ));
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &payload),
            "mowa"
        );
        assert_eq!(lane.state.direct_words_over_silence, 1);

        let phrase = direct_payload(session, words(), TailSegmentGrain::Phrase);
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &phrase),
            "mowa dziękuję",
            "a phrase-grain payload has no word spans to judge"
        );
        assert_eq!(lane.state.direct_words_over_silence, 1);
    }

    /// A decoder loop writes copies with no lexical duration, so pinning
    /// leaves them out; the direct lane drops each unpinned copy of the word
    /// before it. Spoken repeats carry their own pins and all stay.
    #[test]
    fn direct_word_text_drops_unpinned_copies_and_keeps_pinned_repeats() {
        let session = "direct-unpinned-loop";
        let mut lane = open(session);
        // Three spoken words inside one Silero range, each its own voiced
        // burst: the 20 ms hops between them fall below the floor.
        let mut pcm = vec![0.0_f32; 160_000];
        for (start, end) in [(40_000, 42_000), (42_600, 44_400), (45_000, 48_000)] {
            pcm[start..end].fill(0.2);
        }
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 160_000, Some((40_000, 48_000)));

        let mut looped = direct_payload(
            session,
            vec![
                word_pin(session, "gdzieś", 41_000, 43_000),
                word_pin(session, "to,", 43_500, 44_500),
            ],
            TailSegmentGrain::Word,
        );
        looped.text = " gdzieś to, to, to, to, w".into();
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &looped),
            "gdzieś to, w",
            "three unpinned copies go; the unpinned word after them is no copy"
        );
        assert_eq!(lane.state.direct_unpinned_repeats, 3);
        assert_eq!(lane.state.direct_words_over_silence, 0);

        // Each counted repeat lies on its own burst and lasts longer than one
        // repeat syllable (WHISPER_REPEAT_MIN_MS_PER_SYLLABLE).
        let counted = direct_payload(
            session,
            vec![
                word_pin(session, "trzy,", 40_100, 41_900),
                word_pin(session, "trzy,", 42_700, 44_400),
                word_pin(session, "trzy.", 45_100, 46_900),
            ],
            TailSegmentGrain::Word,
        );
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &counted),
            counted.text,
            "each spoken repeat has its own pin"
        );
        assert_eq!(lane.state.direct_unpinned_repeats, 3);
    }

    /// Dropping a word over silence no longer drops the unpinned words of the
    /// same window; pins that do not line up with the text keep the old rule.
    #[test]
    fn direct_word_text_keeps_unpinned_words_beside_a_word_over_silence() {
        let session = "direct-unpinned-kept";
        let mut lane = open(session);
        record_energy(
            &lane,
            &[vec![0.0; 40_000], vec![0.2; 8_000], vec![0.0; 112_000]],
        );
        record_silero(&mut lane, 160_000, Some((40_000, 48_000)));
        let words = || {
            vec![
                word_pin(session, "mowa", 41_000, 47_000),
                word_pin(session, "dziękuję", 120_000, 126_000),
            ]
        };

        let mut payload = direct_payload(session, words(), TailSegmentGrain::Word);
        payload.text = "mowa jakby dziękuję".into();
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &payload),
            "mowa jakby"
        );
        assert_eq!(lane.state.direct_words_over_silence, 1);

        assert_eq!(pin_text_tokens("dziękuję mowa", &words()), None);
        payload.text = "dziękuję mowa".into();
        assert_eq!(
            direct_text_heard_by_silero(&mut lane.state, &payload),
            "mowa",
            "out-of-order pins fall back to the kept pins alone"
        );
        assert_eq!(lane.state.direct_words_over_silence, 2);
        assert_eq!(lane.state.direct_unpinned_repeats, 0);
    }

    /// A valid direct final withheld by the take's own Silero evidence is
    /// complete work: Stop's direct completion requires zero skipped jobs,
    /// so counting it as skipped turned a quiet window into CaptureStopFailure.
    #[test]
    fn suppressed_direct_final_is_complete_work_not_a_skipped_job() {
        let mut lane = open("direct-suppressed-accounting");
        lane.state.account_direct_job(DirectJobOutcome::Suppressed);
        lane.state.account_direct_job(DirectJobOutcome::Published);
        assert_eq!(
            (
                lane.state.tail_patch_jobs_applied,
                lane.state.tail_patch_jobs_skipped,
                lane.state.direct_jobs_suppressed,
            ),
            (1, 0, 1)
        );
        lane.state.account_direct_job(DirectJobOutcome::Failed);
        assert_eq!(
            lane.state.tail_patch_jobs_skipped, 1,
            "a failed job still withholds the direct completion"
        );
    }

    #[test]
    fn speech_seen_by_both_witnesses_keeps_the_exclusive_pin_route() {
        let mut lane = open("relay-both-speech");
        record_energy(&lane, &[vec![0.2; 48_000]]);
        record_silero(&mut lane, 48_000, Some((25_000, 47_000)));

        let (routes, events) = route_one_pin(&mut lane, "słowo");
        assert_eq!(routes[0].exclusive.len(), 1);
        assert_eq!(routes[0].exclusive[0].text, "słowo");
        assert!(
            events.is_empty(),
            "speech follows the original exclusive route"
        );
        assert_eq!(energy_lookups(&lane), 0);
    }

    /// Qualified occurrence, last 1.5 s measured silence. A Whisper pin that
    /// lies wholly in that silence is offered as an exclusive tail.
    ///
    /// Silero's extent covers the pin's end and no speech island touches it,
    /// so Silero refuses this plausible Whisper silence hallucination with a
    /// named receipt; energy decides only where Silero has not observed the
    /// span (c28d375a). Nothing is relabelled, appended, or painted.
    #[test]
    fn whisper_pin_wholly_inside_measured_silence_cannot_relabel_or_append() {
        let mut lane = open("relay-silent-pin");
        let session = "relay-silent-pin";
        let silence_at = 24_000_u64;
        let end = 48_000_u64;
        record_energy(
            &lane,
            &[
                vec![0.2; silence_at as usize],
                vec![0.0; (end - silence_at) as usize],
            ],
        );
        record_silero(&mut lane, end, Some((0, 20_000)));
        let occurrence = OccurrenceIdentity::new(session, 1, 0, end);
        stage(&mut lane, 1, occurrence.clone(), "mowa");
        let pcm = piece(1, &occurrence, "mowa");
        lane.state.audio.push(&pcm.audio);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "mowa");
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            requests.len(),
            1,
            "a stopped 3 s head is one capture-grid partial"
        );
        let pin_start = 26_000;
        let pin_end = 46_000;
        assert!(pin_start >= silence_at && pin_end <= end);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "dziękuję", pin_start, pin_end)],
            ),
            3.0,
        );
        let events = drain(&mut lane.rx);
        assert_eq!(
            mutation_count(&events),
            0,
            "a pin wholly inside measured silence must not relabel or append"
        );
        assert!(
            !unanchored_label(&events, "dziękuję"),
            "no voiced hop is a refusal, not paint"
        );
        assert!(
            named_refusal(&events, "span_without_own_speech"),
            "the refusal is named span_without_own_speech"
        );
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("mowa"));
        assert_eq!(held_count(&lane), 1);
        assert_eq!(
            energy_lookups(&lane),
            0,
            "Silero observed the span; no energy lookup decided the refusal"
        );
        assert_conserved(&lane, Some("span_without_own_speech"));
    }

    /// The same geometry, with the pin overlapping a voiced hop.
    ///
    /// Contract: a pin that contains voiced audio stays admissible. Hop
    /// evidence is the gate; mean loudness of the occurrence is not.
    #[test]
    fn whisper_pin_partly_over_voiced_hops_stays_admissible() {
        let mut lane = open("relay-voiced-pin");
        let session = "relay-voiced-pin";
        let silence_at = 24_000_u64;
        let end = 48_000_u64;
        record_energy(
            &lane,
            &[
                vec![0.2; silence_at as usize],
                vec![0.0; (end - silence_at) as usize],
            ],
        );
        let occurrence = OccurrenceIdentity::new(session, 1, 0, end);
        stage(&mut lane, 1, occurrence.clone(), "mowa");
        let pcm = piece(1, &occurrence, "mowa");
        lane.state.audio.push(&pcm.audio);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "mowa");
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 1);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "koniec", 20_000, 30_000)],
            ),
            3.0,
        );
        let events = drain(&mut lane.rx);
        assert_eq!(
            mutation_count(&events),
            1,
            "a pin that overlaps a voiced hop stays admissible"
        );
        assert!(
            !named_refusal(&events, "no_voiced_hop_in_pin"),
            "partial voiced overlap is not a silence refusal"
        );
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("koniec"));
        assert_eq!(held_count(&lane), 1);
        assert_eq!(energy_lookups(&lane), 0);
        assert_conserved(&lane, None);
    }

    /// VAD override can hand Layer 1 one fragment longer than 32 s.
    ///
    /// Contract step 1: a window is `[cursor, cursor + 4 s)` clipped to
    /// admitted speech. The fragment is split into those windows before any
    /// length refusal. The default 12 s force-seal never reaches this path.
    #[test]
    fn fragment_longer_than_32s_reaches_l1_as_step1_windows() {
        let mut lane = open("relay-over-32");
        let samples = 33 * u64::from(RATE);
        assert!(samples > u64::from(RATE) * 32);
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, samples);
        stage(&mut lane, 1, occurrence.clone(), "dlugo");
        let pcm = piece(1, &occurrence, "dlugo");
        lane.state.audio.push(&pcm.audio);
        lane.state.pump_capture_windows(&lane.tx);
        assert_eq!(lane.state.audio.session_sample_end(), samples);
        let sent = take_requests(&mut lane.tail_rx);
        let pending = lane.state.refinement_pending.len();
        let windows = sent.len() + pending;
        assert!(
            windows >= 2,
            "expected step-1 windows, sent {} pending {}",
            sent.len(),
            pending
        );
        assert!(
            lane.state.windows_admitted >= 2,
            "windows_admitted={}, refusals={:?}",
            lane.state.windows_admitted,
            lane.state.windows_refused_before_inference
        );
        let backlog = lane
            .state
            .windows_refused_before_inference
            .get("live_refinement_backlog_exhausted")
            .copied()
            .unwrap_or(0);
        assert!(
            !(lane.state.windows_admitted == 0 && backlog == 1),
            "the whole fragment must not be one backlog refusal"
        );
        for request in sent.iter().chain(lane.state.refinement_pending.iter()) {
            let span = request.provider_request.identity.range.sample_end
                - request.provider_request.identity.range.sample_start;
            assert_eq!(
                span,
                9 * u64::from(RATE),
                "a full capture window is 9 s, got {span} samples"
            );
            assert_eq!(request.audio.len() as u64, span);
        }
    }

    /// A pause between two admitted word pins that is silence on the energy
    /// clock is not uncovered speech. The member is still replaced once.
    #[test]
    fn silent_pause_between_word_pins_still_joins_the_member() {
        let mut lane = open("relay-silent-gap");
        let session = "relay-silent-gap";
        record_energy(
            &lane,
            &[
                vec![ROOM_TONE_RMS; 8_000],
                vec![0.2; 12_000],
                vec![ROOM_TONE_RMS; 50_000],
                vec![0.2; 18_000],
                vec![ROOM_TONE_RMS; 8_000],
            ],
        );
        let mut actual = vec![ROOM_TONE_RMS; 192_000];
        actual[8_000..20_000].fill(0.2);
        actual[70_000..88_000].fill(0.2);
        lane.state.audio.push(&actual);
        let occurrence = OccurrenceIdentity::new(session, 1, 0, 96_000);
        stage(&mut lane, 1, occurrence.clone(), "cale");
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "cale");
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);

        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "raz", 8_000, 20_000),
                    word_pin(session, "dwa", 70_000, 88_000),
                ],
            ),
            4.0,
        );
        let mut events = drain(&mut lane.rx);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(&requests[1], vec![word_pin(session, "dwa", 70_000, 88_000)]),
            6.0,
        );
        events.extend(drain(&mut lane.rx));
        assert_eq!(
            mutation_count(&events),
            1,
            "a measured-silent pause between word pins is not uncovered speech"
        );
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("raz dwa"));
        assert_eq!(held_count(&lane), 1);
        assert_eq!(energy_lookups(&lane), 0);
        assert_replaced_slot_evidence(&lane, &occurrence, &["cale"]);
        assert_conserved(&lane, None);
    }

    /// A voiced gap does not keep a superseded aggregate label in the document.
    #[test]
    fn voiced_gap_does_not_block_admission_of_heard_words() {
        let mut lane = open("relay-voiced-gap");
        let session = "relay-voiced-gap";
        record_energy(
            &lane,
            &[
                vec![ROOM_TONE_RMS; 8_000],
                vec![0.2; 12_000],
                vec![ROOM_TONE_RMS; 20_000],
                vec![0.2; 4_000],
                vec![ROOM_TONE_RMS; 26_000],
                vec![0.2; 18_000],
                vec![ROOM_TONE_RMS; 8_000],
            ],
        );
        let mut actual = vec![ROOM_TONE_RMS; 192_000];
        actual[8_000..20_000].fill(0.2);
        actual[70_000..88_000].fill(0.2);
        actual[40_000..44_000].fill(0.2);
        lane.state.audio.push(&actual);
        let occurrence = OccurrenceIdentity::new(session, 1, 0, 96_000);
        stage(&mut lane, 1, occurrence.clone(), "cale");
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "cale");
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);

        let mut events = Vec::new();
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "raz", 8_000, 20_000),
                    word_pin(session, "dwa", 70_000, 88_000),
                ],
            ),
            4.0,
        );
        events.extend(drain(&mut lane.rx));
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(&requests[1], vec![word_pin(session, "dwa", 70_000, 88_000)]),
            6.0,
        );
        events.extend(drain(&mut lane.rx));
        assert_eq!(
            mutation_count(&events),
            1,
            "both words arrive in one owning window; a voiced hop remains debt"
        );
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("raz dwa"));
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_recovery_pending(&occurrence)
        );
        assert_eq!(held_count(&lane), 1);
        assert_replaced_slot_evidence(&lane, &occurrence, &["cale"]);
        assert_conserved(&lane, None);
    }

    /// 9.5 s at 16 kHz. `emit_long_piece` cuts it into three step-1 windows,
    /// the same geometry as take d566fa17's late occurrences.
    const LONG_SAMPLES: u64 = 152_000;

    fn warning_lines(events: &[EngineEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::Warning { code, message } => Some(format!("{code}: {message}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn record_voiced_spans(lane: &Lane, total: u64, voiced: &[(u64, u64)]) {
        let mut samples = vec![0.0_f32; total as usize];
        for &(start, end) in voiced {
            for sample in &mut samples[start as usize..end as usize] {
                *sample = 0.2;
            }
        }
        let blocks = samples
            .chunks(1_000)
            .map(<[f32]>::to_vec)
            .collect::<Vec<_>>();
        record_energy(lane, &blocks);
    }

    // Feed the exact same PCM through both owners used by production:
    // classification alone cannot authenticate a retained waveform edge.
    fn record_voiced_capture(lane: &mut Lane, total: u64, voiced: &[(u64, u64)]) {
        record_voiced_spans(lane, total, voiced);
        let mut samples = vec![0.0_f32; total as usize];
        for &(start, end) in voiced {
            samples[start as usize..end as usize].fill(0.2);
        }
        lane.state.audio.push(&samples);
        assert_eq!(lane.state.audio.session_sample_end(), total);
    }

    fn qualify_unlabelled(lane: &mut Lane, occurrence: &OccurrenceIdentity) {
        let calibration = EnergyCalibration {
            version: "relay-l1-overlap".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: occurrence.sample_len() as f64 * 1_000.0 / f64::from(RATE),
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .qualify(&evidence, &calibration);
        lane.state.energy_calibration = Some(calibration);
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .require_text_recovery(occurrence)
        );
        lane.state.pending_events.insert(
            1,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: String::new(),
                layer1_baseline: String::new(),
                start_ts: 0.0,
                end_ts: occurrence.sample_len() as f32 / RATE as f32,
                segments: Vec::new(),
            },
        );
    }

    /// Three actual capture-plan frames; retained PCM and measured energy agree.
    fn launch_three_frame_span(
        lane: &mut Lane,
        apple_text: &str,
        voiced: &[(u64, u64)],
    ) -> (OccurrenceIdentity, Vec<TailPatchRequest>) {
        let total = 240_000;
        let occurrence = OccurrenceIdentity::new(&lane.state.session_id, 1, 0, total);
        let mut pcm = vec![0.0_f32; total as usize];
        for &(start, end) in voiced {
            assert!(start < end && end <= total);
            pcm[start as usize..end as usize].fill(0.2);
        }
        record_energy(
            lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        if apple_text.is_empty() {
            qualify_unlabelled(lane, &occurrence);
        } else {
            stage(lane, 1, occurrence.clone(), apple_text);
        }
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
        if !apple_text.is_empty() {
            close_lexicon(lane, 1, &occurrence, apple_text);
        }
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 3);
        for (request, (start, end)) in
            requests
                .iter()
                .zip([(0, 144_000), (48_000, 192_000), (96_000, 240_000)])
        {
            let frame = &request.provider_request.identity.range;
            assert_eq!((frame.sample_start, frame.sample_end), (start, end));
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                (start, end)
            );
            assert_eq!(request.audio, pcm[start as usize..end as usize]);
        }
        (occurrence, requests)
    }

    /// Whisper observation buffer a take selects once, at worker start.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum BufferMode {
        Windows,
        Adaptive,
    }

    impl BufferMode {
        const BOTH: [Self; 2] = [Self::Windows, Self::Adaptive];

        fn session(self, base: &str) -> String {
            match self {
                Self::Windows => base.to_string(),
                Self::Adaptive => format!("{base}-adaptive"),
            }
        }
    }

    // Seed word geometry without replacing it with an occurrence-wide label.
    fn forensic_lane(
        session: &str,
        held: &[(&str, u64, u64)],
    ) -> (Lane, OccurrenceIdentity, Vec<TailPatchRequest>) {
        forensic_lane_in(BufferMode::Windows, session, held)
    }

    /// Both buffer-mode names share `CaptureWindowPlan`. The mode only
    /// changes the session label so the two tests stay distinct.
    fn forensic_lane_in(
        mode: BufferMode,
        session: &str,
        held: &[(&str, u64, u64)],
    ) -> (Lane, OccurrenceIdentity, Vec<TailPatchRequest>) {
        forensic_lane_voiced(mode, session, held, &[(0, 200_000)])
    }

    /// [`forensic_lane_in`] whose owner PCM is speech only over `voiced` and
    /// sub-floor room tone elsewhere, so each spoken word has its own voiced
    /// segment on the capture clock.
    fn forensic_lane_voiced(
        mode: BufferMode,
        session: &str,
        held: &[(&str, u64, u64)],
        voiced: &[(u64, u64)],
    ) -> (Lane, OccurrenceIdentity, Vec<TailPatchRequest>) {
        let _ = mode;
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
        qualify_unlabelled(&mut lane, &owner);
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            let observation = ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
            let words = held
                .iter()
                .map(|(text, start, end)| {
                    crate::pipeline::acoustic_ledger::WordPin::new(*start, *end, *text)
                })
                .collect::<Vec<_>>();
            if !words.is_empty() {
                assert!(
                    ledger
                        .admit_word_slots(&observation, &words)
                        .grants_mutation()
                );
            }
        }
        // The owner's PCM reaches the energy owner too, as a capture does.
        let mut pcm = vec![ROOM_TONE_RMS; owner.sample_len() as usize];
        for &(start, end) in voiced {
            pcm[start as usize..end as usize].fill(0.2);
        }
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        (lane, owner, requests)
    }

    #[test]
    fn forensic_group_edge_refuses_second_prosbe() {
        let session = "forensic-prosbe";
        let (mut lane, owner, requests) = forensic_lane(
            session,
            &[
                ("albo prośbę o", 10_000, 40_000),
                ("wierszyk", 50_000, 60_000),
            ],
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "albo", 12_000, 30_000),
                    word_pin(session, "prośbę", 31_000, 50_000),
                ],
            ),
            12.5,
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(
            ledger
                .text_of(&owner)
                .unwrap()
                .split_whitespace()
                .filter(|word| *word == "prośbę")
                .count(),
            1
        );
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .any(|alternative| alternative.candidate == "albo prośbę o"
                    && alternative.reason == "resegmentation_source_label")
        );
        let sources = ledger
            .slot_alternatives()
            .iter()
            .find(|alternative| {
                alternative.candidate == "albo prośbę o"
                    && alternative.reason == "resegmentation_source_label"
            })
            .unwrap()
            .sources
            .clone();
        // D5 (ledger speech truth brief): absence is not evidence. The capture
        // is speech throughout and the decoded words leave speech under the
        // group uncovered (10 000..12 000, 30 000..31 000, Apple's "o"), so the
        // group keeps its words and the decoded reading waits over it.
        assert!(ledger.slot_alternatives().iter().any(|alternative| {
            alternative.sources == sources
                && alternative.reason == "absence_requires_acoustic_evidence"
        }));
        assert!(
            ledger
                .slots_of(&owner)
                .unwrap()
                .iter()
                .any(|slot| slot.text == "wierszyk"
                    && slot.sample_start == 50_000
                    && slot.sample_end == 60_000)
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn forensic_window_stub_waits_for_next_window() {
        window_stub_waits_for_next_window(BufferMode::Windows, false);
    }

    #[test]
    fn adaptive_buffer_window_stub_waits_for_next_window() {
        window_stub_waits_for_next_window(BufferMode::Adaptive, false);
    }

    #[test]
    fn forensic_same_physical_word_reverse_adaptive() {
        window_stub_waits_for_next_window(BufferMode::Adaptive, true);
    }

    fn window_stub_waits_for_next_window(mode: BufferMode, reverse: bool) {
        let session = &mode.session("forensic-one-physical-word");
        let (_, _, clock_requests) = forensic_lane_in(mode, "forensic-one-word-clock", &[]);
        let end = clock_requests[0].provider_request.identity.range.sample_end;
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
        let mut input = piece(1, &owner, "");
        input.audio.fill(0.0);
        input.audio[(end - 3_500) as usize..(end + 1_500) as usize].fill(0.2);
        record_energy(
            &lane,
            &input
                .audio
                .chunks(1_000)
                .map(<[f32]>::to_vec)
                .collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 200_000, Some((end - 3_500, end + 1_500)));
        qualify_unlabelled(&mut lane, &owner);
        let pcm = input.audio.clone();
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert!(requests.len() >= 2);
        for request in &requests {
            let range = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
        }
        let order = if reverse { [1, 0] } else { [0, 1] };
        for (position, index) in order.into_iter().enumerate() {
            let pin_end = if index == 0 { end } else { end + 2_000 };
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[index],
                    vec![word_pin(session, "Iwo", end - 4_000, pin_end)],
                ),
                12.5,
            );
            if position == 0 && !reverse {
                assert!(held_text(&lane, &owner).is_none_or(|text| !text.contains("Iwo")));
            }
            if position == 0 && reverse {
                assert_eq!(held_text(&lane, &owner).as_deref(), Some("Iwo"));
            }
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        let slots = ledger.slots_of(&owner).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(
            (slots[0].sample_start, slots[0].sample_end),
            (end - 4_000, end + 2_000)
        );
        assert!(lane.state.pending_whisper_stubs.is_empty());
        assert!(
            !ledger.layer_trail().iter().any(|entry| matches!(
                entry.decision,
                MutationReceipt::Refuse {
                    reason: RefuseReason::ReplacedByWhisper,
                    ..
                }
            )),
            "the first window never published the context stub, so no word was retired"
        );
    }

    // Integrator (2026-10-01, take 2bcb99f9): the last word of a decode, cut
    // at the fence, lasted 20 ms; read as a clock lie it refused every later
    // revision of its window and the words over that speech never landed.
    #[test]
    fn integrator_fence_cut_stub_is_not_a_clock_lie() {
        fence_cut_stub_is_not_a_clock_lie(BufferMode::Windows);
    }

    #[test]
    fn integrator_adaptive_fence_cut_stub_is_not_a_clock_lie() {
        fence_cut_stub_is_not_a_clock_lie(BufferMode::Adaptive);
    }

    fn fence_cut_stub_is_not_a_clock_lie(mode: BufferMode) {
        let session = &mode.session("integrator-fence-lie");
        let (mut lane, owner, mut requests) = forensic_lane_in(mode, session, &[]);
        lane.state
            .acoustic_ledger
            .lock()
            .unwrap()
            .bind_capture_rate(RATE);
        let (base, first_end) = shared_window_region(&requests);
        assert_eq!(
            requests[0].provider_request.identity.range.sample_end,
            first_end
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "bo", base - 8_000, base - 2_000),
                    word_pin(session, "nie", first_end - 320, first_end),
                ],
            ),
            12.5,
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    word_pin(session, "nie", first_end - 2_000, first_end + 3_000),
                    word_pin(session, "jest", first_end + 5_000, first_end + 10_000),
                ],
            ),
            12.5,
        );
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        requests.extend(take_requests(&mut lane.tail_rx));
        assert!(
            requests.len() >= 3,
            "Stop must issue the actual owning EOF frame"
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[2],
                vec![
                    word_pin(session, "nie", first_end - 2_000, first_end + 3_000),
                    word_pin(session, "jest", first_end + 5_000, first_end + 10_000),
                ],
            ),
            12.5,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        assert_eq!(
            lane.state.acoustic_ledger.lock().unwrap().clock_lie_count(),
            0,
            "{mode:?}: a fence-cut stub is display evidence, not committed text"
        );
        assert_eq!(
            held_text(&lane, &owner).as_deref(),
            Some("bo nie jest"),
            "{mode:?}"
        );
    }

    #[test]
    fn integrator_coarse_apple_slot_does_not_swallow_whisper_words() {
        coarse_apple_slot_does_not_swallow_whisper_words(BufferMode::Windows);
    }

    #[test]
    fn integrator_coarse_apple_slot_does_not_swallow_whisper_words_adaptive() {
        coarse_apple_slot_does_not_swallow_whisper_words(BufferMode::Adaptive);
    }

    /// Field shape W14 of channel take fc40155d (2026-10-01): Apple held
    /// "usunięcie tego jednego" and one 3.6 s token "Case". Whisper measured
    /// six words there, the first crossing both Apple slots; all six were
    /// refused as ambiguous and the seal read "usunięcie tego jednego Case
    /// Foundation.".
    fn coarse_apple_slot_does_not_swallow_whisper_words(mode: BufferMode) {
        let session = &mode.session("integrator-coarse-slot");
        let (mut lane, owner, requests) = forensic_lane_in(
            mode,
            session,
            &[
                ("usunięcie tego jednego", 4_000, 18_000),
                ("Case", 20_000, 40_000),
            ],
        );
        let range = &requests[0].provider_request.identity.range;
        assert!(
            range.sample_start <= 4_000 && range.sample_end >= 50_000,
            "{mode:?}: window {}..{}",
            range.sample_start,
            range.sample_end
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "na", 17_000, 21_000),
                    word_pin(session, "tym", 21_000, 23_000),
                    word_pin(session, "naprawdę", 23_000, 27_000),
                    word_pin(session, "jesteśmy", 27_000, 31_000),
                    word_pin(session, "z", 31_000, 32_000),
                    word_pin(session, "Technicznym", 32_000, 38_000),
                    word_pin(session, "Foundation.", 40_500, 46_000),
                ],
            ),
            12.5,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        // D5 (ledger speech truth brief): absence is not evidence. The take is
        // speech under both Apple slots, and no decoded word covers 4 000..17 000
        // under the first one, so the Apple words stay (the measured form of
        // this geometry is three_real_frames_preserve_independent_pins_...).
        // "Foundation." lies over speech no held word labels and still lands.
        let text = held_text(&lane, &owner).unwrap_or_default();
        assert_eq!(text, "usunięcie tego jednego Case Foundation.", "{mode:?}");
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .slots_of(&owner)
                .unwrap()
                .iter()
                .any(|slot| slot.text == "Case"
                    && slot.sample_start == 20_000
                    && slot.sample_end == 40_000)
        );
        assert!(
            ledger.word_choices().iter().any(|choice| choice
                .candidate
                .surface
                .contains("tym naprawdę jesteśmy z Technicznym")
                && !choice.accepted
                && !choice.lexical_resolved),
            "{:?}",
            ledger.word_choices()
        );
        assert!(ledger.word_deletions().is_empty());
        drop(ledger);
        assert_eq!(lane.state.session_conservation().residue(), 0, "{mode:?}");
    }

    #[test]
    fn integrator_unpaired_held_tokens_do_not_veto_new_whisper_words() {
        unpaired_held_tokens_do_not_veto_new_whisper_words(BufferMode::Windows);
    }

    #[test]
    fn integrator_unpaired_held_tokens_do_not_veto_new_whisper_words_adaptive() {
        unpaired_held_tokens_do_not_veto_new_whisper_words(BufferMode::Adaptive);
    }

    /// Field shape W27 of channel take fc40155d: one Apple slot spans the
    /// window and opens with five tokens that belong to the previous window.
    /// Whisper does not repeat them, so the group kept itself and vetoed the
    /// eighteen words Whisper measured, among them "I właśnie tam bym teraz
    /// wróciła". Neither description may lose a word.
    fn unpaired_held_tokens_do_not_veto_new_whisper_words(mode: BufferMode) {
        let session = &mode.session("integrator-unpaired-held");
        for physical in [false, true] {
            let label = "powiedzieć tak albo nie żaden człowiek tak po polsku nie mówi";
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(session, 0, 200_000, &[(0, 200_000)]);
            let prefix = [
                ("powiedzieć", 4_000, 8_000),
                ("tak", 8_000, 11_000),
                ("albo", 11_000, 15_000),
                ("nie", 15_000, 18_000),
                ("żaden", 18_000, 22_000),
            ];
            let source = {
                let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
                let observation =
                    ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
                let pins = if physical {
                    let mut pins = prefix
                        .iter()
                        .map(|&(word, start, end)| {
                            crate::pipeline::acoustic_ledger::WordPin::new(start, end, word)
                        })
                        .collect::<Vec<_>>();
                    pins.push(crate::pipeline::acoustic_ledger::WordPin::new(
                        22_000,
                        40_000,
                        "człowiek tak po polsku nie mówi",
                    ));
                    pins
                } else {
                    vec![crate::pipeline::acoustic_ledger::WordPin::new(
                        4_000, 40_000, label,
                    )]
                };
                assert!(
                    ledger
                        .admit_word_slots(&observation, &pins)
                        .grants_mutation()
                );
                ledger.slots_of(&owner).unwrap()[0].clone()
            };
            let range = &requests[0].provider_request.identity.range;
            assert!(
                range.sample_start <= 4_000 && range.sample_end >= 60_000,
                "{mode:?}: window {}..{}",
                range.sample_start,
                range.sample_end
            );
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(session, "człowiek", 22_000, 25_000),
                        word_pin(session, "tak", 25_000, 26_500),
                        word_pin(session, "po", 26_500, 28_000),
                        word_pin(session, "polsku", 28_000, 32_000),
                        word_pin(session, "nie", 32_000, 34_000),
                        word_pin(session, "mówi.", 34_000, 38_000),
                        word_pin(session, "I", 42_000, 43_000),
                        word_pin(session, "właśnie", 43_000, 46_000),
                        word_pin(session, "tam", 46_000, 48_000),
                        word_pin(session, "bym", 48_000, 50_000),
                        word_pin(session, "teraz", 50_000, 53_000),
                        word_pin(session, "wróciła.", 53_000, 57_000),
                    ],
                ),
                12.5,
            );
            lane.state
                .return_outstanding_whisper_without_label(&lane.tx);
            let text = held_text(&lane, &owner).unwrap_or_default();
            assert!(
                text.contains("I właśnie tam bym teraz wróciła"),
                "{mode:?}: Whisper words outside the held tokens were vetoed: {text:?}"
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            if physical {
                assert!(
                    text.starts_with("powiedzieć tak albo nie żaden człowiek"),
                    "{mode:?}: individually admitted Words must survive an omission: {text:?}"
                );
                let kept = ledger
                    .slots_of(&owner)
                    .unwrap()
                    .iter()
                    .filter(|pin| pin.sample_end <= 22_000)
                    .map(|pin| (pin.text.as_str(), pin.sample_start, pin.sample_end))
                    .collect::<Vec<_>>();
                assert_eq!(kept, prefix);
            } else {
                // A coarse source does not prove five individual Words, but
                // one uncorroborated frame must not remove its negation either.
                // New words outside its scope still land independently.
                assert!(
                    text.starts_with(label),
                    "polarity source was erased: {text}"
                );
                assert!(ledger.slots_of(&owner).unwrap().contains(&source));
                assert_eq!(
                    ledger
                        .slot_operations()
                        .iter()
                        .filter(|operation| operation.sources.contains(&source))
                        .count(),
                    0
                );
            }
            assert!(ledger.word_deletions().is_empty());
            assert!(
                !ledger.is_sealed(&owner),
                "cancelled decode work cannot certify completion"
            );
            drop(ledger);
            assert_eq!(text.matches("polsku").count(), 1, "{mode:?}: {text:?}");
            assert_eq!(lane.state.session_conservation().residue(), 0, "{mode:?}");
        }
    }

    // A real clock lie (held text far faster than speech) still cannot
    // authorize replacing its neighbour, yet words over audio that holds no
    // word are admitted around it (N1).
    #[test]
    fn integrator_clock_lie_never_drops_words_over_unheld_speech() {
        let session = "integrator-lie-n1";
        let (mut lane, owner, requests) = forensic_lane(session, &[]);
        lane.state
            .acoustic_ledger
            .lock()
            .unwrap()
            .bind_capture_rate(RATE);
        let (base, _) = shared_window_region(&requests);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "najprawdopodobniej", base - 9_000, base - 8_840),
                    word_pin(session, "tak", base - 6_000, base - 2_000),
                    word_pin(session, "potem", base + 2_000, base + 7_000),
                    word_pin(session, "dalej", base + 9_000, base + 14_000),
                ],
            ),
            12.5,
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    word_pin(session, "potem", base + 2_000, base + 7_000),
                    word_pin(session, "dalej", base + 9_000, base + 14_000),
                ],
            ),
            12.5,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let held = held_text(&lane, &owner).unwrap_or_default();
        for word in ["tak", "potem", "dalej"] {
            assert!(held.contains(word), "{word} lost: {held}");
        }
    }

    // Integrator (2026-10-01): the terminal coverage receipt carries the
    // take's bound capture clock, so the overlay can place uncovered speech.
    #[test]
    fn integrator_terminal_coverage_receipt_carries_the_capture_clock() {
        let (lane, _owner, _requests) = forensic_lane("integrator-coverage-clock", &[]);
        let receipt = publish_terminal_coverage(&lane.state, &lane.tx);
        assert_eq!(receipt.sample_rate_hz, Some(RATE));
    }

    #[test]
    fn forensic_terminal_frontier_deadline_windows() {
        terminal_frontier_requires_successful_pcm(BufferMode::Windows, false, false);
    }

    #[test]
    fn forensic_terminal_frontier_deadline_adaptive() {
        terminal_frontier_requires_successful_pcm(BufferMode::Adaptive, false, false);
    }

    #[test]
    fn forensic_terminal_frontier_voiced_fence_windows() {
        terminal_frontier_requires_successful_pcm(BufferMode::Windows, true, true);
    }

    #[test]
    fn forensic_terminal_frontier_voiced_fence_adaptive() {
        terminal_frontier_requires_successful_pcm(BufferMode::Adaptive, true, true);
    }

    #[test]
    fn forensic_first_whisper_measured_windows() {
        first_whisper_measured_capture(false, true, false);
    }

    #[test]
    fn forensic_first_whisper_measured_adaptive() {
        first_whisper_measured_capture(true, true, false);
    }

    #[test]
    fn forensic_first_whisper_measured_refusals() {
        for adaptive in [false, true] {
            first_whisper_measured_capture(adaptive, false, false);
            first_whisper_measured_capture(adaptive, true, true);
        }
    }

    /// A synthetic returned frame clears debt only when admission does.
    pub(super) fn admit_completed_window_recovery(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        occurrence: &OccurrenceIdentity,
        payload: &TailProviderPayload,
    ) -> bool {
        state.admit_completed_window(ev_tx, payload, true);
        !state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .text_recovery_pending(occurrence)
    }

    pub(super) fn first_whisper_measured_capture(
        adaptive: bool,
        decoded: bool,
        voiced_fence: bool,
    ) {
        let session = "forensic-first-whisper";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let speech_end = if voiced_fence { 48_000 } else { 16_000 };
        let mut pcm = vec![0.0_f32; 48_000];
        pcm[6_000..speech_end as usize].fill(0.2);
        record_energy(
            &lane,
            &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 48_000, Some((6_000, speech_end)));
        lane.state.audio.push(&pcm);
        lane.state.energy_calibration = Some(EnergyCalibration {
            version: "forensic-first-whisper-measured".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        });
        let mut closed = super::super::silero_fusion::UtteranceLedger::new();
        closed.open_or_extend(session, 1, 0, 48_000);
        closed.close_open(48_000);
        assert!(reconcile_silero_ledger(
            &mut lane.state,
            &lane.tx,
            &closed,
            &[]
        ));
        {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.text_of(&owner).is_none());
            assert!(ledger.slots_of(&owner).is_none());
            assert!(ledger.text_recovery_pending(&owner));
        }
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let mut requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert_eq!(request.audio, pcm);
        assert_eq!(request.provider_request.identity.range.sample_start, 0);
        assert_eq!(request.provider_request.identity.range.sample_end, 48_000);
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        if decoded {
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(&request, vec![word_pin(session, "Iwo", 4_000, 48_000)]),
                3.0,
            );
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        lane.state.close_admission_horizon(&lane.tx, 48_000);
        assert_eq!(lane.state.tail_patch_awaiting_completion(), 0);
        assert!(lane.state.refinement_pending.is_empty());
        if decoded && !voiced_fence {
            // With no Apple label, production schedules only Whisper. The
            // deterministic lexicon rewrite runs inside routed-word admission;
            // there is no separately scheduled L2 callback to manufacture.
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 1);
            assert_eq!(slots[0].producer, ObservationProducer::Whisper);
            assert_eq!(
                (slots[0].sample_start, slots[0].sample_end),
                (4_000, 48_000)
            );
            assert!(!ledger.text_recovery_pending(&owner));
            let seal = ledger
                .seal_of(&owner)
                .expect("successful measured first word seals its owned PCM")
                .clone();
            let before = slots.to_vec();
            drop(ledger);
            assert!(lane.state.stop_document_settled());
            let before_events = drain(&mut lane.rx);
            // The word is deferred at its decode fence and committed by the
            // terminal settlement. Job counters describe the earlier callback;
            // the actual mutation/word clock below prove successful admission.
            assert_eq!(
                lane.state.tail_patch_jobs_applied + lane.state.tail_patch_jobs_skipped,
                1
            );
            for event in &before_events {
                if let EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                } = event
                {
                    eprintln!(
                        "first Whisper {:?} {} {:?}",
                        observation.producer, label, receipt
                    );
                }
            }
            assert!(!before_events.iter().any(|event| matches!(event,
                EngineEvent::Warning { code, .. } if code == RefinementFailure::NoLabel.code())));
            assert_eq!(before_events.iter().filter(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { .. } }
                    if observation.producer == ObservationProducer::Whisper && label == "Iwo"
            )).count(), 1);
            assert_eq!(
                before_events
                    .iter()
                    .filter(|event| matches!(event,
                        EngineEvent::UtteranceFinal { text, .. } if text == "Iwo"
                    ))
                    .count(),
                1
            );
            assert_eq!(
                before_events
                    .iter()
                    .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                    .count(),
                1
            );
            assert!(reconcile_silero_ledger(
                &mut lane.state,
                &lane.tx,
                &closed,
                &[TranscriptSegment {
                    confidence: None,
                    text: "late replacement".into(),
                    start_ts: 0.25,
                    end_ts: 3.0,
                }],
            ));
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
            assert_eq!(ledger.seal_of(&owner), Some(&seal));
            assert!(!ledger.text_recovery_pending(&owner));
            assert_eq!(ledger.post_seal_decisions(&owner).len(), 1);
            assert!(take_requests(&mut lane.tail_rx).is_empty());
        } else {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .frontier_of(&owner)
                    .unwrap()
                    .open_producers()
                    .is_empty()
            );
            assert!(
                ledger.text_recovery_pending(&owner),
                "adaptive={adaptive} decoded={decoded} voiced_fence={voiced_fence} text={:?}",
                ledger.text_of(&owner)
            );
            assert!(!ledger.is_sealed(&owner));
            drop(ledger);
            assert!(!lane.state.stop_document_settled());
            assert!(
                !drain(&mut lane.rx)
                    .iter()
                    .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            );
        }
    }

    fn terminal_frontier_requires_successful_pcm(
        mode: BufferMode,
        decoded: bool,
        voiced_fence: bool,
    ) {
        let session = &mode.session("forensic-terminal-frontier");
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let speech_end = if voiced_fence { 48_000 } else { 16_000 };
        let mut pcm = vec![ROOM_TONE_RMS; 48_000];
        pcm[6_000..speech_end as usize].fill(0.2);
        record_energy(
            &lane,
            &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 48_000, Some((6_000, speech_end)));
        lane.state.audio.push(&pcm);
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .require_text_recovery(&owner)
        );
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let mut requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert_eq!(request.audio, pcm);
        let range = &request.provider_request.identity.range;
        assert_eq!((range.sample_start, range.sample_end), (0, 48_000));
        if decoded {
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(&request, vec![word_pin(session, "Iwo", 4_000, 48_000)]),
                3.0,
            );
            let returned_events = drain(&mut lane.rx);
            if voiced_fence {
                assert!(unanchored_label(&returned_events, "Iwo"));
                assert_eq!(held_text(&lane, &owner).as_deref(), Some("hipoteza"));
            } else {
                assert_eq!(held_text(&lane, &owner).as_deref(), Some("Iwo"));
                assert!(!unanchored_label(&returned_events, "Iwo"));
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(!ledger.text_recovery_pending(&owner));
                assert!(ledger.word_deletions().is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        assert_eq!(lane.state.tail_patch_awaiting_completion(), 0);
        assert!(lane.state.refinement_pending.is_empty());
        {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.frontier_of(&owner).unwrap().open_producers(),
                vec![ObservationProducer::Lexicon]
            );
            assert!(
                !ledger.is_sealed(&owner),
                "actual scheduled L2 has not returned"
            );
        }
        let label = if decoded { "Iwo" } else { "hipoteza" };
        close_lexicon(&mut lane, 1, &owner, label);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .frontier_of(&owner)
                .unwrap()
                .open_producers()
                .is_empty()
        );
        if decoded && !voiced_fence {
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 1);
            assert_eq!(slots[0].producer, ObservationProducer::Whisper);
            assert_eq!(
                (slots[0].sample_start, slots[0].sample_end),
                (4_000, 48_000)
            );
            assert!(!ledger.text_recovery_pending(&owner));
            assert!(ledger.is_sealed(&owner));
            drop(ledger);
            assert!(lane.state.stop_document_settled());
        } else {
            assert!(
                ledger.text_recovery_pending(&owner),
                "drained queue or speech-cut fence cannot certify decoder work"
            );
            assert!(!ledger.is_sealed(&owner));
            drop(ledger);
            assert!(!lane.state.stop_document_settled());
        }
    }

    #[test]
    fn forensic_window_stub_is_accounted_at_seal() {
        window_stub_is_accounted_at_seal(BufferMode::Windows);
    }

    #[test]
    fn adaptive_buffer_window_stub_is_accounted_at_seal() {
        window_stub_is_accounted_at_seal(BufferMode::Adaptive);
    }

    fn window_stub_is_accounted_at_seal(mode: BufferMode) {
        terminal_frontier_requires_successful_pcm(mode, true, false);
    }

    #[test]
    fn forensic_unclipped_seam_pin_is_heard_in_prior_owner() {
        let session = "forensic-seam";
        let mut lane = open(session);
        let a = OccurrenceIdentity::new(session, 1, 0, 100_000);
        let b = OccurrenceIdentity::new(session, 1, 100_000, 200_000);
        qualify_unlabelled(&mut lane, &a);
        stage(&mut lane, 2, b.clone(), "");
        {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            let observation = ledger.next_word_observation(ObservationProducer::Whisper, 99, &a);
            assert!(
                ledger
                    .admit_word_slots(
                        &observation,
                        &[crate::pipeline::acoustic_ledger::WordPin::new(
                            90_000, 100_000, "dużo"
                        )]
                    )
                    .grants_mutation()
            );
        }
        // The one physical word is in A. The decoder's shifted long pin
        // must not create another occurrence in the measured quiet of B.
        let mut pcm = vec![0.0_f32; 200_000];
        pcm[89_920..99_840].fill(0.2);
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            (
                requests[0].provider_request.identity.range.sample_start,
                requests[0].provider_request.identity.range.sample_end,
            ),
            (0, 144_000),
            "the seam pin is inside the first full capture window"
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "dużo", 92_000, 140_000)],
            ),
            12.5,
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        let count = [a, b]
            .iter()
            .flat_map(|owner| ledger.slots_of(owner).unwrap_or(&[]))
            .filter(|slot| slot.text == "dużo")
            .count();
        assert_eq!(count, 1);
        assert_eq!(ledger.conservation().residue(), 0);
    }

    fn measured_coarse_apple_hypothesis_yields_to_words(mode: BufferMode, leading_speech: bool) {
        use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
        use crate::pipeline::acoustic_ledger::WordPin;
        let session = "physical-coarse-to-four";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
        // Room quiet below the active floor, not a voice gate's exact zeros.
        let mut pcm: Vec<f32> = (0..200_000)
            .map(|i| if i % 2 == 0 { 5.0e-5 } else { -5.0e-5 })
            .collect();
        let mut speech = vec![
            (13_000, 19_000),
            (23_000, 29_000),
            (33_000, 39_000),
            (43_000, 53_000),
            (172_000, 178_000),
        ];
        if leading_speech {
            speech.push((10_000, 12_000));
        }
        speech.sort_unstable();
        for &(start, end) in &speech {
            pcm[start as usize..end as usize].fill(0.2);
        }
        record_energy(
            &lane,
            &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let mut fusion = SileroIngress::new(RATE, session.to_string(), 1);
        assert!(fusion.vad_available());
        fusion.note_observed_pcm(200_000, 200_000);
        for &(start, end) in &speech {
            fusion.observe_boundaries(&[
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechStart,
                    sample: start,
                    speech_probability: 0.9,
                },
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechEnd,
                    sample: end,
                    speech_probability: 0.1,
                },
            ]);
        }
        lane.state.fusion = Some(fusion);
        qualify_unlabelled(&mut lane, &owner);
        let (source, unrelated) = {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            let observation = ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
            let receipt = ledger.admit_word_slots(
                &observation,
                &[
                    WordPin::new(10_000, 55_000, "wierszu kiedyś"),
                    WordPin::new(170_000, 180_000, "dalej"),
                ],
            );
            assert!(receipt.grants_mutation());
            let slots = ledger.slots_of(&owner).unwrap();
            (slots[0].clone(), slots[1].clone())
        };
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert!(
            requests.len() > 1,
            "{mode:?}: the unrelated later span must remain undecoded"
        );
        assert_eq!(
            (
                requests[0].provider_request.identity.range.sample_start,
                requests[0].provider_request.identity.range.sample_end,
                requests[0].admit_sample_start,
                requests[0].admit_sample_end,
            ),
            (0, 144_000, 0, 144_000),
            "{mode:?}"
        );
        for request in &requests {
            let range = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
        }
        let range = &requests[0].provider_request.identity.range;
        assert!(range.sample_start <= 10_000 && range.sample_end >= 55_000);
        assert!(range.sample_end < unrelated.sample_start);
        let expected = [
            ("wierszyk", 12_000, 20_000),
            ("był", 22_000, 30_000),
            ("sobie", 32_000, 40_000),
            ("kiedyś", 42_000, 54_000),
        ];
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                expected
                    .iter()
                    .map(|&(label, start, end)| word_pin(session, label, start, end))
                    .collect(),
            ),
            12.5,
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        if leading_speech {
            // D5 (ledger speech truth brief): absence is not evidence. The
            // measured speech at 10 000..12 000 lies under the coarse Apple
            // source and under no decoded word, so the source keeps its words
            // and the decoded reading waits as an alternative over it.
            assert_eq!(ledger.text_of(&owner), Some("wierszu kiedyś dalej"));
            assert_eq!(
                ledger.slots_of(&owner).unwrap(),
                [source.clone(), unrelated.clone()]
            );
            assert!(ledger.slot_alternatives().iter().any(|alternative| {
                alternative.observation.producer == ObservationProducer::Whisper
                    && alternative.sources == [source.clone()]
                    && alternative.reason == "absence_requires_acoustic_evidence"
            }));
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
            return;
        }
        assert_eq!(
            ledger.text_of(&owner),
            Some("wierszyk był sobie kiedyś dalej")
        );
        let slots = ledger.slots_of(&owner).unwrap();
        assert_eq!(slots.len(), 5);
        assert_eq!(
            slots[4], unrelated,
            "unobserved later source is not rebuilt"
        );
        for (slot, &(label, start, end)) in slots[..4].iter().zip(&expected) {
            assert_eq!(
                (slot.text.as_str(), slot.sample_start, slot.sample_end),
                (label, start, end)
            );
            assert_eq!(slot.producer, ObservationProducer::Whisper);
            assert!(
                ledger
                    .slot_source_ranges(slot)
                    .contains(&OccurrenceIdentity::new(session, 1, start, end))
            );
        }
        let operations = ledger
            .slot_operations()
            .iter()
            .filter(|op| op.observation == slots[0].observation)
            .collect::<Vec<_>>();
        let accounting = operations
            .iter()
            .filter(|op| op.sources.contains(&source))
            .collect::<Vec<_>>();
        assert_eq!(
            accounting.len(),
            1,
            "the coarse source is accounted once, not preserved as a lexical floor"
        );
        assert!(
            accounting[0]
                .source_ranges
                .iter()
                .any(|r| r.same_capture(&owner)
                    && r.sample_start == source.sample_start
                    && r.sample_end == source.sample_end)
        );
        let outputs = operations
            .iter()
            .flat_map(|op| &op.outputs)
            .collect::<Vec<_>>();
        assert_eq!(outputs.len(), 4);
        for slot in &slots[..4] {
            assert_eq!(outputs.iter().filter(|output| **output == slot).count(), 1);
        }
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
        assert!(
            !ledger.is_sealed(&owner),
            "one successful window cannot seal the whole take"
        );
        drop(ledger);
        assert!(!lane.state.stop_document_settled());
    }

    #[test]
    fn forensic_measured_coarse_hypothesis_adaptive() {
        for leading_speech in [false, true] {
            measured_coarse_apple_hypothesis_yields_to_words(BufferMode::Adaptive, leading_speech);
        }
    }

    #[test]
    fn forensic_resegment_coarse_slot_with_other_slots_and_full_speech() {
        measured_coarse_apple_hypothesis_yields_to_words(BufferMode::Windows, false);
    }

    #[test]
    fn forensic_resegment_keeps_uncovered_held_speech() {
        measured_coarse_apple_hypothesis_yields_to_words(BufferMode::Windows, true);
    }

    #[test]
    fn forensic_stub_after_wider_window_never_adds_second_copy() {
        window_stub_waits_for_next_window(BufferMode::Windows, true);
    }

    #[test]
    fn forensic_measured_partition_keeps_each_source_lineage() {
        measured_partition_keeps_each_source_lineage(34_000);
    }

    fn measured_partition_keeps_each_source_lineage(second_start: u64) {
        use crate::pipeline::acoustic_ledger::WordPin;
        let session = "forensic-physical-three-to-two";
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
        let mut pcm = vec![0.0_f32; 200_000];
        let speech = [
            (12_000, 24_000),
            (27_000, 33_000),
            (36_000, 58_000),
            (72_000, 78_000),
        ];
        for &(start, end) in &speech {
            pcm[start as usize..end as usize].fill(0.2);
        }
        record_energy(
            &lane,
            &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
        let mut fusion = SileroIngress::new(RATE, session.to_string(), 1);
        assert!(fusion.vad_available());
        fusion.note_observed_pcm(200_000, 200_000);
        for &(start, end) in &speech {
            fusion.observe_boundaries(&[
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechStart,
                    sample: start,
                    speech_probability: 0.9,
                },
                VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechEnd,
                    sample: end,
                    speech_probability: 0.1,
                },
            ]);
        }
        lane.state.fusion = Some(fusion);
        qualify_unlabelled(&mut lane, &owner);
        let sources = {
            let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
            let observation = ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
            let receipt = ledger.admit_word_slots(
                &observation,
                &[
                    WordPin::new(10_000, 25_000, "zima"),
                    WordPin::new(25_000, 40_000, "usza"),
                    WordPin::new(40_000, 60_000, "rości"),
                    WordPin::new(70_000, 80_000, "dalej"),
                ],
            );
            assert!(receipt.grants_mutation(), "{receipt:?}");
            ledger.slots_of(&owner).unwrap()[..3].to_vec()
        };
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            (
                requests[0].provider_request.identity.range.sample_start,
                requests[0].provider_request.identity.range.sample_end,
            ),
            (0, 144_000)
        );
        assert!(!requests.is_empty());
        for request in &requests {
            let range = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
        }
        let range = &requests[0].provider_request.identity.range;
        assert!(range.sample_start <= 10_000 && range.sample_end >= 60_000);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "zimą", 12_000, 34_000),
                    word_pin(session, "szarości", second_start, 58_000),
                ],
            ),
            12.5,
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        if second_start < 34_000 {
            // Overlapping provider pins fail validation. A bounded trial may
            // not turn that malformed payload into a resolved lexical dispute.
            assert_eq!(ledger.text_of(&owner), Some("zima usza rości dalej"));
            assert_eq!(&ledger.slots_of(&owner).unwrap()[..3], sources.as_slice());
            assert!(
                !ledger.has_word_conflicts(),
                "failed trials are closed, not retried forever"
            );
            return;
        }
        assert_eq!(ledger.text_of(&owner), Some("zima usza rości dalej"));
        assert_eq!(&ledger.slots_of(&owner).unwrap()[..3], sources.as_slice());
        // Independent word pins are protected before lexical adjudication.
        // A refused contraction remains an alternative, not a lexical vote.
        assert!(
            ledger.slot_alternatives().iter().any(|alternative| {
                alternative
                    .candidate
                    .split_whitespace()
                    .any(|word| word == "zimą")
                    && sources[..2]
                        .iter()
                        .all(|source| alternative.sources.contains(source))
            }),
            "the refused contraction must retain both independent sources"
        );
        assert!(
            ledger.word_choices().iter().any(|choice| {
                choice.candidate.surface == "szarości"
                    && !choice.accepted
                    && !choice.lexical_resolved
            }),
            "the legal 1:1 candidate still needs lexical evidence"
        );
        assert!(!ledger.slot_operations().iter().any(|operation| {
            operation
                .sources
                .iter()
                .any(|source| sources.contains(source))
        }));
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn three_real_frames_preserve_independent_pins_and_refine_coarse_sources() {
        use crate::pipeline::acoustic_ledger::WordPin;
        for partitioned in [false, true] {
            let session = if partitioned {
                "three-to-two-real-frames"
            } else {
                "coarse-real-frames"
            };
            let pin_time = |sample: u64| {
                if partitioned {
                    96_000 + sample / 2
                } else {
                    96_000 + sample
                }
            };
            let held = if partitioned {
                vec![
                    ("zima", 10_000, 25_000),
                    ("usza", 25_000, 40_000),
                    ("rości", 40_000, 60_000),
                    ("dalej", 70_000, 80_000),
                ]
            } else {
                vec![
                    ("usunięcie tego jednego", 4_000, 18_000),
                    ("Case", 20_000, 40_000),
                ]
            };
            let candidates = if partitioned {
                vec![("zimą", 12_000, 34_000), ("szarości", 34_000, 58_000)]
            } else {
                vec![
                    ("na", 17_000, 21_000),
                    ("tym", 21_000, 23_000),
                    ("naprawdę", 23_000, 27_000),
                    ("jesteśmy", 27_000, 31_000),
                    ("z", 31_000, 32_000),
                    ("Technicznym", 32_000, 38_000),
                    ("Foundation.", 40_500, 46_000),
                ]
            };
            let voice = if partitioned {
                vec![
                    (12_000, 24_000),
                    (27_000, 33_000),
                    (36_000, 58_000),
                    (72_000, 78_000),
                ]
            } else {
                vec![(4_000, 46_000)]
            };
            let voice = voice
                .into_iter()
                .map(|(start, end)| (pin_time(start), pin_time(end)))
                .collect::<Vec<_>>();
            let mut lane = open(session);
            let (owner, requests) = launch_three_frame_span(&mut lane, "", &voice);
            let sources = {
                let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
                let observation =
                    ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
                let words = held
                    .iter()
                    .map(|&(label, start, end)| WordPin::new(pin_time(start), pin_time(end), label))
                    .collect::<Vec<_>>();
                assert!(
                    ledger
                        .admit_word_slots(&observation, &words)
                        .grants_mutation()
                );
                ledger.slots_of(&owner).unwrap().to_vec()
            };
            for request in &requests {
                let offered = candidates
                    .iter()
                    .map(|&(label, start, end)| {
                        word_pin(session, label, pin_time(start), pin_time(end))
                    })
                    .collect();
                let returned = completion(request, offered);
                returned.payload.as_ref().unwrap().validate().unwrap();
                lane.state.complete_whisper_window(&lane.tx, returned, 15.0);
            }
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            // D5 (ledger speech truth brief): absence is not evidence. In the
            // coarse case the measured voice runs 4 000..46 000 and no decoded
            // word covers 4 000..17 000 under the Apple source, so the Apple
            // words stay and the decoded reading waits as an alternative.
            // "Foundation." lies over speech no held word labels and lands.
            let expected = if partitioned {
                "zima usza rości dalej"
            } else {
                "usunięcie tego jednego Case Foundation."
            };
            assert_eq!(
                ledger.text_of(&owner),
                Some(expected),
                "partitioned={partitioned}; choices={:?}",
                ledger.word_choices()
            );
            if !partitioned {
                let slots = ledger.slots_of(&owner).unwrap();
                assert_eq!(&slots[..sources.len()], sources.as_slice());
                assert_eq!(
                    slots[sources.len()..]
                        .iter()
                        .map(|slot| (slot.text.as_str(), slot.producer))
                        .collect::<Vec<_>>(),
                    [("Foundation.", ObservationProducer::Whisper)]
                );
                assert!(ledger.slot_alternatives().iter().any(|alternative| {
                    alternative.observation.producer == ObservationProducer::Whisper
                        && alternative.reason == "absence_requires_acoustic_evidence"
                }));
            }
            // One owned frame cannot manufacture lexical authority to
            // relabel a narrower measured source or contract its neighbor.
            if partitioned {
                let slots = ledger.slots_of(&owner).unwrap();
                assert_eq!(slots.len(), sources.len());
                for source in &sources {
                    assert!(
                        slots.contains(source),
                        "independent source changed: {source:?}"
                    );
                    assert!(
                        !ledger
                            .slot_operations()
                            .iter()
                            .any(|operation| { operation.sources.contains(source) }),
                        "a protected source must not be retired"
                    );
                }
                assert!(
                    ledger
                        .slot_operations()
                        .iter()
                        .all(|operation| { operation.sources.len() <= operation.outputs.len() }),
                    "lexical agreement cannot contract independent word pins"
                );
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_overlapping_word_trial_keeps_three_sources() {
        measured_partition_keeps_each_source_lineage(33_000);
    }

    #[test]
    fn forensic_stub_cannot_overwrite_complete_next_window_word() {
        let session = "forensic-stub-correction";
        let (mut lane, owner, requests) = forensic_lane(session, &[]);
        let end = requests[0].provider_request.identity.range.sample_end;
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "wierszu", end - 4_000, end)],
            ),
            12.5,
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![word_pin(session, "wierszyk", end - 4_000, end + 20_000)],
            ),
            12.5,
        );
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let tail = take_requests(&mut lane.tail_rx);
        assert_eq!(tail.len(), 1, "the ordinary EOF owner of the full word");
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &tail[0],
                vec![word_pin(session, "wierszyk", end - 4_000, end + 20_000)],
            ),
            12.5,
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        assert_eq!(held_text(&lane, &owner).as_deref(), Some("wierszyk"));
        assert!(lane.state.pending_whisper_stubs.is_empty());
    }

    #[test]
    fn forensic_real_repetitions_on_distinct_pcm_stay() {
        real_repetitions_on_distinct_pcm_stay(BufferMode::Windows);
    }

    #[test]
    fn adaptive_buffer_real_repetitions_on_distinct_pcm_stay() {
        real_repetitions_on_distinct_pcm_stay(BufferMode::Adaptive);
    }

    fn real_repetitions_on_distinct_pcm_stay(mode: BufferMode) {
        let session = &mode.session("forensic-repetitions");
        let spoken = [
            ("Iwo", 1_000, 5_000),
            ("Iwo", 7_000, 11_000),
            ("Iwo", 13_000, 17_000),
            ("Iwo", 19_000, 23_000),
            ("Iwo", 25_000, 29_000),
            ("trzema", 31_000, 38_000),
            ("trzema", 40_000, 48_000),
        ];
        // Each repeat is its own voiced burst, with room tone between them.
        let voiced = spoken.map(|(_, start, end)| (start, end));
        let (mut lane, owner, requests) = forensic_lane_voiced(mode, session, &[], &voiced);
        let pins = spoken
            .iter()
            .map(|&(text, start, end)| word_pin(session, text, start, end))
            .collect::<Vec<_>>();
        lane.state
            .complete_whisper_window(&lane.tx, completion(&requests[0], pins), 12.5);
        assert_eq!(
            held_text(&lane, &owner).as_deref(),
            Some("Iwo Iwo Iwo Iwo Iwo trzema trzema")
        );
    }

    /// Start of the second window and the region both windows decode.
    fn shared_window_region(requests: &[TailPatchRequest]) -> (u64, u64) {
        let first = &requests[0].provider_request.identity.range;
        let second = &requests[1].provider_request.identity.range;
        assert!(
            first.sample_end >= second.sample_start + 15_000,
            "the two windows must share at least 15 000 samples: first {}..{}, second {}..{}",
            first.sample_start,
            first.sample_end,
            second.sample_start,
            second.sample_end
        );
        (second.sample_start, first.sample_end)
    }

    // Take agent-channel-3-22e65577 (Astra's evidence): generation 0 brought one
    // "dość", generation 1 one adjacent "dość"; the ledger kept both.
    #[test]
    fn integrator_adjacent_batches_each_with_one_word_keep_one_copy() {
        adjacent_batches_each_with_one_word_keep_one_copy(BufferMode::Windows);
    }

    #[test]
    fn adaptive_buffer_adjacent_batches_each_with_one_word_keep_one_copy() {
        adjacent_batches_each_with_one_word_keep_one_copy(BufferMode::Adaptive);
    }

    fn adjacent_batches_each_with_one_word_keep_one_copy(mode: BufferMode) {
        forensic_adjacent_batches_physical_identity(mode, false);
    }

    #[test]
    fn forensic_disjoint_equal_labels_remain_two_windows() {
        forensic_adjacent_batches_physical_identity(BufferMode::Windows, true);
    }

    #[test]
    fn forensic_disjoint_equal_labels_remain_two_adaptive() {
        forensic_adjacent_batches_physical_identity(BufferMode::Adaptive, true);
    }

    fn forensic_adjacent_batches_physical_identity(mode: BufferMode, distinct: bool) {
        let session = &mode.session("integrator-dosc");
        let (mut lane, owner, requests, pcm) =
            forensic_boundary_capture(session, 0, 200_000, &[(0, 200_000)]);
        assert_eq!(pcm.len(), 200_000);
        assert!(requests.len() >= 2);
        let (base, _) = shared_window_region(&requests);
        // All four original pins are in the first window's owning band.
        let mut first_words = vec![
            word_pin(session, "to", base + 2_000, base + 8_000),
            word_pin(session, "dość", base + 8_000, base + 10_000),
        ];
        if distinct {
            first_words.push(word_pin(session, "dość", base + 10_000, base + 21_000));
        }
        first_words.push(word_pin(
            session,
            "niesympatyczne",
            base + 21_000,
            base + 40_000,
        ));
        lane.state
            .complete_whisper_window(&lane.tx, completion(&requests[0], first_words), 12.5);
        let first_admission = {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            let slots = ledger
                .slots_of(&owner)
                .unwrap_or(&[])
                .iter()
                .map(|slot| (slot.text.clone(), slot.sample_start, slot.sample_end))
                .collect::<Vec<_>>();
            let choices = ledger
                .word_choices()
                .iter()
                .map(|choice| {
                    (
                        choice.reason.clone(),
                        choice.accepted,
                        choice.selected_label.clone(),
                    )
                })
                .collect::<Vec<_>>();
            (slots, choices)
        };
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    word_pin(session, "to", base + 2_200, base + 8_000),
                    word_pin(
                        session,
                        "dość",
                        base + if distinct { 10_000 } else { 8_000 },
                        base + if distinct { 21_000 } else { 10_000 },
                    ),
                    word_pin(session, "niesympatyczne", base + 21_000, base + 40_000),
                ],
            ),
            12.5,
        );
        let text = held_text(&lane, &owner).unwrap_or_default();
        assert_eq!(
            text.split_whitespace()
                .filter(|word| *word == "dość")
                .count(),
            if distinct { 2 } else { 1 },
            "{text}; first_admission={first_admission:?}"
        );
        assert!(
            text.contains(if distinct {
                "to dość dość niesympatyczne"
            } else {
                "to dość niesympatyczne"
            }),
            "{text}"
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        let actual_pins = ledger
            .slots_of(&owner)
            .expect("physical word slots")
            .iter()
            .filter(|slot| slot.text == "dość")
            .map(|slot| (slot.sample_start, slot.sample_end))
            .collect::<Vec<_>>();
        let mut expected_pins = vec![(base + 8_000, base + 10_000)];
        if distinct {
            expected_pins.push((base + 10_000, base + 21_000));
        }
        assert_eq!(
            actual_pins, expected_pins,
            "count follows physical source pins, not matching labels"
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    // Same take: generation 13 decoded from inside "niesympatyczne" and offered
    // "sympatyczne" on the same PCM; the ledger replaced the complete word.
    #[test]
    fn integrator_pin_clipped_at_window_start_cannot_replace_a_complete_word() {
        pin_clipped_at_window_start_cannot_replace_a_complete_word(BufferMode::Windows);
    }

    #[test]
    fn adaptive_buffer_pin_clipped_at_window_start_cannot_replace_a_complete_word() {
        pin_clipped_at_window_start_cannot_replace_a_complete_word(BufferMode::Adaptive);
    }

    fn pin_clipped_at_window_start_cannot_replace_a_complete_word(mode: BufferMode) {
        let session = &mode.session("integrator-niesympatyczne");
        let (mut lane, owner, requests) = forensic_lane_in(mode, session, &[]);
        let (second_start, _) = shared_window_region(&requests);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "dość", second_start - 16_000, second_start - 6_000),
                    word_pin(
                        session,
                        "niesympatyczne",
                        second_start - 6_000,
                        second_start + 12_000,
                    ),
                ],
            ),
            12.5,
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    word_pin(session, "sympatyczne", second_start, second_start + 12_000),
                    word_pin(
                        session,
                        "kiedy",
                        second_start + 12_000,
                        second_start + 20_000,
                    ),
                ],
            ),
            12.5,
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        let text = ledger.text_of(&owner).unwrap_or_default().to_string();
        assert!(text.contains("dość niesympatyczne"), "{text}");
        assert!(
            !text.split_whitespace().any(|word| word == "sympatyczne"),
            "{text}"
        );
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .all(|alternative| !alternative.candidate.contains("sympatyczne")),
            "out-of-band clipped context cannot become an adjudication alternative"
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    // Same take: an early, too wide "Dzieją" pin made the later window's
    // "Bym to powiedział. Nie." a SealedReplay; the words were heard and lost.
    #[test]
    fn integrator_wide_early_pin_splits_for_the_words_it_covers() {
        wide_early_pin_splits_for_the_words_it_covers(BufferMode::Windows);
    }

    #[test]
    fn adaptive_buffer_wide_early_pin_splits_for_the_words_it_covers() {
        wide_early_pin_splits_for_the_words_it_covers(BufferMode::Adaptive);
    }

    fn wide_early_pin_splits_for_the_words_it_covers(mode: BufferMode) {
        let session = &mode.session("integrator-wide-pin");
        let mut lane = open(session);
        let (owner, requests) = launch_three_frame_span(&mut lane, "", &[(100_000, 115_000)]);
        let base = 100_000;
        // The split words tile the wide pin and its burst: D5 retires a held
        // word only where its successors cover the speech, and more than one
        // word's 60 ms continuity left uncovered is room for another word.
        let split = vec![
            word_pin(session, "Bym", base, base + 3_000),
            word_pin(session, "to", base + 3_000, base + 5_000),
            word_pin(session, "powiedział.", base + 5_000, base + 9_000),
            word_pin(session, "Nie.", base + 9_000, base + 11_000),
            word_pin(session, "Dzieją", base + 11_000, base + 15_000),
        ];
        for (request, words) in requests.iter().zip([
            vec![word_pin(session, "Dzieją", base, base + 15_000)],
            split.clone(),
            split,
        ]) {
            let returned = completion(request, words);
            returned.payload.as_ref().unwrap().validate().unwrap();
            lane.state.complete_whisper_window(&lane.tx, returned, 15.0);
        }
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        let text = ledger.text_of(&owner).unwrap_or_default().to_string();
        assert!(text.contains("Bym to powiedział. Nie. Dzieją"), "{text}");
        assert_eq!(
            text.split_whitespace()
                .filter(|word| *word == "Dzieją")
                .count(),
            1,
            "{text}"
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    fn request_geometry(requests: &[TailPatchRequest]) -> Vec<(u64, u64, u64, u64)> {
        requests
            .iter()
            .map(|request| {
                let range = &request.provider_request.identity.range;
                (
                    range.sample_start,
                    range.sample_end,
                    request.admit_sample_start,
                    request.admit_sample_end,
                )
            })
            .collect()
    }

    // Both modes name one session. The capture clock is one 9s window / 3s stride.
    #[test]
    fn buffer_modes_launch_requests_whose_admission_partitions_the_occurrence() {
        let total = 200_000;
        let windows = [(0, 144_000), (48_000, 192_000), (96_000, total)];
        for mode in BufferMode::BOTH {
            let session = &mode.session("buffer-geometry");
            let mut lane = open(session);
            let owner = OccurrenceIdentity::new(session, 1, 0, total);
            let pcm = vec![0.2_f32; total as usize];
            lane.state.audio.push(&pcm);
            lane.state.capture_stopping = true;
            lane.state.pump_capture_windows(&lane.tx);
            let requests = take_requests(&mut lane.tail_rx);
            let geometry = request_geometry(&requests);
            assert_eq!(requests.len(), windows.len(), "{mode:?}: {geometry:?}");
            for (request, expected) in requests.iter().zip(windows) {
                let frame = &request.provider_request.identity.range;
                assert_eq!((frame.sample_start, frame.sample_end), expected, "{mode:?}");
                assert_eq!(
                    (request.admit_sample_start, request.admit_sample_end),
                    expected,
                    "{mode:?}"
                );
                assert_eq!(
                    request.audio,
                    pcm[frame.sample_start as usize..frame.sample_end as usize],
                    "{mode:?}"
                );
            }
            assert_eq!(geometry[0].2, owner.sample_start, "{mode:?}: {geometry:?}");
            assert_eq!(
                geometry[geometry.len() - 1].3,
                owner.sample_end,
                "{mode:?}: {geometry:?}"
            );
            for &(start, end, admit_start, admit_end) in &geometry {
                assert!(
                    start <= admit_start && admit_start < admit_end && admit_end <= end,
                    "{mode:?}: admission inside the decoded range: {geometry:?}"
                );
            }
            for pair in geometry.windows(2) {
                let (_, previous_end, _, _) = pair[0];
                let (next_start, _, _, _) = pair[1];
                assert_eq!(
                    previous_end - next_start,
                    96_000,
                    "{mode:?}: consecutive capture windows share six seconds: {geometry:?}"
                );
            }
            for request in &requests {
                let job = lane
                    .state
                    .refinement_submitted
                    .values()
                    .find(|job| job.request_identity == request.provider_request.identity)
                    .expect("every emitted request is a launched job");
                assert_eq!(
                    (job.admit_sample_start, job.admit_sample_end),
                    (request.admit_sample_start, request.admit_sample_end),
                    "{mode:?}"
                );
                assert_eq!(
                    job.request_identity.range.sample_start,
                    request.provider_request.identity.range.sample_start,
                    "{mode:?}: the decode start completion hands the ledger"
                );
            }
        }
    }

    // The second both decodes share: a word both report on the same PCM is held
    // once, and a real repetition inside that second stays twice.
    #[test]
    fn buffer_modes_hold_a_shared_region_word_once_and_keep_real_repetitions() {
        // Second window start on the 9 s / 3 s capture grid at 16 kHz.
        const SHARED_BASE: u64 = 48_000;
        for mode in BufferMode::BOTH {
            let session = &mode.session("buffer-overlap");
            // Each spoken word is its own voiced burst, with room tone between.
            let voiced = [
                (SHARED_BASE - 10_000, SHARED_BASE - 2_000),
                (SHARED_BASE + 1_000, SHARED_BASE + 4_000),
                (SHARED_BASE + 5_000, SHARED_BASE + 8_000),
                (SHARED_BASE + 20_000, SHARED_BASE + 30_000),
            ];
            let (mut lane, owner, requests) = forensic_lane_voiced(mode, session, &[], &voiced);
            let (base, first_end) = shared_window_region(&requests);
            assert_eq!(base, SHARED_BASE, "{mode:?}: the grid's second window");
            assert!(base + 8_000 < first_end, "{mode:?}: no window-edge stub");
            let shared = |text: &str, start: u64, end: u64| {
                word_pin(session, text, base + start, base + end)
            };
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(session, "wcześniej", base - 10_000, base - 2_000),
                        shared("raz", 1_000, 4_000),
                        shared("raz", 5_000, 8_000),
                        shared("dwa", 20_000, 30_000),
                    ],
                ),
                12.5,
            );
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[1],
                    vec![
                        shared("raz", 1_000, 4_000),
                        shared("raz", 5_000, 8_000),
                        shared("dwa", 20_000, 30_000),
                    ],
                ),
                12.5,
            );
            let text = held_text(&lane, &owner).unwrap_or_default();
            assert_eq!(
                text.split_whitespace()
                    .filter(|word| *word == "raz")
                    .count(),
                2,
                "{mode:?}: {text}"
            );
            assert!(text.contains("wcześniej raz raz dwa"), "{mode:?}: {text}");
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.word_deletions().is_empty(), "{mode:?}");
            assert_eq!(ledger.conservation().residue(), 0, "{mode:?}");
        }
    }

    fn long_word_windows(session: &str) -> [Vec<TimedTailSegment>; 2] {
        [
            vec![
                word_pin(session, "raz", 50_000, 60_000),
                word_pin(session, "krawedz", 60_000, 66_000),
                word_pin(session, "echo", 66_000, 78_000),
                word_pin(session, "trzy", 80_000, 98_000),
            ],
            vec![
                word_pin(session, "raz", 50_000, 60_000),
                word_pin(session, "krawedz", 60_000, 66_000),
                word_pin(session, "dwa", 66_000, 78_000),
                word_pin(session, "trzy", 80_000, 98_000),
                word_pin(session, "cztery", 100_000, 140_000),
            ],
        ]
    }

    fn play_long_words(
        lane: &mut Lane,
        requests: &[TailPatchRequest],
        session: &str,
    ) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(long_word_windows(session)) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 9.5);
            events.extend(drain(&mut lane.rx));
        }
        events
    }

    fn play_seam_word(
        first_pin: &str,
        second_copy: (u64, u64),
        session: &str,
    ) -> (Lane, OccurrenceIdentity, Vec<EngineEvent>) {
        let mut lane = open(session);
        let (occurrence, requests) = launch_three_frame_span(
            &mut lane,
            "apple",
            &[(52_000, 60_000), (70_000, 88_000), (100_000, 140_000)],
        );
        let windows = [
            vec![
                word_pin(session, first_pin, 52_000, 58_000),
                word_pin(session, "dalej", 70_000, 88_000),
            ],
            vec![
                word_pin(session, "szew", second_copy.0, second_copy.1),
                word_pin(session, "koniec", 100_000, 140_000),
            ],
            vec![word_pin(session, "koniec", 100_000, 140_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            let completed = completion(request, segments);
            assert!(completed.payload.as_ref().unwrap().validate().is_ok());
            lane.state
                .complete_whisper_window(&lane.tx, completed, 15.0);
            events.extend(drain(&mut lane.rx));
        }
        (lane, occurrence, events)
    }

    #[test]
    fn seam_word_midpoint_in_first_window_is_admitted_once() {
        let (lane, occurrence, events) = play_seam_word("szew", (52_000, 58_000), "seam-first");
        let warnings = warning_lines(&events);
        assert!(!replay_refusal(&events, "szew"), "{warnings}");
        assert!(
            !warnings.contains("ledger=overlap_without_word_pins"),
            "{warnings}"
        );
        assert_eq!(mutation_count(&events), 2);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("szew dalej koniec")
        );
        assert_conserved(&lane, None);
    }

    #[test]
    fn seam_word_with_jitter_across_midpoints_is_admitted_once() {
        let (lane, occurrence, events) = play_seam_word("szew", (53_000, 60_000), "seam-jitter");
        assert!(!replay_refusal(&events, "szew"));
        assert_eq!(mutation_count(&events), 2);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("szew dalej koniec")
        );
        assert_conserved(&lane, None);
    }

    /// Both copies are inside the actual decoder overlap; normalized spelling
    /// is one acoustic word, while a complete later surface may replace it.
    #[test]
    fn seam_word_with_case_and_punctuation_change_is_admitted_once() {
        let session = "seam-normalized-word";
        let mut lane = open(session);
        let (occurrence, requests) = launch_three_frame_span(
            &mut lane,
            "apple",
            &[(52_000, 58_000), (70_000, 88_000), (100_000, 140_000)],
        );
        let windows = [
            vec![
                word_pin(session, "Szew,", 52_000, 58_000),
                word_pin(session, "dalej", 70_000, 88_000),
            ],
            vec![
                word_pin(session, "szew", 52_000, 58_000),
                word_pin(session, "koniec", 100_000, 140_000),
            ],
            vec![word_pin(session, "koniec", 100_000, 140_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            assert!(segments.iter().all(
                |word| request.provider_request.identity.range.sample_start
                    <= word.range.sample_start
                    && word.range.sample_end <= request.provider_request.identity.range.sample_end
            ));
            let completed = completion(request, segments);
            assert!(completed.payload.as_ref().unwrap().validate().is_ok());
            lane.state
                .complete_whisper_window(&lane.tx, completed, 15.0);
            events.extend(drain(&mut lane.rx));
        }
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .slots_of(&occurrence)
                .unwrap()
                .iter()
                .any(|slot| slot.text == "Szew,"
                    && slot.sample_start == 52_000
                    && slot.sample_end == 58_000)
        );
        drop(ledger);
        let warnings = warning_lines(&events);
        assert!(!replay_refusal(&events, "szew"), "{warnings}");
        assert_eq!(mutation_count(&events), 2);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("Szew, dalej koniec"),
            "{warnings}"
        );
    }

    #[test]
    fn overlapping_distinct_short_words_keep_pcm_order() {
        let session = "seam-distinct-short-words";
        let mut lane = open(session);
        let member = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 96_000);
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 1,
                sample_start: 48_000,
                sample_end: 96_000,
            },
            &[(1, member)],
            &[
                word_pin(session, "w", 60_000, 64_000),
                word_pin(session, "i", 62_000, 65_000),
            ],
            LedgerObservationProducer::Whisper,
        );
        let events = drain(&mut lane.rx);
        assert_eq!(routes[0].exclusive.len(), 2, "{}", warning_lines(&events));
        assert_eq!(exclusive_label(&routes[0].exclusive), "w i");
        assert!(!replay_refusal(&events, "w"));
        assert!(!replay_refusal(&events, "i"));
    }

    /// Shared play for the replayed-seam-copy scenarios: the second copy of
    /// the seam word reaches 2_000 samples past the kept copy, over voiced
    /// audio. Both acceptance tests below assert on this exact take.
    fn play_replayed_seam_copy(session: &str) -> (Lane, OccurrenceIdentity, Vec<EngineEvent>) {
        let mut lane = open(session);
        let (occurrence, requests) = launch_three_frame_span(
            &mut lane,
            "apple",
            &[(52_000, 60_000), (70_000, 88_000), (100_000, 140_000)],
        );
        let windows = [
            vec![
                word_pin(session, "szew", 52_000, 58_000),
                word_pin(session, "dalej", 70_000, 88_000),
            ],
            vec![
                word_pin(session, "szew", 53_000, 60_000),
                word_pin(session, "koniec", 100_000, 140_000),
            ],
            vec![word_pin(session, "koniec", 100_000, 140_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            let completed = completion(request, segments);
            assert!(completed.payload.as_ref().unwrap().validate().is_ok());
            lane.state
                .complete_whisper_window(&lane.tx, completed, 15.0);
            events.extend(drain(&mut lane.rx));
        }
        (lane, occurrence, events)
    }

    /// W0 §10 defect, kept visible until T-C decides the coverage contract.
    /// The later copy of the same seam word reaches 2_000 samples past the kept
    /// copy, over voiced audio. Only exclusive slices count toward hop coverage
    /// today, so those samples leave the long occurrence `sliced` and it loses
    /// its whole Whisper text. Un-ignoring this test is a T-C acceptance criterion.
    #[test]
    fn replayed_seam_copy_past_the_kept_word_keeps_the_whisper_text() {
        let (lane, occurrence, events) = play_replayed_seam_copy("seam-jitter-coverage");
        let warnings = warning_lines(&events);
        assert!(!replay_refusal(&events, "szew"), "{warnings}");
        assert_eq!(mutation_count(&events), 2, "{warnings}");
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("szew dalej koniec"),
            "{warnings}"
        );
    }

    #[test]
    fn slot_replay_never_duplicates_the_seam_word_in_document_text() {
        let (lane, occurrence, _events) = play_replayed_seam_copy("seam-replay-text-once");
        let held = held_text(&lane, &occurrence).expect("Whisper replacement");
        assert_eq!(
            held.split_whitespace()
                .filter(|word| *word == "szew")
                .count(),
            1
        );
    }

    /// `szyk` is an ExclusiveTail under `classify_overlap_pin`: its midpoint
    /// belongs to window 2 and its range fits the open member. Text differs,
    /// so T-A2 cannot turn it into a same-word replay.
    #[test]
    fn distinct_seam_word_is_admitted_without_coverage_gate() {
        let session = "seam-distinct-coverage";
        let mut lane = open(session);
        let (occurrence, requests) = launch_three_frame_span(
            &mut lane,
            "apple",
            &[(44_000, 55_000), (70_000, 88_000), (100_000, 140_000)],
        );
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![
                    word_pin(session, "szew", 44_000, 51_000),
                    word_pin(session, "szyk", 52_000, 55_000),
                ],
            ),
            9.5,
        );
        let _ = drain(&mut lane.rx);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![word_pin(session, "szyk", 52_000, 55_000)],
            ),
            9.5,
        );
        let events = drain(&mut lane.rx);
        assert!(!replay_refusal(&events, "szyk"));
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .slots_of(&occurrence)
                .unwrap()
                .iter()
                .any(|slot| slot.text == "szyk")
        );
        ledger.assert_slot_labels();
    }

    // Root-owned stop proof: real retained PCM/request ranges, supplied Words.
    // A timeout closes obligations, never successful decoder work.
    #[test]
    fn forensic_l44_cancel_retains_word_and_unreturned_source_debt() {
        for mode in BufferMode::BOTH {
            let session = mode.session("l44-cancel");
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 0, LONG_SAMPLES, &[(44_000, 51_000)]);
            assert!(requests.len() >= 2);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![word_pin(&session, "szew", 44_000, 51_000)],
                ),
                9.5,
            );
            let before = lane
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .to_vec();
            assert_eq!(held_text(&lane, &owner).as_deref(), Some("szew"));
            let _ = drain(&mut lane.rx);
            lane.state
                .return_outstanding_whisper_without_label(&lane.tx);
            let events = drain(&mut lane.rx);
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("szew"));
            assert_eq!(ledger.slots_of(&owner).unwrap(), before);
            assert!(ledger.text_recovery_pending(&owner));
            assert!(!ledger.is_sealed(&owner));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            );
            assert!(ledger.word_deletions().is_empty());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    fn forensic_l44_complete_blank_windows_case(missing_speech: bool) {
        for mode in BufferMode::BOTH {
            let session = mode.session(if missing_speech {
                "l44-blank-voiced"
            } else {
                "l44-blank-quiet"
            });
            let mut voiced = vec![(44_000, 51_000)];
            if missing_speech {
                voiced.push((160_000, 168_000));
            }
            let (mut lane, owner, requests, _) = forensic_boundary_capture(
                &session,
                0,
                if missing_speech {
                    200_000
                } else {
                    LONG_SAMPLES
                },
                &voiced,
            );
            assert!(requests.len() >= 2);
            let speech = coverage_speech_evidence(&lane.state);
            assert!(requests[0].provider_request.identity.range.sample_end < 160_000);
            assert!(
                speech
                    .ranges()
                    .iter()
                    .any(|range| range.sample_start < 51_000 && range.sample_end > 44_000)
            );
            if missing_speech {
                assert!(
                    speech
                        .ranges()
                        .iter()
                        .any(|range| range.sample_start < 168_000 && range.sample_end > 160_000)
                );
            }
            if !missing_speech {
                assert!(speech.ranges().iter().all(|range| range.sample_end
                    < requests[0].provider_request.identity.range.sample_end));
            }
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![word_pin(&session, "szew", 44_000, 51_000)],
                ),
                9.5,
            );
            assert_eq!(held_text(&lane, &owner).as_deref(), Some("szew"));
            let source_slots = lane
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .to_vec();
            let mut events = drain(&mut lane.rx);
            for request in &requests[1..] {
                // Every final carries the issued identity and exact PCM range.
                // No invented Word pin stands in for a successful empty decode.
                lane.state
                    .complete_whisper_window(&lane.tx, completion(request, vec![]), 9.5);
                events.extend(drain(&mut lane.rx));
            }
            assert_eq!(lane.state.tail_patch_awaiting_completion(), 0);
            lane.state
                .return_outstanding_whisper_without_label(&lane.tx);
            events.extend(drain(&mut lane.rx));
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("szew"));
            assert_eq!(ledger.slots_of(&owner).unwrap(), source_slots);
            assert!(ledger.word_deletions().is_empty());
            eprintln!(
                "L44 {mode:?} missing={missing_speech} pending={} sealed={} speech={:?} warnings={}",
                ledger.text_recovery_pending(&owner),
                ledger.is_sealed(&owner),
                speech.ranges(),
                warning_lines(&events)
            );
            assert_eq!(ledger.text_recovery_pending(&owner), missing_speech);
            assert_eq!(ledger.is_sealed(&owner), !missing_speech);
            if missing_speech {
                assert!(
                    !events
                        .iter()
                        .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                );
            }
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_l44_blank_final_on_measured_quiet_accounts_work_without_word() {
        forensic_l44_complete_blank_windows_case(false);
    }

    #[test]
    fn forensic_l44_blank_final_over_untranscribed_speech_cannot_seal() {
        forensic_l44_complete_blank_windows_case(true);
    }

    fn forensic_l45_near_decode_fence_case(continues: bool) {
        for mode in BufferMode::BOTH {
            let (_, _, clock) = forensic_lane_in(mode, "l45-clock", &[]);
            let end = clock[0].provider_request.identity.range.sample_end;
            let session = mode.session(if continues {
                "l45-fence-voiced"
            } else {
                "l45-fence-quiet"
            });
            let voiced_end = if continues { end + 1_500 } else { end - 640 };
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 0, 200_000, &[(end - 3_500, voiced_end)]);
            assert!(requests.len() >= 2);
            assert_eq!(requests[0].provider_request.identity.range.sample_end, end);
            assert!(requests[1].provider_request.identity.range.sample_start <= end - 4_000);
            // Word time quantization: it stops 8 ms before the PCM fence at 16 kHz.
            // The continuing-speech case is still cut mid-word in the fixture.
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![word_pin(&session, "Iwo", end - 4_000, end - 128)],
                ),
                12.5,
            );
            let first_events = drain(&mut lane.rx);
            eprintln!(
                "L45 {mode:?} continues={continues} decode_end={end} raw={:?} stubs={} warnings={}",
                held_text(&lane, &owner),
                lane.state.pending_whisper_stubs.len(),
                warning_lines(&first_events)
            );
            assert_eq!(
                held_text(&lane, &owner),
                None,
                "a right-context word waits for its owning window, with or without quiet margin"
            );
            assert!(!unanchored_label(&first_events, "Iwo"));
            assert!(lane.state.pending_whisper_stubs.is_empty());
            {
                let complete_end = if continues { end + 2_000 } else { end - 128 };
                lane.state.complete_whisper_window(
                    &lane.tx,
                    completion(
                        &requests[1],
                        vec![word_pin(&session, "Iwo", end - 4_000, complete_end)],
                    ),
                    12.5,
                );
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert_eq!(ledger.text_of(&owner), Some("Iwo"));
                let words = ledger.slots_of(&owner).unwrap();
                assert_eq!(words.len(), 1);
                assert_eq!(
                    (words[0].sample_start, words[0].sample_end),
                    (end - 4_000, complete_end)
                );
                assert!(lane.state.pending_whisper_stubs.is_empty());
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_l45_quantized_edge_over_continuing_speech_waits_for_whole_word() {
        forensic_l45_near_decode_fence_case(true);
    }

    #[test]
    fn forensic_l45_quantized_edge_with_quiet_margin_commits_without_delay() {
        forensic_l45_near_decode_fence_case(false);
    }

    #[test]
    fn forensic_l47_invalid_empty_returns_do_not_lend_decoded_scope() {
        for mode in BufferMode::BOTH {
            for invalid in [
                "missing_payload",
                "foreign_payload_identity",
                "wrong_engine_family",
            ] {
                let session = mode.session(&format!("l47-{invalid}"));
                let (mut lane, owner, requests, _) =
                    forensic_boundary_capture(&session, 0, LONG_SAMPLES, &[(44_000, 51_000)]);
                assert!(requests.len() >= 2);
                lane.state.complete_whisper_window(
                    &lane.tx,
                    completion(
                        &requests[0],
                        vec![word_pin(&session, "szew", 44_000, 51_000)],
                    ),
                    9.5,
                );
                assert_eq!(held_text(&lane, &owner).as_deref(), Some("szew"));
                let before = lane
                    .state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .slots_of(&owner)
                    .unwrap()
                    .to_vec();
                let _ = drain(&mut lane.rx);
                for request in &requests[1..] {
                    let mut reply = completion(request, vec![]);
                    match invalid {
                        "missing_payload" => reply.payload = None,
                        "foreign_payload_identity" => {
                            reply.payload.as_mut().unwrap().identity.range.session =
                                "foreign".into()
                        }
                        "wrong_engine_family" => {
                            reply.payload.as_mut().unwrap().evidence.source =
                                TailEvidenceSource::AppleSpeech
                        }
                        _ => unreachable!(),
                    }
                    lane.state.complete_whisper_window(&lane.tx, reply, 9.5);
                }
                lane.state
                    .return_outstanding_whisper_without_label(&lane.tx);
                let events = drain(&mut lane.rx);
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert_eq!(ledger.text_of(&owner), Some("szew"), "{mode:?}: {invalid}");
                assert_eq!(ledger.slots_of(&owner).unwrap(), before);
                assert!(ledger.text_recovery_pending(&owner), "{mode:?}: {invalid}");
                assert!(!ledger.is_sealed(&owner), "{mode:?}: {invalid}");
                assert!(
                    !events
                        .iter()
                        .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                );
                assert!(ledger.word_deletions().is_empty());
                ledger.assert_slot_labels();
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    fn forensic_l50_reverse_blank_windows_case(missing_speech: bool) {
        for mode in BufferMode::BOTH {
            let session = mode.session(if missing_speech {
                "l50-reverse-voiced"
            } else {
                "l50-reverse-quiet"
            });
            let mut voiced = vec![(44_000, 51_000)];
            if missing_speech {
                voiced.push((160_000, 168_000));
            }
            let (mut lane, owner, requests, _) = forensic_boundary_capture(
                &session,
                0,
                if missing_speech {
                    200_000
                } else {
                    LONG_SAMPLES
                },
                &voiced,
            );
            assert!(requests.len() >= 2);
            let speech = coverage_speech_evidence(&lane.state);
            assert!(requests[0].provider_request.identity.range.sample_end < 160_000);
            assert!(
                speech
                    .ranges()
                    .iter()
                    .any(|range| range.sample_start < 51_000 && range.sample_end > 44_000)
            );
            if missing_speech {
                assert!(
                    speech
                        .ranges()
                        .iter()
                        .any(|range| range.sample_start < 168_000 && range.sample_end > 160_000)
                );
            }
            if !missing_speech {
                assert!(speech.ranges().iter().all(|range| range.sample_end
                    < requests[0].provider_request.identity.range.sample_end));
            }
            let mut events = drain(&mut lane.rx);
            for request in &requests[1..] {
                // Every final carries the issued identity and exact PCM range.
                // No invented Word pin stands in for a successful empty decode.
                lane.state
                    .complete_whisper_window(&lane.tx, completion(request, vec![]), 9.5);
                events.extend(drain(&mut lane.rx));
            }
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![word_pin(&session, "szew", 44_000, 51_000)],
                ),
                9.5,
            );
            assert_eq!(held_text(&lane, &owner).as_deref(), Some("szew"));
            let source_slots = lane
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .to_vec();
            events.extend(drain(&mut lane.rx));
            assert_eq!(lane.state.tail_patch_awaiting_completion(), 0);
            lane.state
                .return_outstanding_whisper_without_label(&lane.tx);
            events.extend(drain(&mut lane.rx));
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("szew"));
            assert_eq!(ledger.slots_of(&owner).unwrap(), source_slots);
            assert!(ledger.word_deletions().is_empty());
            eprintln!(
                "L50 {mode:?} missing={missing_speech} pending={} sealed={} speech={:?} warnings={}",
                ledger.text_recovery_pending(&owner),
                ledger.is_sealed(&owner),
                speech.ranges(),
                warning_lines(&events)
            );
            assert_eq!(ledger.text_recovery_pending(&owner), missing_speech);
            assert_eq!(ledger.is_sealed(&owner), !missing_speech);
            if missing_speech {
                assert!(
                    !events
                        .iter()
                        .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                );
            }
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_l50_empty_returns_before_words_still_settle_accounted_quiet() {
        forensic_l50_reverse_blank_windows_case(false);
    }

    #[test]
    fn forensic_l50_empty_returns_before_words_keep_untranscribed_voice_debt() {
        forensic_l50_reverse_blank_windows_case(true);
    }

    #[test]
    fn forensic_l54_accepted_prefix_cannot_launder_an_unfinished_edge_word() {
        for mode in BufferMode::BOTH {
            let (_, _, clock) = forensic_lane_in(mode, "l54-clock", &[]);
            let end = clock[0].provider_request.identity.range.sample_end;
            let session = mode.session("l54-prefix-edge");
            let (mut lane, owner, requests, _) = forensic_boundary_capture(
                &session,
                0,
                200_000,
                &[(80_000, 82_000), (end - 3_500, end + 1_500)],
            );
            assert_eq!(requests[0].provider_request.identity.range.sample_end, end);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(&session, "tak", 80_000, 82_000),
                        word_pin(&session, "Iwo", end - 4_000, end - 128),
                    ],
                ),
                12.5,
            );
            assert_eq!(held_text(&lane, &owner).as_deref(), Some("tak"));
            let source = lane
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&owner)
                .unwrap()
                .to_vec();
            assert!(
                !unanchored_label(&drain(&mut lane.rx), "Iwo"),
                "unowned context is not a staged word"
            );
            lane.state
                .return_outstanding_whisper_without_label(&lane.tx);
            let events = drain(&mut lane.rx);
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&owner), Some("tak"));
            assert_eq!(ledger.slots_of(&owner).unwrap(), source);
            assert!(ledger.text_recovery_pending(&owner));
            assert!(!ledger.is_sealed(&owner));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, EngineEvent::LedgerSeal { .. }))
            );
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
            ledger.assert_slot_labels();
        }
    }

    fn forensic_l65_invalid_capture_fence_case(mode: BufferMode, invalid_position: usize) {
        let session = &mode.session("l65-invalid-capture-fence");
        let decoded = true;
        let voiced_fence = true;
        let mut lane = open(session);
        let owner = OccurrenceIdentity::new(session, 1, 0, 48_000);
        let speech_end = 16_000;
        let mut pcm = vec![0.0_f32; 48_000];
        pcm[6_000..speech_end as usize].fill(0.2);
        pcm[invalid_position] = f32::NAN;
        record_energy(
            &lane,
            &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, 48_000, Some((6_000, speech_end)));
        stage(&mut lane, 1, owner.clone(), "hipoteza");
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .require_text_recovery(&owner)
        );
        lane.state.audio.push(&pcm);
        assert_eq!(lane.state.audio.session_sample_end(), pcm.len() as u64);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let mut requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert_eq!(request.audio.len(), pcm.len());
        assert!(
            request
                .audio
                .iter()
                .zip(&pcm)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        let range = &request.provider_request.identity.range;
        assert_eq!((range.sample_start, range.sample_end), (0, 48_000));
        if decoded {
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(&request, vec![word_pin(session, "Iwo", 4_000, 48_000)]),
                3.0,
            );
            let returned_events = drain(&mut lane.rx);
            if voiced_fence {
                assert!(unanchored_label(&returned_events, "Iwo"));
                assert_eq!(held_text(&lane, &owner).as_deref(), Some("hipoteza"));
            } else {
                assert_eq!(
                    held_text(&lane, &owner).as_deref(),
                    Some("Iwo"),
                    "positive measured quiet must admit now, before terminal frontier drain"
                );
                assert!(!unanchored_label(&returned_events, "Iwo"));
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert!(!ledger.text_recovery_pending(&owner));
                assert!(ledger.word_deletions().is_empty());
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        assert_eq!(lane.state.tail_patch_awaiting_completion(), 0);
        assert!(lane.state.refinement_pending.is_empty());
        {
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.frontier_of(&owner).unwrap().open_producers(),
                vec![ObservationProducer::Lexicon]
            );
            assert!(
                !ledger.is_sealed(&owner),
                "actual scheduled L2 has not returned"
            );
        }
        let label = if decoded { "Iwo" } else { "hipoteza" };
        close_lexicon(&mut lane, 1, &owner, label);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .frontier_of(&owner)
                .unwrap()
                .open_producers()
                .is_empty()
        );
        if decoded && !voiced_fence {
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 1);
            assert_eq!(slots[0].producer, ObservationProducer::Whisper);
            assert_eq!(
                (slots[0].sample_start, slots[0].sample_end),
                (4_000, 48_000)
            );
            assert!(!ledger.text_recovery_pending(&owner));
            assert!(ledger.is_sealed(&owner));
            drop(ledger);
            assert!(lane.state.stop_document_settled());
        } else {
            assert!(
                ledger.text_recovery_pending(&owner),
                "drained queue or speech-cut fence cannot certify decoder work"
            );
            assert!(!ledger.is_sealed(&owner));
            drop(ledger);
            assert!(!lane.state.stop_document_settled());
        }
    }

    #[test]
    fn forensic_l65_invalid_capture_never_becomes_quiet_fence() {
        for mode in BufferMode::BOTH {
            // Invalid PCM both inside and outside any narrow edge query:
            // downstream silence cannot overrule the original capture writer.
            for invalid_position in [1_000, 47_990] {
                forensic_l65_invalid_capture_fence_case(mode, invalid_position);
            }
        }
    }
    #[test]
    fn stop_keeps_already_admitted_word_slots() {
        let session = "seam-stop-slots";
        let mut lane = open(session);
        // Only the represented word is voiced; the capture proves the rest silent.
        let mut pcm = vec![0.0_f32; LONG_SAMPLES as usize];
        pcm[44_000..51_000].fill(0.2);
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, LONG_SAMPLES, Some((44_000, 51_000)));
        let owner = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        qualify_unlabelled(&mut lane, &owner);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| (
                    request.provider_request.identity.range.sample_start,
                    request.provider_request.identity.range.sample_end,
                    request.admit_sample_start,
                    request.admit_sample_end,
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, 144_000, 0, 144_000),
                (48_000, LONG_SAMPLES, 48_000, LONG_SAMPLES)
            ]
        );
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "szew", 44_000, 51_000)],
            ),
            9.5,
        );
        assert_eq!(
            lane.state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("szew"),
            "the word must be committed before stop can preserve it"
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("szew"));
        assert!(
            !ledger.is_sealed(&owner),
            "unfinished decoder jobs do not certify the whole take"
        );
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn private_stop_retains_words_without_certifying_uncovered_speech() {
        let session = "seam-stop-slots";
        let mut lane = open(session);
        // The entire capture is voiced; one returned word cannot certify its missing speech.
        let pcm = vec![0.2_f32; LONG_SAMPLES as usize];
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, LONG_SAMPLES, Some((0, LONG_SAMPLES)));
        let owner = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        qualify_unlabelled(&mut lane, &owner);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| (
                    request.provider_request.identity.range.sample_start,
                    request.provider_request.identity.range.sample_end,
                    request.admit_sample_start,
                    request.admit_sample_end,
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, 144_000, 0, 144_000),
                (48_000, LONG_SAMPLES, 48_000, LONG_SAMPLES)
            ]
        );
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "szew", 44_000, 51_000)],
            ),
            9.5,
        );
        assert_eq!(
            lane.state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("szew"),
            "the word must be committed before stop can preserve it"
        );
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("szew"));
        assert!(
            !ledger.is_sealed(&owner),
            "returning unfinished work must not certify uncovered speech"
        );
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn private_complete_window_set_keeps_admitted_word_and_seals_quiet_rest() {
        let session = "seam-stop-slots";
        let mut lane = open(session);
        // Only the represented word is voiced; the capture proves the rest
        // silent with measured room tone, not a voice gate's exact zeros.
        let mut pcm = vec![ROOM_TONE_RMS; LONG_SAMPLES as usize];
        pcm[60_000..80_000].fill(0.2);
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        record_silero(&mut lane, LONG_SAMPLES, Some((60_000, 80_000)));
        let owner = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        qualify_unlabelled(&mut lane, &owner);
        assert!(lane.state.register_whisper_owner(&lane.tx, 1, &owner));
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| (
                    request.provider_request.identity.range.sample_start,
                    request.provider_request.identity.range.sample_end,
                    request.admit_sample_start,
                    request.admit_sample_end,
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, 144_000, 0, 144_000),
                (48_000, LONG_SAMPLES, 48_000, LONG_SAMPLES)
            ]
        );
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![word_pin(session, "szew", 60_000, 80_000)],
            ),
            9.5,
        );
        assert_eq!(
            lane.state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("szew"),
            "the word must be committed before stop can preserve it"
        );
        // Repeated pins describe the same physical word heard in left context.
        // These are actual successful completions, not cancellation receipts.
        for request in requests.iter().skip(1) {
            let frame = &request.provider_request.identity.range;
            assert!(frame.sample_start <= 60_000 && frame.sample_end >= 80_000);
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(request, vec![word_pin(session, "szew", 60_000, 80_000)]),
                9.5,
            );
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        let coverage = reconcile_terminal_coverage(&mut lane.state, &lane.tx);
        assert_eq!(
            coverage.status,
            crate::pipeline::acoustic_ledger::SealCoverageStatus::Complete,
            "{coverage:#?}"
        );
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("szew"));
        assert!(
            ledger.is_sealed(&owner),
            "trail={:#?}",
            ledger.layer_trail()
        );
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }

    #[test]
    fn cloud_word_context_guard_emits_one_matching_live_receipt() {
        for (decode, accepted) in [((48_000, 192_000), false), ((24_000, 168_000), true)] {
            let session = "cloud-word-context-receipt";
            let mut lane = open(session);
            let owner = OccurrenceIdentity::new(session, 1, 0, 160_000);
            let lexicon = tempfile::tempdir().unwrap();
            lane.state.lexicon_custom_path = lexicon.path().join("absent.jsonl");
            let mut pcm = vec![0.0_f32; 192_000];
            pcm[48_000..64_000].fill(0.2);
            record_voiced_capture(&mut lane, 192_000, &[(48_000, 64_000)]);
            *lane.state.acoustic_ledger.lock().unwrap() =
                crate::pipeline::acoustic_ledger::word_adjudication_tests::measured_ledger(
                    &owner, &pcm,
                );
            let pins = [RoutedPin {
                confidence: None,
                index: 0,
                pin: OccurrenceIdentity::new(session, 1, 48_000, 64_000),
                text: "1286".into(),
            }];
            assert!(lane.state.admit_routed_words_with_decode_window(
                &lane.tx,
                1,
                &owner,
                (1, Some((0, 144_000))),
                RoutedWords {
                    pins: &pins,
                    neighbours: &[]
                },
                LedgerObservationProducer::CloudLive,
            ));
            assert_eq!(
                lane.state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .slots_of(&owner)
                    .unwrap()[0]
                    .producer,
                ObservationProducer::CloudLive
            );
            drain(&mut lane.rx);
            let candidate = [RoutedPin {
                text: "56".into(),
                ..pins[0].clone()
            }];
            let admitted = lane.state.admit_routed_words_with_decode_window(
                &lane.tx,
                1,
                &owner,
                (2, Some(decode)),
                RoutedWords {
                    pins: &candidate,
                    neighbours: &[],
                },
                LedgerObservationProducer::Whisper,
            );
            assert_eq!(admitted, accepted);
            let events = drain(&mut lane.rx);
            // The batch has one receipt; refused individual pins also carry
            // child receipts with their own physical ranges.
            let mutations = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::LedgerMutation {
                        observation,
                        label,
                        receipt,
                    } if observation.producer == ObservationProducer::Whisper
                        && observation.occurrence == owner =>
                    {
                        Some((label.clone(), receipt.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                mutations.len(),
                1,
                "one owner admission must emit one batch receipt"
            );
            assert_eq!(mutations[0].1.grants_mutation(), accepted);
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.text_of(&owner),
                Some(if accepted { "56" } else { "1286" })
            );
            assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
            assert_eq!(ledger.conservation().residue(), 0);
            if accepted {
                assert_eq!(mutations[0].0, "56");
            }
        }
    }

    #[test]
    fn overlap_routing_preserves_native_pin_for_ledger_identity_adjudication() {
        let session = "seam-clipped-slots";
        let mut lane = open(session);
        let pcm = vec![0.2_f32; 96_000];
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let owner = OccurrenceIdentity::new(session, 1, 0, 52_000);
        stage(&mut lane, 1, owner.clone(), "apple");
        let admitted = lane.state.admit_routed_words_with_decode_window(
            &lane.tx,
            1,
            &owner,
            (1, None),
            RoutedWords {
                pins: &[RoutedPin {
                    confidence: None,
                    index: 0,
                    pin: OccurrenceIdentity::new(session, 1, 44_000, 51_000),
                    text: "szew".into(),
                }],
                neighbours: &[],
            },
            LedgerObservationProducer::Whisper,
        );
        assert!(
            admitted,
            "the prior Whisper pin must actually enter the ledger before replay comparison"
        );
        assert_eq!(
            lane.state.acoustic_ledger.lock().unwrap().text_of(&owner),
            Some("szew")
        );
        let _ = drain(&mut lane.rx);
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 2,
                sample_start: 48_000,
                sample_end: 96_000,
            },
            &[(1, owner)],
            &[word_pin(session, "szew", 45_000, 53_000)],
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert_eq!(
            (
                routes[0].exclusive[0].pin.sample_start,
                routes[0].exclusive[0].pin.sample_end
            ),
            (45_000, 53_000)
        );
        assert!(
            whisper_mutations(&drain(&mut lane.rx)).is_empty(),
            "routing has no authority to decide word identity"
        );
    }

    /// The first pin geometry is copied from live take b2707ce7: the word
    /// starts at 96_000, 768 samples before Apple's 96_768 occurrence edge.
    #[test]
    fn live_member_start_straddle_keeps_every_whisper_word_once() {
        let session = "live-member-edge";
        let mut lane = open(session);
        let occurrence = OccurrenceIdentity::new(session, 1, 96_768, 192_768);
        // The word crosses the owner edge, but the decoder receives its
        // whole original PCM, including the 768-sample leading context.
        let mut pcm = vec![0.0_f32; 192_768];
        for (start, end) in [(96_000, 126_720), (132_000, 152_000), (160_000, 188_000)] {
            pcm[start..end].fill(0.2);
        }
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        // The straddle is measured on an owner Apple left without text. A
        // coarse two-word Apple label over the whole owner keeps its words
        // under D5: neither window's words cover all of its speech.
        lane.state.energy_calibration = Some(EnergyCalibration {
            version: "live-member-measured-fixture".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        });
        assert!(qualify_owned_occurrence(&lane.state, &occurrence));
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .require_text_recovery(&occurrence)
        );
        lane.state.pending_events.insert(
            1,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: String::new(),
                layer1_baseline: String::new(),
                start_ts: 96_768.0 / RATE as f32,
                end_ts: 192_768.0 / RATE as f32,
                segments: Vec::new(),
            },
        );
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 3);
        for request in &requests {
            let frame = &request.provider_request.identity.range;
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
        }
        assert!(requests[0].provider_request.identity.range.sample_start <= 96_000);

        let windows = [
            vec![word_pin(session, "tak", 96_000, 126_720)],
            vec![
                word_pin(session, "tak", 98_624, 126_720),
                word_pin(session, "poza", 132_000, 152_000),
            ],
            vec![
                word_pin(session, "tym", 160_000, 174_000),
                word_pin(session, "to", 174_000, 188_000),
            ],
        ];
        let mut events = Vec::new();
        for (request, words) in requests.iter().zip(windows) {
            assert!(
                words
                    .iter()
                    .all(|word| request.provider_request.identity.range.sample_start
                        <= word.range.sample_start
                        && word.range.sample_end
                            <= request.provider_request.identity.range.sample_end)
            );
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, words), 9.0);
            events.extend(drain(&mut lane.rx));
        }
        let warnings = warning_lines(&events);
        assert_eq!(mutation_count(&events), 2, "{warnings}");
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("tak poza tym to"),
            "{warnings}"
        );
        assert_conserved(&lane, None);
    }

    /// Measured Words replace a coarse Apple hypothesis while preserving its
    /// actual source lineage. Complete speech coverage has its own receipt;
    /// partial decoder scope keeps explicit source debt and cannot publish a seal.
    #[test]
    fn measured_whisper_words_replace_a_coarse_group_only_over_covered_speech() {
        for covered in [true, false] {
            let session = if covered {
                "group-speech-covered"
            } else {
                "group-speech-gap"
            };
            let mut lane = open(session);
            let occurrence = OccurrenceIdentity::new(session, 1, 96_768, 192_768);
            let mut voiced = vec![(96_768, 126_720), (132_000, 152_000)];
            if !covered {
                voiced.push((160_000, 188_000));
            }
            let mut pcm = vec![0.0_f32; 192_768];
            for &(start, end) in &voiced {
                pcm[start as usize..end as usize].fill(0.2);
            }
            lane.state.audio.push(&pcm);
            record_energy(
                &lane,
                &pcm.chunks(1_000).map(<[f32]>::to_vec).collect::<Vec<_>>(),
            );
            stage(&mut lane, 1, occurrence.clone(), "apple floor");
            close_lexicon(&mut lane, 1, &occurrence, "apple floor");
            lane.state.pump_capture_windows(&lane.tx);
            let sources_before = lane
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&occurrence)
                .unwrap()
                .to_vec();
            let _ = drain(&mut lane.rx);
            let requests = take_requests(&mut lane.tail_rx);
            assert!(!requests.is_empty());
            for request in &requests {
                request
                    .provider_request
                    .validate_pcm(&request.audio)
                    .unwrap();
                let range = &request.provider_request.identity.range;
                assert_eq!(
                    request.audio,
                    pcm[range.sample_start as usize..range.sample_end as usize]
                );
            }
            assert!(requests[0].provider_request.identity.range.sample_start <= 96_000);
            let covering = requests
                .iter()
                .find(|request| {
                    let range = &request.provider_request.identity.range;
                    range.sample_start <= 96_000 && range.sample_end >= 152_000
                })
                .expect("a capture window covers tak and poza");
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    covering,
                    vec![
                        word_pin(session, "tak", 96_000, 127_000),
                        word_pin(session, "poza", 132_000, 152_000),
                    ],
                ),
                9.0,
            );
            let received_events = drain(&mut lane.rx);
            let warnings = warning_lines(&received_events);
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            if covered {
                assert_eq!(ledger.text_of(&occurrence), Some("tak poza"), "{warnings}");
                assert_eq!(ledger.group_speech_coverages().len(), 1, "{warnings}");
            } else {
                // D5 (ledger speech truth brief): absence is not evidence. The
                // speech at 160 000..188 000 lies under the coarse Apple group
                // and under no decoded word, so the group keeps its words and
                // the decoded reading waits as an alternative over it.
                assert_eq!(
                    ledger.text_of(&occurrence),
                    Some("apple floor"),
                    "{warnings}"
                );
                assert_eq!(ledger.slots_of(&occurrence).unwrap(), sources_before);
                assert!(ledger.group_speech_coverages().is_empty());
                assert!(ledger.slot_alternatives().iter().any(|alternative| {
                    alternative.reason == "absence_requires_acoustic_evidence"
                        && alternative.sources == sources_before
                }));
                let range = &covering.provider_request.identity.range;
                let full_source_decoded = range.sample_start <= occurrence.sample_start
                    && range.sample_end >= occurrence.sample_end;
                assert_eq!(
                    ledger.text_recovery_pending(&occurrence),
                    !full_source_decoded
                );
                if !full_source_decoded {
                    assert!(ledger.slot_alternatives().iter().any(
                        |alternative| alternative.reason == "partial_group_speech_pending"
                            && alternative.sources == sources_before
                    ));
                    assert!(!ledger.is_sealed(&occurrence));
                    assert!(!received_events.iter().any(|event| matches!(
                        event,
                        EngineEvent::LedgerSeal { .. } | EngineEvent::UtteranceFinal { .. }
                    )));
                }
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn owned_word_retains_original_clock_without_lending_text_to_adjacent_member() {
        let session = "adjacent-word-coverage";
        let mut lane = open(session);
        let first = OccurrenceIdentity::new(session, 1, 0, 64_000);
        let second = OccurrenceIdentity::new(session, 1, 64_000, 96_000);
        record_voiced_spans(
            &lane,
            96_000,
            &[(8_000, 20_000), (62_000, 64_000), (64_000, 76_000)],
        );
        stage(&mut lane, 1, first.clone(), "frst");
        stage(&mut lane, 2, second.clone(), "secnd");
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 3,
                sample_start: 0,
                sample_end: 96_000,
            },
            &[(1, first.clone()), (2, second.clone())],
            &[
                word_pin(session, "first", 8_000, 20_000),
                word_pin(session, "second", 62_000, 76_000),
            ],
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(exclusive_label(&routes[0].exclusive), "first");
        assert_eq!(exclusive_label(&routes[1].exclusive), "second");
        // The route assigns one owner without destroying physical provenance.
        // The two-edge admission witness separately checks bounded final slots.
        assert_eq!(routes[1].exclusive[0].pin.sample_start, 62_000);
        assert_eq!(routes[1].exclusive[0].pin.sample_end, 76_000);
        let receipt = admit_ledger_label(
            &mut lane.state,
            &lane.tx,
            LabelAdmission {
                observation: ObservationIdentity::new(
                    ObservationProducer::Whisper,
                    3,
                    0,
                    first.clone(),
                ),
                label: &exclusive_label(&routes[0].exclusive),
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .expect("first member admission");
        assert!(receipt.grants_mutation());
        assert_eq!(held_text(&lane, &first).as_deref(), Some("first"));
        assert!(!held_text(&lane, &first).unwrap().contains("second"));
        let second_receipt = admit_ledger_label(
            &mut lane.state,
            &lane.tx,
            LabelAdmission {
                observation: ObservationIdentity::new(
                    ObservationProducer::Whisper,
                    3,
                    1,
                    second.clone(),
                ),
                label: &exclusive_label(&routes[1].exclusive),
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .expect("second member admission");
        assert!(second_receipt.grants_mutation());
        assert_eq!(held_text(&lane, &second).as_deref(), Some("second"));
    }

    /// A short Word sharing an earlier source is a split, not acoustic replay.
    /// Preserve the offered split when only one later frame hears it; confirm
    /// the split when two independent later frames cover the full old source.
    #[test]
    fn short_distinct_word_after_the_seam_is_not_a_replay_of_its_neighbour() {
        for corroborated in [false, true] {
            let base = if corroborated { 52_000 } else { 0 };
            let session = "seam-short-word";
            let mut lane = open(session);
            let (occurrence, requests) = launch_three_frame_span(
                &mut lane,
                "apple",
                &[
                    (48_000 + base, 55_000 + base),
                    (70_000 + base, 88_000 + base),
                    (150_000, 180_000),
                ],
            );
            let windows = [
                if corroborated {
                    vec![word_pin(session, "domu", 48_000 + base, 54_000 + base)]
                } else {
                    vec![
                        word_pin(session, "domu", 48_000, 54_000),
                        word_pin(session, "dalej", 70_000, 88_000),
                    ]
                },
                vec![
                    word_pin(session, "domu", 48_500 + base, 52_000 + base),
                    word_pin(session, "w", 52_000 + base, 55_000 + base),
                    word_pin(session, "dalej", 70_000 + base, 88_000 + base),
                ],
                if corroborated {
                    vec![
                        word_pin(session, "domu", 48_500 + base, 52_000 + base),
                        word_pin(session, "w", 52_000 + base, 55_000 + base),
                        word_pin(session, "dalej", 70_000 + base, 88_000 + base),
                        word_pin(session, "koniec", 150_000, 180_000),
                    ]
                } else {
                    vec![word_pin(session, "koniec", 150_000, 180_000)]
                },
            ];
            let mut events = Vec::new();
            for (index, (request, segments)) in requests.iter().zip(windows).enumerate() {
                let completed = completion(request, segments);
                assert!(completed.payload.as_ref().unwrap().validate().is_ok());
                lane.state
                    .complete_whisper_window(&lane.tx, completed, 15.0);
                let returned = drain(&mut lane.rx);
                if index == 1 {
                    assert!(
                        !replay_refusal(&returned, "w"),
                        "the new short Word is not replay"
                    );
                }
                events.extend(returned);
            }
            let warnings = warning_lines(&events);
            if corroborated {
                let ledger = lane.state.acoustic_ledger.lock().unwrap();
                assert_eq!(
                    ledger
                        .slots_of(&occurrence)
                        .unwrap()
                        .iter()
                        .filter(|slot| slot.text == "w")
                        .count(),
                    1
                );
                assert!(
                    ledger
                        .slot_operations()
                        .iter()
                        .flat_map(|operation| &operation.source_ranges)
                        .any(|range| range.same_capture(&occurrence)
                            && (range.sample_start, range.sample_end)
                                == (52_000 + base, 55_000 + base)),
                    "the corroborated short word retains its native PCM exactly once"
                );
            } else {
                assert!(!replay_refusal(&events, "w"), "{warnings}");
            }
            assert_eq!(
                held_text(&lane, &occurrence).as_deref(),
                if corroborated {
                    Some("domu w dalej koniec")
                } else {
                    Some("domu dalej koniec")
                },
                "{warnings}; choices={:?}",
                lane.state.acoustic_ledger.lock().unwrap().word_choices()
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            if !corroborated {
                assert!(
                    ledger
                        .word_choices()
                        .iter()
                        .all(|choice| choice.candidate.surface != "domu w"),
                    "ignored context must not create a split vote: {:#?}",
                    ledger.word_choices()
                );
                // The complete windows paid the text debt (decision of Maciej,
                // 2026-10-21); the unresolved split is a lexical dispute kept
                // in word choices, not missing speech.
                assert!(
                    ledger.text_recovery_pending(&occurrence),
                    "the actual54000..55000 voiced tail was never transcribed by its owner"
                );
            }
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn apple_held_range_does_not_replay_an_owned_word() {
        let mut lane = open("seam-apple-held");
        let member = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 152_000);
        stage(&mut lane, 1, member.clone(), "apple");
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 2,
                sample_start: 48_000,
                sample_end: 96_000,
            },
            &[(1, member)],
            &[word_pin("seam-apple-held", "nowe", 60_000, 70_000)],
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert_eq!(routes[0].exclusive[0].text, "nowe");
        assert!(!replay_refusal(&drain(&mut lane.rx), "nowe"));
    }

    #[test]
    fn word_wholly_inside_admit_keeps_exclusive_tail_route() {
        let mut lane = open("seam-normal");
        let member = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 48_000, 96_000);
        let routes = lane.state.route_overlap_pins(
            &lane.tx,
            AdmitWindow {
                request_id: 2,
                sample_start: 48_000,
                sample_end: 96_000,
            },
            &[(1, member)],
            &[word_pin("seam-normal", "zwykle", 60_000, 70_000)],
            LedgerObservationProducer::Whisper,
        );
        assert_eq!(routes[0].exclusive.len(), 1);
        assert_eq!(routes[0].exclusive[0].text, "zwykle");
        assert!(!routes[0].blocked);
    }

    /// (a) Apple text debt, three step-1 windows, overlap duplicates, one
    /// word across a window edge.
    ///
    /// A seam word owned by the second window enters its slice there; the
    /// first window's copy is replay. Other duplicate pins remain replay.
    #[test]
    fn debt_long_occurrence_joins_exclusive_word_pins_around_a_straddle() {
        let session = "relay-debt-long";
        let mut lane = open(session);
        let mut pcm = vec![0.0_f32; LONG_SAMPLES as usize];
        for (start, end) in [
            (50_000, 61_000),
            (60_000, 66_000),
            (66_000, 78_000),
            (80_000, 98_000),
            (100_000, 140_000),
        ] {
            pcm[start..end].fill(0.2);
        }
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let occurrence = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        qualify_unlabelled(&mut lane, &occurrence);
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        let windows = [(0, 144_000), (48_000, LONG_SAMPLES)];
        for (index, request) in requests.iter().enumerate() {
            let frame = &request.provider_request.identity.range;
            assert_eq!((frame.sample_start, frame.sample_end), windows[index]);
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                windows[index]
            );
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let words = long_word_windows(session);
            assert!(
                words[index]
                    .iter()
                    .all(|pin| pin.range.sample_start >= frame.sample_start
                        && pin.range.sample_end <= frame.sample_end),
                "every word must lie inside the actual decoded PCM"
            );
        }
        let events = play_long_words(&mut lane, &requests, session);
        let warnings = warning_lines(&events);
        assert!(!replay_refusal(&events, "raz"));
        assert!(!replay_refusal(&events, "krawedz"));
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .word_deletions()
                .is_empty(),
            "context copies cannot retire the first owned word pins"
        );
        assert_eq!(
            (
                lane.state.tail_patch_jobs_applied,
                lane.state.tail_patch_jobs_skipped
            ),
            (2, 0),
            "each completing window admits words; held={:?}\n{warnings}",
            held_text(&lane, &occurrence)
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("raz krawedz echo trzy cztery")
        );
        // Both complete windows paid the text debt (decision of Maciej,
        // 2026-10-21). "dwa" against the held "echo" is a lexical dispute,
        // retained below as an alternative; it does not prove missing speech
        // (recovery_source_is_pending).
        assert!(
            !lane
                .state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .text_recovery_pending(&occurrence),
            "a lexical dispute is not text debt"
        );
        assert_eq!(held_count(&lane), 1);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .all(|alternative| alternative.candidate != "dwa"),
            "out-of-band context cannot become an independent alternative"
        );
        assert!(
            ledger
                .slots_of(&occurrence)
                .unwrap()
                .iter()
                .any(|slot| slot.text == "echo"
                    && slot.sample_start == 66_000
                    && slot.sample_end == 78_000)
        );
        assert!(ledger.word_deletions().is_empty());
        drop(ledger);
        assert_conserved(&lane, None);
    }

    /// (b) The same windows with Apple text already on the occurrence. Under
    /// D5 (absence is not evidence) "ras echo" is one coarse source over the
    /// whole owner and neither window's words cover all of its measured
    /// speech, so it keeps its words; (a) carries the word routing.
    #[test]
    fn labelled_long_occurrence_keeps_a_coarse_label_no_window_covers() {
        let session = "relay-labelled-long";
        let mut lane = open(session);
        let mut pcm = vec![0.0_f32; LONG_SAMPLES as usize];
        for (start, end) in [
            (50_000, 61_000),
            (60_000, 66_000),
            (66_000, 78_000),
            (80_000, 98_000),
            (100_000, 140_000),
        ] {
            pcm[start..end].fill(0.2);
        }
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let occurrence = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        stage(&mut lane, 1, occurrence.clone(), "ras echo");
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "ras echo");
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        let windows = [(0, 144_000), (48_000, LONG_SAMPLES)];
        for (index, request) in requests.iter().enumerate() {
            let frame = &request.provider_request.identity.range;
            assert_eq!((frame.sample_start, frame.sample_end), windows[index]);
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                windows[index]
            );
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let words = long_word_windows(session);
            assert!(
                words[index]
                    .iter()
                    .all(|pin| pin.range.sample_start >= frame.sample_start
                        && pin.range.sample_end <= frame.sample_end),
                "every word must lie inside the actual decoded PCM"
            );
        }
        let events = play_long_words(&mut lane, &requests, session);
        let warnings = warning_lines(&events);
        assert!(
            !replay_refusal(&events, "echo"),
            "the first window's overlap pin is replay against the committed occurrence\n{warnings}"
        );
        assert!(!replay_refusal(&events, "raz"), "{warnings}");
        // Both returns were preserved, so the worker counts both skipped.
        assert_eq!(
            (
                lane.state.tail_patch_jobs_applied,
                lane.state.tail_patch_jobs_skipped
            ),
            (0, 2),
            "held={:?}\n{warnings}",
            held_text(&lane, &occurrence)
        );
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("ras echo"));
        assert_eq!(held_count(&lane), 1);
        let ledger = lane.state.acoustic_ledger.lock().unwrap();
        let source = ledger.slots_of(&occurrence).unwrap().to_vec();
        assert_eq!(source.len(), 1);
        assert_eq!(
            ledger
                .slot_alternatives()
                .iter()
                .filter(|alternative| alternative.sources == source
                    && alternative.reason == "absence_requires_acoustic_evidence")
                .count(),
            2,
            "each window's reading waits over the coarse source: {:?}",
            ledger.slot_alternatives()
        );
        assert!(ledger.word_deletions().is_empty());
        drop(ledger);
        assert_conserved(&lane, None);
    }

    #[test]
    fn overlapping_transport_pins_cannot_claim_whisper_authority() {
        let session = "relay-labelled-long";
        let mut lane = open(session);
        let mut pcm = vec![0.0_f32; LONG_SAMPLES as usize];
        for (start, end) in [
            (50_000, 61_000),
            (60_000, 66_000),
            (66_000, 78_000),
            (80_000, 98_000),
            (100_000, 140_000),
        ] {
            pcm[start..end].fill(0.2);
        }
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        let occurrence = OccurrenceIdentity::new(session, 1, 0, LONG_SAMPLES);
        stage(&mut lane, 1, occurrence.clone(), "ras echo");
        lane.state.capture_stopping = true;
        lane.state.pump_capture_windows(&lane.tx);
        close_lexicon(&mut lane, 1, &occurrence, "ras echo");
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 2);
        let windows = [(0, 144_000), (48_000, LONG_SAMPLES)];
        for (index, request) in requests.iter().enumerate() {
            let frame = &request.provider_request.identity.range;
            assert_eq!((frame.sample_start, frame.sample_end), windows[index]);
            assert_eq!(
                (request.admit_sample_start, request.admit_sample_end),
                windows[index]
            );
            assert_eq!(
                request.audio,
                pcm[frame.sample_start as usize..frame.sample_end as usize]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let words = long_word_windows(session);
            assert!(
                words[index]
                    .iter()
                    .all(|pin| pin.range.sample_start >= frame.sample_start
                        && pin.range.sample_end <= frame.sample_end),
                "every word must lie inside the actual decoded PCM"
            );
        }
        let mut windows = long_word_windows(session);
        // Keep the original ambiguous observation as a negative case even
        // when the positive fixture uses disjoint child-word ranges.
        windows[0][0].range.sample_end = 61_000;
        windows[1][0].range.sample_end = 61_000;
        assert_eq!(
            windows[0][0].range.sample_end - windows[0][1].range.sample_start,
            1_000
        );
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            let frame = &request.provider_request.identity.range;
            assert!(
                segments
                    .iter()
                    .all(|pin| pin.range.sample_start >= frame.sample_start
                        && pin.range.sample_end <= frame.sample_end)
            );
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 9.5);
            events.extend(drain(&mut lane.rx));
        }
        let ledger = lane.state.acoustic_ledger.lock().expect("synthetic ledger");
        assert!(warning_lines(&events).contains("untrusted_evidence"));
        assert!(
            whisper_mutations(&events).is_empty(),
            "invalid pins cannot mint Whisper authority"
        );
        assert_eq!(ledger.text_of(&occurrence), Some("ras echo"));
        assert!(
            ledger
                .slots_of(&occurrence)
                .unwrap()
                .iter()
                .all(|slot| slot.producer != ObservationProducer::Whisper)
        );
        assert!(ledger.word_deletions().is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
    }

    fn recovery_payload(
        occurrence: &OccurrenceIdentity,
        segments: Vec<TimedTailSegment>,
    ) -> TailProviderPayload {
        let text = segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let segment_grain = if segments
            .iter()
            .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word)
        {
            crate::stt::tail_provider::TailSegmentGrain::Word
        } else {
            crate::stt::tail_provider::TailSegmentGrain::Phrase
        };
        TailProviderPayload {
            identity: crate::stt::tail_provider::TailRequestIdentity {
                request_id: 9,
                range: crate::stt::tail_provider::TailSampleRange {
                    session: occurrence.session.clone(),
                    capture_epoch: occurrence.capture_epoch,
                    sample_start: occurrence.sample_start,
                    sample_end: occurrence.sample_end,
                },
            },
            text,
            segments,
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.1),
            provider_id: crate::stt::tail_provider::TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: crate::stt::tail_provider::TailProviderEvidence {
                segment_grain,
                source: crate::stt::tail_provider::TailEvidenceSource::Whisper,
                revision: Some("relay-recovery".into()),
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: crate::stt::tail_provider::TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        }
    }

    /// (c) Stop recovery: every word pin is inside the debt occurrence except
    /// one that runs past the end by a few samples.
    ///
    /// Contract step 3: the escape is unanchored and does not block the
    /// contained pins. The contained label is the occurrence's whisper text.
    #[test]
    fn midpoint_owned_edge_word_joins_the_recovery() {
        let mut lane = open("relay-recovery-escape");
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 32_000);
        // The recovery decodes 0..48 000 of the capture: speech under the two
        // decoded words, measured room tone around them.
        let mut pcm = vec![ROOM_TONE_RMS; 48_000];
        pcm[4_000..12_000].fill(0.2);
        pcm[30_000..32_008].fill(0.2);
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        stage(&mut lane, 1, occurrence.clone(), "apple");
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .require_text_recovery(&occurrence),
            "debt is recorded before lexicon closes the frontier"
        );
        let mut payload = recovery_payload(
            &occurrence,
            vec![
                word_pin(&occurrence.session, "nowy", 4_000, 12_000),
                word_pin(&occurrence.session, "ucieka", 30_000, 32_008),
            ],
        );
        // This synthetic recovery decodes 0..48_000, including right context;
        // the occurrence owns 0..32_000. Every returned pin must be in the decode.
        payload.identity.range.sample_end = 48_000;
        assert!(
            payload.validate().is_ok(),
            "fixture has an authenticated decode clock"
        );
        let cleared =
            admit_completed_window_recovery(&mut lane.state, &lane.tx, &occurrence, &payload);
        let events = drain(&mut lane.rx);
        let warnings = warning_lines(&events);
        assert!(
            cleared,
            "contained pins must clear the debt; held={:?}\n{warnings}",
            held_text(&lane, &occurrence)
        );
        assert!(!unanchored_label(&events, "ucieka"), "{warnings}");
        assert!(
            !warnings.contains("seal_coverage_text_recovery_refused"),
            "{warnings}"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("nowy ucieka")
        );
        assert!(
            !lane
                .state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .text_recovery_pending(&occurrence)
        );
        assert_replaced_slot_evidence(&lane, &occurrence, &["apple"]);
        assert_conserved(&lane, None);
    }

    /// (d) Debt recovery cannot replace an overlapping higher-rank Lexicon slot.
    /// The open owner retains its slot, with a refusal in the layer trail; the
    /// complete recovery window pays the text debt it covered (bbc05929).
    #[test]
    fn recovery_words_refuse_overlapping_higher_rank_lexicon_slot() {
        let mut lane = open("relay-recovery-lexicon");
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 32_000);
        // The owner's speech reaches the buffer and the energy owner, so the
        // recovered word is measured before its rank is weighed.
        let pcm = vec![0.2_f32; 32_000];
        lane.state.audio.push(&pcm);
        record_energy(
            &lane,
            &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        stage(&mut lane, 1, occurrence.clone(), "lexikon trzymac");
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .require_text_recovery(&occurrence),
            "debt is recorded before lexicon closes the frontier"
        );
        assert!(
            admit_ledger_label(
                &mut lane.state,
                &lane.tx,
                LabelAdmission {
                    observation: ObservationIdentity::new(
                        ObservationProducer::Lexicon,
                        1,
                        0,
                        occurrence.clone(),
                    ),
                    label: "lexikon trzyma",
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some_and(|receipt| receipt.grants_mutation())
        );
        assert!(
            lane.state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .require_text_recovery(&occurrence)
        );
        let slots_before = {
            let ledger = lane.state.acoustic_ledger.lock().expect("ledger");
            assert!(!ledger.is_sealed(&occurrence));
            let slots = ledger.slots_of(&occurrence).expect("lexicon slot").to_vec();
            assert_eq!(slots.len(), 1);
            assert_eq!(slots[0].producer, ObservationProducer::Lexicon);
            assert_eq!(slots[0].text, "lexikon trzyma");
            slots
        };
        let _ = drain(&mut lane.rx);
        let payload = recovery_payload(
            &occurrence,
            vec![word_pin(&occurrence.session, "whisper inny", 4_000, 12_000)],
        );
        let recovered =
            admit_completed_window_recovery(&mut lane.state, &lane.tx, &occurrence, &payload);
        // Drain before asserting so an unexpected result reports the actual events.
        let events = drain(&mut lane.rx);
        assert!(recovered, "the complete window pays its debt: {events:#?}");
        assert_eq!(
            whisper_mutations(&events),
            vec![
                (
                    "whisper inny".to_string(),
                    MutationReceipt::Refuse {
                        occurrence: OccurrenceIdentity::new(
                            &occurrence.session,
                            occurrence.capture_epoch,
                            4_000,
                            12_000
                        ),
                        reason: RefuseReason::SlotAdmissionRejected,
                    },
                ),
                (
                    "lexikon trzyma".to_string(),
                    MutationReceipt::Refuse {
                        occurrence: occurrence.clone(),
                        reason: RefuseReason::SlotAdmissionRejected,
                    },
                )
            ],
            "{events:#?}"
        );
        assert_eq!(mutation_count(&events), 0);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("lexikon trzyma")
        );
        {
            let ledger = lane.state.acoustic_ledger.lock().expect("ledger");
            assert!(!ledger.text_recovery_pending(&occurrence));
            assert!(!ledger.is_sealed(&occurrence));
            assert_eq!(
                ledger.slots_of(&occurrence).unwrap(),
                slots_before.as_slice()
            );
            // The routed event preserves the held label; the rejected candidate's
            // receipt lives in the ledger trail. This open owner was not sealed.
            let refused = ledger
                .layer_trail_for(&occurrence)
                .filter(|entry| {
                    entry.observation.producer == ObservationProducer::Whisper
                        && entry.observation.request == payload.identity.request_id
                        && entry.candidate_label == "whisper inny"
                })
                .collect::<Vec<_>>();
            assert_eq!(refused.len(), 1, "trail={refused:#?}; events={events:#?}");
            assert_eq!(
                refused[0].decision,
                MutationReceipt::Refuse {
                    occurrence: occurrence.clone(),
                    reason: RefuseReason::SlotAdmissionRejected,
                },
                "{events:#?}"
            );
            ledger.assert_slot_labels();
        }
        assert_conserved(&lane, Some("slot_admission_rejected"));
    }
    #[test]
    fn forensic_measured_group_edge_has_no_duplicate_and_retains_candidate() {
        for mode in BufferMode::BOTH {
            let session = mode.session("forensic-group-edge");
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 0, 200_000, &[(10_000, 60_000)]);
            {
                let mut ledger = lane.state.acoustic_ledger.lock().unwrap();
                let obs = ledger.next_word_observation(ObservationProducer::Apple, 1, &owner);
                assert!(
                    ledger
                        .admit_word_slots(
                            &obs,
                            &[
                                crate::pipeline::acoustic_ledger::WordPin::new(
                                    10_000,
                                    40_000,
                                    "albo prośbę o"
                                ),
                                crate::pipeline::acoustic_ledger::WordPin::new(
                                    50_000, 60_000, "wierszyk"
                                )
                            ]
                        )
                        .grants_mutation()
                );
            }
            lane.state.complete_whisper_window(
                &lane.tx,
                completion(
                    &requests[0],
                    vec![
                        word_pin(&session, "albo", 12_000, 30_000),
                        word_pin(&session, "prośbę", 31_000, 50_000),
                    ],
                ),
                12.5,
            );
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger
                    .text_of(&owner)
                    .unwrap()
                    .split_whitespace()
                    .filter(|w| *w == "prośbę")
                    .count(),
                1
            );
            assert!(
                ledger
                    .slot_alternatives()
                    .iter()
                    .any(|a| a.candidate.split_whitespace().any(|w| w == "prośbę")),
                "coarse source label remains in the lineage"
            );
            assert!(!ledger.is_sealed(&owner));
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn forensic_late_measured_recovery_cannot_cross_lexicon_authority() {
        for mode in BufferMode::BOTH {
            let session = mode.session("forensic-rank-recovery");
            let (mut lane, owner, requests, _) =
                forensic_boundary_capture(&session, 0, 32_000, &[(0, 32_000)]);
            for (producer, label) in [
                (ObservationProducer::Apple, "lexikon trzymac"),
                (ObservationProducer::Lexicon, "lexikon trzyma"),
            ] {
                let observation = lane
                    .state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .next_word_observation(producer, 1, &owner);
                assert!(
                    admit_ledger_label(
                        &mut lane.state,
                        &lane.tx,
                        LabelAdmission {
                            observation,
                            label,
                            energy: EnergyAdmission::RequireExistingQualification
                        }
                    )
                    .unwrap()
                    .grants_mutation()
                );
            }
            let before = {
                let mut l = lane.state.acoustic_ledger.lock().unwrap();
                assert!(l.require_text_recovery(&owner));
                l.slots_of(&owner).unwrap().to_vec()
            };
            let _ = drain(&mut lane.rx);
            let result = completion(
                &requests[0],
                vec![
                    word_pin(&session, "whisper", 4_000, 8_000),
                    word_pin(&session, "inny", 8_000, 12_000),
                ],
            );
            lane.state.complete_whisper_window(&lane.tx, result, 9.0);
            let events = drain(&mut lane.rx);
            assert_eq!(mutation_count(&events), 0);
            let ledger = lane.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
            assert_eq!(ledger.text_of(&owner), Some("lexikon trzyma"));
            // The complete window paid the text debt it covered; no producer
            // rank is asked (bbc05929, decision of Maciej, 2026-10-21). The
            // Lexicon words stay and the answered owner seals.
            assert!(!ledger.text_recovery_pending(&owner));
            assert!(ledger.is_sealed(&owner));
            let trail = ledger
                .layer_trail_for(&owner)
                .filter(|e| e.observation.producer == ObservationProducer::Whisper)
                .collect::<Vec<_>>();
            assert!(!trail.is_empty());
            assert!(trail.iter().all(|e| !e.decision.grants_mutation()));
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }
