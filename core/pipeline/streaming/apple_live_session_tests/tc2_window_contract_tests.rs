/// T-C2r contract falsifiers, authored before the admission cut and unrun in W1.
/// Coordinates come from take ef1fa240; lexical labels and PCM amplitudes below
/// are synthetic. No acoustic or transcription quality is asserted by a fixture.
    use super::super::silero_fusion::UtteranceLedger;
    use super::*;
    use crate::pipeline::acoustic_ledger::SlotWitness;
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailSegmentGrain, TailTimingQuality,
    };

    const SESSION: &str = "ef1fa240-window-contract";
    const RATE: u32 = 48_000;

    struct Fixture {
        state: AppleSealState,
        physical: UtteranceLedger,
        events: mpsc::UnboundedSender<EngineEvent>,
        receiver: mpsc::UnboundedReceiver<EngineEvent>,
        requests: mpsc::Receiver<TailPatchRequest>,
        occurrence: OccurrenceIdentity,
        capture: Option<CaptureLevelAccumulator>,
        script: Vec<TimedTailSegment>,
    }

    /// Calibrated existence floor of the fixture's capture device. Fixture
    /// speech is 0.2 RMS, so every word over the take's PCM clears it.
    const EXISTENCE_FLOOR_RMS: f32 = 0.01;

    fn fixture() -> Fixture {
        fixture_with_pcm(vec![0.2; 719_000], true)
    }

    fn fixture_for_words(words: &[TimedTailSegment]) -> Fixture {
        let mut pcm = vec![0.00001; 719_000];
        for word in words {
            pcm[word.range.sample_start as usize..word.range.sample_end as usize].fill(0.2);
        }
        let mut f = fixture_with_pcm(pcm, true);
        f.script = words.to_vec();
        f
    }

    fn fixture_with_pcm(pcm: Vec<f32>, measure_energy: bool) -> Fixture {
        assert_eq!(pcm.len(), 719_000);
        let (events, receiver) = mpsc::unbounded_channel();
        let (sender, requests) = mpsc::channel(8);
        let mut state = AppleSealState::new_with_tail_patch_for_session(
            RATE,
            SESSION.into(),
            1,
            sender,
            Arc::new(Mutex::new(AcousticLedger::new())),
            Some(EnergyCalibration {
                version: "tc2-physical-geometry".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            }),
        );
        state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
        state.audio.push(&pcm);
        let mut capture =
            measure_energy.then(|| CaptureLevelAccumulator::bound_to(&state.capture_energy));
        if let Some(capture) = capture.as_mut() {
            for chunk in pcm.chunks(960) {
                capture.push_samples(chunk);
            }
        }
        let mut physical = UtteranceLedger::new();
        physical.open_or_extend(SESSION, 1, 227_328, 510_464);
        physical.close_open(510_464);
        assert!(reconcile_silero_ledger(&mut state, &events, &physical, &[]));
        let occurrence = OccurrenceIdentity::new(SESSION, 1, 227_328, 510_464);
        {
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert!(ledger.is_qualified(&occurrence));
            assert!(ledger.text_of(&occurrence).is_none());
            assert!(!ledger.is_sealed(&occurrence));
            assert_eq!(
                ledger.frontier_of(&occurrence).unwrap().open_producers(),
                vec![LedgerObservationProducer::Whisper]
            );
        }
        state.pump_capture_windows(&events);
        Fixture {
            state,
            physical,
            events,
            receiver,
            requests,
            occurrence,
            capture,
            script: Vec::new(),
        }
    }

    /// Later capture: the PCM reaches the buffer and the energy owner
    /// together, on the same 20 ms hops as the fixture's first capture.
    fn capture_more(f: &mut Fixture, pcm: &[f32]) {
        f.state.audio.push(pcm);
        let capture = f
            .capture
            .as_mut()
            .expect("the fixture measures its capture");
        for chunk in pcm.chunks(960) {
            capture.push_samples(chunk);
        }
    }

    fn pin(text: &str, start: u64, end: u64) -> TimedTailSegment {
        TimedTailSegment {
            confidence: None,
            grain: TailSegmentGrain::Word,
            text: text.into(),
            range: TailSampleRange {
                session: SESSION.into(),
                capture_epoch: 1,
                sample_start: start,
                sample_end: end,
            },
        }
    }

    fn completion(
        request: &TailPatchRequest,
        segments: Vec<TimedTailSegment>,
    ) -> TailPatchCompletion {
        let grain = if segments.is_empty() {
            TailSegmentGrain::Phrase
        } else {
            TailSegmentGrain::Word
        };
        TailPatchCompletion {
            submission_sequence: request.submission_sequence,
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: Some(TailProviderPayload {
                identity: request.provider_request.identity.clone(),
                text: segments
                    .iter()
                    .map(|pin| pin.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                segments,
                avg_logprob: Some(-0.2),
                compression_ratio: Some(1.0),
                provider_id: TailProviderId::Fake,
                elapsed_ms: 1,
                evidence: TailProviderEvidence {
                    segment_grain: grain,
                    source: TailEvidenceSource::Whisper,
                    revision: Some("tc2-real-geometry-synthetic-labels".into()),
                    stability: TailEvidenceStability::Final,
                    timing_quality: TailTimingQuality::ExactSampleRange,
                    avg_logprob: Some(-0.2),
                    vad_selected_samples: None,
                },
            }),
            member_occurrences: request.member_occurrences.clone(),
        }
    }

    fn assert_words(fixture: &Fixture, expected: &str) {
        let ledger = fixture.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&fixture.occurrence), Some(expected));
        ledger.assert_slot_labels();
        for slot in ledger.slots_of(&fixture.occurrence).unwrap() {
            assert_eq!(slot.witness, SlotWitness::Unwitnessed);
        }
        assert!(
            ledger
                .frontier_of(&fixture.occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        assert_eq!(ledger.conservation().residue(), 0);
    }

    /// Progress the real capture grid past this owner, including every newly
    /// submitted overlapping frame. Existing in-flight returns remain owned.
    fn advance_live_horizon(f: &mut Fixture) {
        let _ = advance_live_horizon_frames(f);
    }

    fn advance_live_horizon_frames(f: &mut Fixture) -> Vec<TailPatchRequest> {
        let head = f.state.audio.session_sample_end();
        assert!(head <= 864_000);
        capture_more(f, &vec![0.2; (864_000 - head) as usize]);
        f.state.pump_capture_windows(&f.events);
        let mut frames = Vec::new();
        while let Ok(request) = f.requests.try_recv() {
            let words = f
                .script
                .iter()
                .filter(|word| {
                    request
                        .provider_request
                        .identity
                        .range
                        .contains(&word.range)
                })
                .cloned()
                .collect();
            let result = completion(&request, words);
            result.payload.as_ref().unwrap().validate().unwrap();
            f.state.complete_whisper_window(&f.events, result, 18.0);
            frames.push(request);
        }
        assert!(f.state.window_plan.admission_horizon() >= f.occurrence.sample_end);
        frames
    }

    #[test]
    fn late_apple_requires_current_unclamped_pcm() {
        for defect in ["stale", "negative", "nonfinite", "past_capture", "straddle"] {
            let mut f = fixture();
            let start = 434_496;
            let end = if defect == "straddle" {
                520_000
            } else {
                470_976
            };
            let word = FusionWord::from_timed(&pin("late", start, end));
            let mut current = vec![apple_word("late", start, end)];
            match defect {
                "stale" => current.clear(),
                "negative" => current[0].start_ts = -1.0,
                "nonfinite" => current[0].start_ts = f32::NAN,
                "past_capture" => current[0].end_ts = 20.0,
                "straddle" => {}
                _ => unreachable!(),
            }
            while f.receiver.try_recv().is_ok() {}
            admit_late_apple_words(&mut f.state, &f.events, 1, &[word], &current);
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&f.occurrence), None, "{defect}");
            assert_eq!(ledger.conservation().residue(), 0, "{defect}");
            drop(ledger);
            assert!(
                std::iter::from_fn(|| f.receiver.try_recv().ok()).any(|event| matches!(
                    event,
                    EngineEvent::LedgerMutation {
                        label,
                        // AR-1b: a non-current late word is evidence at its own
                        // pin under its own reason; admission is still refused.
                        receipt: MutationReceipt::KeepVisibleUnanchored {
                            reason: NoAuthorityReason::LateAppleWordNotCurrent,
                            ..
                        },
                        ..
                    } if label == "late"
                )),
                "{defect}"
            );
        }
    }

    #[test]
    fn horizon_open_owner_does_not_widen_the_next_window() {
        let mut f = fixture();
        let first = f.requests.try_recv().unwrap();
        let second = f.requests.try_recv().unwrap();
        for request in [&first, &second] {
            f.state.complete_whisper_window(
                &f.events,
                completion(request, vec![pin("alpha", 246_464, 262_784)]),
                10.7,
            );
        }
        assert!(f.state.refinement_submitted.is_empty());
        assert!(f.state.refinement_pending.is_empty());
        assert_words(&f, "alpha");
        let head = f.state.audio.session_sample_end();
        capture_more(&mut f, &vec![0.2; 743_424 - head as usize]);
        f.physical.open_or_extend(SESSION, 1, 508_416, 743_424);
        f.physical.close_open(743_424);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[],
        ));
        assert!(
            f.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .is_qualified(&OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424))
        );
        f.state.pump_capture_windows(&f.events);
        let next = f.requests.try_recv().unwrap();
        assert_eq!(next.provider_request.identity.range.sample_start, 288_000);
        assert_eq!(next.provider_request.identity.range.sample_end, 720_000);
        let newer = OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424);
        let mut members = next
            .member_occurrences
            .iter()
            .map(|(_, owner)| owner.clone())
            .collect::<Vec<_>>();
        members.sort_by_key(|owner| owner.sample_start);
        assert_eq!(members, vec![f.occurrence.clone(), newer.clone()]);
        assert!(
            next.provider_request.identity.range.sample_end < newer.sample_end,
            "the capture clock must not widen the window out to the new owner"
        );
        assert!(f.requests.try_recv().is_err());
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.qualified_occurrences().count(), 2);
        assert!(!ledger.is_sealed(&f.occurrence));
        assert_eq!(ledger.conservation().residue(), 0);
    }

    /// RED contract 1: request 2's two completions, then a future request's pad.
    /// The third job does not even exist when request 2's last window completes.
    /// Waiting only for currently submitted jobs cannot protect this owner.
    #[test]
    fn ef1fa_long_occurrence_admits_each_window_and_future_pad() {
        let mut f = fixture_for_words(&[
            pin("alpha", 246_464, 262_784),
            pin("beta", 262_784, 284_864),
            pin("gamma", 382_080, 418_560),
            pin("delta", 434_496, 470_976),
            pin("epsilon", 652_416, 659_136),
        ]);
        let first = f.requests.try_recv().unwrap();
        let second = f.requests.try_recv().unwrap();
        assert!(f.requests.try_recv().is_err());
        assert_eq!(first.provider_request.identity.range.sample_start, 0);
        assert_eq!(first.provider_request.identity.range.sample_end, 432_000);
        assert_eq!(second.provider_request.identity.range.sample_start, 144_000);
        assert_eq!(second.provider_request.identity.range.sample_end, 576_000);
        f.state.complete_whisper_window(
            &f.events,
            completion(
                &first,
                vec![
                    pin("alpha", 246_464, 262_784),
                    pin("beta", 262_784, 284_864),
                ],
            ),
            8.8,
        );
        assert_words(&f, "alpha beta");
        f.state.complete_whisper_window(
            &f.events,
            completion(
                &second,
                vec![
                    pin("alpha", 246_464, 262_784),
                    pin("gamma", 382_080, 418_560),
                ],
            ),
            10.7,
        );
        assert_words(&f, "alpha beta gamma");
        let after_second = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
        {
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger
                    .slots_of(&f.occurrence)
                    .unwrap()
                    .iter()
                    .filter(|slot| slot.text == "alpha")
                    .count(),
                1
            );
            assert!(
                !ledger.slot_operations().iter().any(|operation| operation
                    .sources
                    .iter()
                    .any(|source| source.text == "alpha")
                    && operation
                        .outputs
                        .iter()
                        .any(|output| output.text == "alpha")),
                "the context return cannot retire or rewrite the published alpha: {after_second:#?}"
            );
        }

        // Next physical closure and its context pad, from the same take.
        let head = f.state.audio.session_sample_end();
        capture_more(&mut f, &vec![0.2; 743_424 - head as usize]);
        f.physical.open_or_extend(SESSION, 1, 508_416, 743_424);
        f.physical.close_open(743_424);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[]
        ));
        assert!(
            f.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .is_qualified(&OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424))
        );
        f.state.pump_capture_windows(&f.events);
        let third = f.requests.try_recv().unwrap();
        assert_eq!(third.provider_request.identity.range.sample_start, 288_000);
        assert_eq!(third.provider_request.identity.range.sample_end, 720_000);
        let newer = OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424);
        assert!(
            third
                .member_occurrences
                .iter()
                .any(|(_, owner)| owner == &f.occurrence)
        );
        assert!(
            third
                .member_occurrences
                .iter()
                .any(|(_, owner)| owner == &newer)
        );
        assert!(f.requests.try_recv().is_err());
        f.state.complete_whisper_window(
            &f.events,
            completion(
                &third,
                vec![
                    pin("gamma", 384_576, 400_896),
                    pin("delta", 434_496, 470_976),
                    pin("epsilon", 652_416, 659_136),
                ],
            ),
            14.6,
        );
        assert_words(&f, "alpha beta gamma delta");
        let after_third = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(!after_third.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { label, receipt: MutationReceipt::Refuse { .. }, .. }
                | EngineEvent::LedgerMutation { label, receipt: MutationReceipt::KeepVisibleUnanchored { .. }, .. }
                if label == "delta"
        )));
    }

    /// Context cannot publish an edge word before its own middle-band return.
    #[test]
    fn first_label_keeps_whisper_open_for_later_window_generations() {
        let mut pcm = vec![0.0; 719_000];
        pcm[246_464..262_784].fill(0.2);
        pcm[434_496..470_976].fill(0.2);
        let mut f = fixture_with_pcm(pcm, true);
        let first = f.requests.try_recv().unwrap();
        let second = f.requests.try_recv().unwrap();
        f.state.complete_whisper_window(
            &f.events,
            completion(&first, vec![pin("alpha", 246_464, 262_784)]),
            8.8,
        );
        assert_words(&f, "alpha");
        f.state.complete_whisper_window(
            &f.events,
            completion(&second, vec![pin("delta", 434_496, 470_976)]),
            10.7,
        );
        assert_words(&f, "alpha");
        capture_more(&mut f, &[0.0; 1000]);
        f.state.pump_capture_windows(&f.events);
        let owning = f.requests.try_recv().unwrap();
        assert_eq!(owning.provider_request.identity.range.sample_start, 288_000);
        f.state.complete_whisper_window(
            &f.events,
            completion(&owning, vec![pin("delta", 434_496, 470_976)]),
            15.0,
        );
        assert_words(&f, "alpha delta");
        advance_live_horizon(&mut f);
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_sealed(&f.occurrence));
        assert!(ledger.frontier_of(&f.occurrence).unwrap().is_closed());
        ledger.assert_slot_labels();
    }

    #[test]
    fn horizon_waits_for_earlier_windows_in_either_completion_order() {
        for reversed in [false, true] {
            let mut f = fixture_for_words(&[
                pin("alpha", 246_464, 262_784),
                pin("delta", 434_496, 470_976),
            ]);
            let first = f.requests.try_recv().unwrap();
            let second = f.requests.try_recv().unwrap();
            let mut returns = vec![
                completion(&first, vec![pin("alpha", 246_464, 262_784)]),
                completion(&second, vec![pin("delta", 434_496, 470_976)]),
            ];
            if reversed {
                returns.reverse();
            }
            advance_live_horizon(&mut f);
            f.state
                .complete_whisper_window(&f.events, returns.remove(0), 10.7);
            assert!(
                !f.state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .is_sealed(&f.occurrence)
            );
            f.state
                .complete_whisper_window(&f.events, returns.remove(0), 10.7);
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&f.occurrence), Some("alpha delta"));
            assert!(ledger.is_sealed(&f.occurrence));
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn drained_stop_returns_horizon_without_a_deadline_failure() {
        let mut f = fixture_for_words(&[pin("alpha", 246_464, 262_784)]);
        f.state.capture_stopping = true;
        f.state.pump_capture_windows(&f.events);
        while let Ok(request) = f.requests.try_recv() {
            let frame = &request.provider_request.identity.range;
            let words = if frame.sample_start <= 246_464 && 262_784 <= frame.sample_end {
                vec![pin("alpha", 246_464, 262_784)]
            } else {
                vec![]
            };
            f.state
                .complete_whisper_window(&f.events, completion(&request, words), 10.7);
        }
        f.state.return_outstanding_whisper_without_label(&f.events);
        assert!(
            f.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .is_sealed(&f.occurrence)
        );
        let events = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(!events.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "live_refinement_stop_deadline")));
    }
    #[test]
    fn later_window_start_closes_owner_on_success_or_failure() {
        for failed in [false, true] {
            let mut f = fixture_for_words(&[pin("alpha", 246_464, 262_784)]);
            while let Ok(request) = f.requests.try_recv() {
                f.state.complete_whisper_window(
                    &f.events,
                    completion(&request, vec![pin("alpha", 246_464, 262_784)]),
                    10.7,
                );
            }
            assert_words(&f, "alpha");
            capture_more(&mut f, &vec![0.2; 700_000]);
            f.physical.open_or_extend(SESSION, 1, 1_100_000, 1_300_000);
            f.physical.close_open(1_300_000);
            assert!(reconcile_silero_ledger(
                &mut f.state,
                &f.events,
                &f.physical,
                &[]
            ));
            f.state.pump_capture_windows(&f.events);
            let mut later = None;
            while let Ok(request) = f.requests.try_recv() {
                let start = request.provider_request.identity.range.sample_start;
                let end = request.provider_request.identity.range.sample_end;
                let covers_later = start <= 1_120_000 && 1_150_000 <= end;
                if covers_later && start >= f.occurrence.sample_end && later.is_none() {
                    later = Some(request);
                    continue;
                }
                let pad_start = start.max(f.occurrence.sample_end);
                let pad_end = pad_start.saturating_add(1_000);
                let result = if pad_start >= start && pad_end <= end {
                    completion(&request, vec![pin("pad", pad_start, pad_end)])
                } else {
                    let mut missed = completion(&request, vec![]);
                    missed.payload = None;
                    missed
                };
                f.state.complete_whisper_window(&f.events, result, 27.1);
            }
            let later = later.expect("capture clock window covering the later word");
            assert!(later.provider_request.identity.range.sample_start >= f.occurrence.sample_end);
            let mut result = completion(&later, vec![pin("later", 1_120_000, 1_150_000)]);
            if failed {
                result.payload = None;
            }
            f.state.complete_whisper_window(&f.events, result, 27.1);
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert!(ledger.is_sealed(&f.occurrence));
            assert_eq!(ledger.text_of(&f.occurrence), Some("alpha"));
            ledger.assert_slot_labels();
        }
    }

    /// A return parked behind the in-flight live witness stays parked until
    /// the witness answers. Returning it again on every horizon tick logged
    /// 483 745 `admission_horizon_closed` lines in one take (13467366).
    /// The witness is out while the later windows return, as in that take.
    #[test]
    fn horizon_leaves_a_return_parked_behind_the_live_witness() {
        let mut f = fixture();
        while let Ok(request) = f.requests.try_recv() {
            f.state.complete_whisper_window(
                &f.events,
                completion(&request, vec![pin("alpha", 246_464, 262_784)]),
                10.7,
            );
        }
        f.state.dictation_witness_inflight = true;
        advance_live_horizon(&mut f);
        let owner = f.occurrence.clone();
        assert_eq!(f.state.dictation_witness_waiting, vec![owner.clone()]);
        let logged = super::live_refinement_admission_tests::refinement_log(|| {
            for _ in 0..3 {
                f.state.close_admission_horizon(&f.events, owner.sample_end);
            }
        });
        assert!(
            !logged.contains("disposition=admission_horizon_closed"),
            "{logged}"
        );
        assert_eq!(f.state.dictation_witness_waiting, vec![owner.clone()]);
        assert!(
            !f.state.acoustic_ledger.lock().unwrap().is_sealed(&owner),
            "the witness has not answered yet"
        );
        f.state.dictation_witness_inflight = false;
        resume_witness_waiting_closures(&mut f.state, &f.events);
        assert!(f.state.dictation_witness_waiting.is_empty());
        assert!(
            !f.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .frontier_of(&owner)
                .is_some_and(|frontier| frontier
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)),
            "the answer resumes the parked return"
        );
    }

    #[test]
    fn reclose_late_whisper_word_keeps_visible_and_does_not_revise_seal() {
        let mut f = fixture_for_words(&[
            pin("alpha", 246_464, 262_784),
            pin("original", 434_496, 470_976),
            pin("ending", 482_000, 495_000),
        ]);
        while let Ok(request) = f.requests.try_recv() {
            f.state.complete_whisper_window(
                &f.events,
                completion(&request, vec![pin("alpha", 246_464, 262_784)]),
                10.7,
            );
        }
        let returned_frames = advance_live_horizon_frames(&mut f);
        let seal = f
            .state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_of(&f.occurrence)
            .unwrap()
            .clone();
        f.physical.open_or_extend(SESSION, 1, 508_416, 743_424);
        f.physical.close_open(743_424);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[]
        ));
        // These frames have already returned. Replaying diagnostic evidence
        // may remain visible, but cannot reopen a sealed owner or a transport job.
        assert!(returned_frames.len() >= 2);
        let late = completion(&returned_frames[0], vec![pin("delta", 434_496, 470_976)]);
        let payload = late.payload.as_ref().unwrap();
        payload.validate().unwrap();
        f.state.admit_completed_window(&f.events, payload, false);
        let later = completion(&returned_frames[0], vec![pin("zeta", 482_000, 495_000)]);
        let payload = later.payload.as_ref().unwrap();
        payload.validate().unwrap();
        f.state.admit_completed_window(&f.events, payload, false);
        let events = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { label, receipt: MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::LateWhisperWordSealedOwner, .. }, .. } if label == "delta")));
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { label, receipt: MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::LateWhisperWordSealedOwner, .. }, .. } if label == "delta zeta")));
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&f.occurrence), Some("alpha original ending"));
        assert_eq!(ledger.seal_of(&f.occurrence), Some(&seal));
        assert_eq!(ledger.conservation().residue(), 0);
        ledger.assert_slot_labels();
    }

    fn apple_word(text: &str, start: u64, end: u64) -> TranscriptSegment {
        TranscriptSegment {
            confidence: None,
            text: text.into(),
            start_ts: start as f32 / RATE as f32,
            end_ts: end as f32 / RATE as f32,
        }
    }

    #[test]
    fn sealed_owner_repartition_mints_no_second_identity() {
        let mut f = fixture_for_words(&[
            pin("alpha", 246_464, 262_784),
            pin("original", 434_496, 470_976),
        ]);
        while let Ok(request) = f.requests.try_recv() {
            f.state.complete_whisper_window(
                &f.events,
                completion(&request, vec![pin("alpha", 246_464, 262_784)]),
                10.7,
            );
        }
        let returned_frames = advance_live_horizon_frames(&mut f);
        let (seal, frontier) = {
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            (
                ledger.seal_of(&f.occurrence).unwrap().clone(),
                ledger.frontier_of(&f.occurrence).unwrap().clone(),
            )
        };
        let newer = OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424);
        f.physical
            .open_or_extend(SESSION, 1, newer.sample_start, newer.sample_end);
        f.physical.close_open(newer.sample_end);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[],
        ));
        while f.receiver.try_recv().is_ok() {}
        // These frames have already returned. Replaying diagnostic evidence
        // may remain visible, but cannot reopen a sealed owner or a transport job.
        assert!(!returned_frames.is_empty());
        let late = completion(&returned_frames[0], vec![pin("delta", 434_496, 470_976)]);
        let payload = late.payload.as_ref().unwrap();
        payload.validate().unwrap();
        f.state.admit_completed_window(&f.events, payload, false);
        let events = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    EngineEvent::LedgerMutation {
                        observation,
                        label,
                        receipt: MutationReceipt::KeepVisibleUnanchored {
                            occurrence,
                            reason: NoAuthorityReason::LateWhisperWordSealedOwner,
                            ..
                        },
                    } if observation.occurrence == f.occurrence
                        && occurrence == &f.occurrence && label == "delta"
                ))
                .count(),
            1
        );
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        let owners = ledger.qualified_occurrences().collect::<Vec<_>>();
        assert_eq!(owners, vec![&f.occurrence, &newer]);
        assert_eq!(ledger.text_of(&f.occurrence), Some("alpha original"));
        assert_eq!(ledger.seal_of(&f.occurrence), Some(&seal));
        assert_eq!(ledger.frontier_of(&f.occurrence), Some(&frontier));
        assert!(f.state.unmatched_silero_words.is_empty());
        assert_eq!(ledger.conservation().residue(), 0);
        ledger.assert_slot_labels();
    }

    #[test]
    fn late_apple_range_refusals_publish_for_open_and_sealed_whisper_slots() {
        for sealed in [false, true] {
            let mut f = fixture_for_words(&[pin("Alpha,", 246_464, 262_784)]);
            while let Ok(request) = f.requests.try_recv() {
                f.state.complete_whisper_window(
                    &f.events,
                    completion(&request, vec![pin("Alpha,", 246_464, 262_784)]),
                    10.7,
                );
            }
            if sealed {
                advance_live_horizon(&mut f);
            }
            let before = f
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .slots_of(&f.occurrence)
                .unwrap()
                .to_vec();
            for (text, reason) in [
                ("alpha", RefuseReason::SlotAdmissionRejected),
                ("changed", RefuseReason::SlotAdmissionRejected),
            ] {
                while f.receiver.try_recv().is_ok() {}
                let word = FusionWord::from_timed(&pin(text, 248_000, 266_000));
                admit_late_apple_words(
                    &mut f.state,
                    &f.events,
                    2,
                    &[word],
                    &[apple_word(text, 248_000, 266_000)],
                );
                let events = std::iter::from_fn(|| f.receiver.try_recv().ok())
                    .filter(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
                    .collect::<Vec<_>>();
                assert_eq!(events.len(), 1);
                if sealed {
                    assert!(matches!(
                        &events[0],
                        EngineEvent::LedgerMutation {
                            receipt: MutationReceipt::KeepVisibleUnanchored {
                                reason: NoAuthorityReason::LateAppleWordSealedOwner,
                                ..
                            },
                            ..
                        }
                    ));
                } else {
                    assert!(
                        matches!(
                            &events[0],
                            EngineEvent::LedgerMutation {
                                receipt: MutationReceipt::Refuse { reason: actual, .. }, ..
                            } if actual == &reason
                        ),
                        "{events:#?}"
                    );
                }
                let ledger = f.state.acoustic_ledger.lock().unwrap();
                assert_eq!(ledger.slots_of(&f.occurrence).unwrap(), before.as_slice());
                assert_eq!(ledger.text_of(&f.occurrence), Some("Alpha,"));
                assert_eq!(ledger.is_sealed(&f.occurrence), sealed);
            }
        }
    }

    #[test]
    fn fresh_physical_ledger_utterance_ids_do_not_alias_occurrence_identity() {
        let mut f = fixture();
        let held = OccurrenceIdentity::new(SESSION, 1, 56_832, 222_208);
        let mut second = UtteranceLedger::new();
        second.open_or_extend(SESSION, 1, held.sample_start, held.sample_end);
        second.close_open(held.sample_end);
        assert_eq!(f.physical.utterances()[0].id, second.utterances()[0].id);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &second,
            &[apple_word("held", 58_000, 70_000)],
        ));
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert!(ledger.is_qualified(&held));
        assert!(ledger.is_qualified(&f.occurrence));
        assert_eq!(ledger.qualified_occurrences().count(), 2);
        assert_eq!(ledger.text_of(&held), Some("held"));
        assert_eq!(ledger.text_of(&f.occurrence), None);
    }

    #[test]
    fn horizon_open_owner_survives_partition_change_with_one_identity() {
        let mut f = fixture();
        // Speech under the words these owners hear and room tone between
        // them. Two million samples of one tone would measure as a single
        // 42 s burst whose word geometry swallows every later label on it;
        // the pause before `contained` is longer than one word's continuity.
        let mut later = vec![0.004; 2_000_000];
        for (start, end) in [
            (2_140_000, 2_148_000),
            (2_153_000, 2_157_000),
            (2_320_000, 2_324_000),
        ] {
            later[start - 719_000..end - 719_000].fill(0.2);
        }
        capture_more(&mut f, &later);
        let owner = OccurrenceIdentity::new(SESSION, 1, 2_132_992, 2_325_504);
        f.physical
            .open_or_extend(SESSION, 1, owner.sample_start, owner.sample_end);
        f.physical.close_open(owner.sample_end);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[],
        ));
        f.state.pump_capture_windows(&f.events);
        while let Ok(request) = f.requests.try_recv() {
            let word = if request
                .member_occurrences
                .iter()
                .any(|(_, range)| range == &owner)
            {
                pin("anchor", 2_140_000, 2_148_000)
            } else {
                pin("debt", 246_464, 262_784)
            };
            let range = &request.provider_request.identity.range;
            let words = if range.sample_start <= word.range.sample_start
                && word.range.sample_end <= range.sample_end
            {
                vec![word]
            } else {
                vec![]
            };
            let result = completion(&request, words);
            result.payload.as_ref().unwrap().validate().unwrap();
            f.state.complete_whisper_window(&f.events, result, 50.0);
            f.state.pump_capture_windows(&f.events);
        }
        assert!(!f.state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
        let windows_before = f.state.windows_admitted;
        let mut reclosed = UtteranceLedger::new();
        for (start, end) in [(227_328, 510_464), (2_132_992, 2_317_824)] {
            reclosed.open_or_extend(SESSION, 1, start, end);
            reclosed.close_open(end);
        }
        // The contained re-close neither mints nor schedules. Its Apple word
        // reaches the original horizon-open owner by midpoint.
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &reclosed,
            &[apple_word("contained", 2_153_000, 2_157_000)],
        ));
        f.state.pump_capture_windows(&f.events);
        assert_eq!(f.state.windows_admitted, windows_before);
        assert!(f.requests.try_recv().is_err());
        assert_eq!(
            f.state
                .acoustic_ledger
                .lock()
                .unwrap()
                .qualified_occurrences()
                .count(),
            2
        );

        // The neighbouring closure owns its full exclusive span at mint time,
        // including the physical overlap with the earlier fixed identity.
        let next_owner = OccurrenceIdentity::new(SESSION, 1, 2_317_824, 2_378_240);
        reclosed.open_or_extend(SESSION, 1, next_owner.sample_start, next_owner.sample_end);
        reclosed.close_open(next_owner.sample_end);
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &reclosed,
            &[apple_word("late", 2_320_000, 2_324_000)],
        ));
        capture_more(&mut f, &vec![0.2; 17_000]);
        f.state.pump_capture_windows(&f.events);
        let requests = std::iter::from_fn(|| f.requests.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        let mut occurrences = requests[0]
            .member_occurrences
            .iter()
            .map(|(_, owner)| owner.clone())
            .collect::<Vec<_>>();
        occurrences.sort_by_key(|owner| owner.sample_start);
        assert_eq!(occurrences, vec![owner.clone(), next_owner.clone()]);
        assert_eq!(
            f.state.windows_admitted,
            windows_before + requests.len() as u64
        );
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        let owners = ledger.qualified_occurrences().collect::<Vec<_>>();
        assert_eq!(owners.len(), 3);
        assert!(owners.contains(&&owner));
        assert!(owners.contains(&&next_owner));
        let overlaps = owners
            .iter()
            .enumerate()
            .flat_map(|(index, first)| {
                owners.iter().skip(index + 1).filter_map(move |second| {
                    let start = first.sample_start.max(second.sample_start);
                    let end = first.sample_end.min(second.sample_end);
                    (start < end).then_some((start, end))
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(overlaps, vec![(2_317_824, 2_325_504)]);
        assert_eq!(ledger.text_of(&owner), Some("anchor contained"));
        assert_eq!(ledger.text_of(&next_owner), Some("late"));
        assert!(
            ledger
                .frontier_of(&owner)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        assert!(!ledger.is_sealed(&owner));
        assert_eq!(ledger.conservation().residue(), 0);
    }

    /// ef1fa physical closures overlap at 508416..510464. Labels and PCM are
    /// synthetic; both live-window and stop recovery must make the same choice.
    #[test]
    fn midpoint_in_closure_overlap_routes_to_exactly_one_owner() {
        // This is the routing matrix. Full native scheduling/Stop/sealing is
        // exercised separately with authenticated capture and actual grid jobs.
        for (older_sealed, newer_sealed) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let mut state = AppleSealState::new_for_session(RATE, SESSION.into(), 1);
            state.energy_calibration = Some(EnergyCalibration::new("overlap-routing", 1.0, 1));
            state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
            let mut pcm = vec![0.00001; 864_000];
            pcm[246_464..262_784].fill(0.2);
            pcm[652_416..659_136].fill(0.2);
            state.audio.push(&pcm);
            CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE)
                .push_samples(&pcm);
            let older = OccurrenceIdentity::new(SESSION, 1, 227_328, 510_464);
            let newer = OccurrenceIdentity::new(SESSION, 1, 508_416, 743_424);
            for (owner, label, sealed) in [
                (&older, "older", older_sealed),
                (&newer, "newer", newer_sealed),
            ] {
                assert!(qualify_owned_occurrence(&state, owner));
                let mut ledger = state.acoustic_ledger.lock().unwrap();
                ledger.schedule_frontier(owner.clone(), [LedgerObservationProducer::Apple]);
                let observation = LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    owner.sample_start,
                    0,
                    owner.clone(),
                );
                state.bind_apple_word_measurement(&mut ledger, owner);
                let (start, end) = if owner == &older {
                    (246_464, 262_784)
                } else {
                    (652_416, 659_136)
                };
                assert!(
                    ledger
                        .admit_word_slots(
                            &observation,
                            &[crate::pipeline::acoustic_ledger::WordPin::new(
                                start, end, label
                            )]
                        )
                        .grants_mutation()
                );
                ledger.note_frontier_return(owner, LedgerObservationProducer::Apple);
                assert!(ledger.frontier_of(owner).unwrap().is_closed());
                if sealed {
                    ledger.seal(owner).expect("canonical closed routing input");
                }
            }
            let (tx, _rx) = mpsc::unbounded_channel();
            let members = vec![(1, older.clone()), (2, newer.clone())];
            let winner = if !older_sealed && newer_sealed {
                &older
            } else {
                &newer
            };
            for producer in [
                LedgerObservationProducer::Apple,
                LedgerObservationProducer::Whisper,
            ] {
                let routes = state.route_overlap_pins(
                    &tx,
                    AdmitWindow {
                        request_id: 77,
                        sample_start: 432_000,
                        sample_end: 576_000,
                    },
                    &members,
                    &[pin("overlap", 509_400, 509_800)],
                    producer,
                );
                let owned = routes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, r)| (!r.exclusive.is_empty()).then_some(&members[i].1))
                    .collect::<Vec<_>>();
                // Routing retains the canonical recipient even when sealed;
                // the admission owner emits its immutable-source refusal later.
                assert_eq!(owned, vec![winner]);
                assert!(routes.iter().map(|r| r.exclusive.len()).sum::<usize>() == 1);
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert_eq!(ledger.text_of(&older), Some("older"));
                assert_eq!(ledger.text_of(&newer), Some("newer"));
                assert_eq!(ledger.is_sealed(&older), older_sealed);
                assert_eq!(ledger.is_sealed(&newer), newer_sealed);
                assert_eq!(ledger.qualified_occurrences().count(), 2);
                assert_eq!(ledger.conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn late_apple_word_fills_open_owner_and_sealed_owner_gets_k5() {
        for sealed in [false, true] {
            let mut pcm = vec![0.0; 719_000];
            pcm[350_000..366_000].fill(0.2);
            if !sealed {
                pcm[434_496..470_976].fill(0.2);
            }
            let mut f = fixture_with_pcm(pcm, true);
            assert!(reconcile_silero_ledger(
                &mut f.state,
                &f.events,
                &f.physical,
                &[apple_word("apple", 350_000, 366_000)]
            ));
            let head = f.state.audio.session_sample_end();
            let quiet = vec![0.0; 720_000 - head as usize];
            f.state.audio.push(&quiet);
            f.capture.as_mut().unwrap().push_samples(&quiet);
            f.state.pump_capture_windows(&f.events);
            let mut returns = 0;
            while let Ok(request) = f.requests.try_recv() {
                let result = completion(&request, vec![pin("whisper", 350_000, 366_000)]);
                result.payload.as_ref().unwrap().validate().unwrap();
                f.state.complete_whisper_window(&f.events, result, 15.0);
                returns += 1;
            }
            assert_eq!(
                returns, 3,
                "three independent captured frames corroborate the word"
            );
            if sealed {
                let head = f.state.audio.session_sample_end();
                let quiet = vec![0.0; (864_000 - head) as usize];
                f.state.audio.push(&quiet);
                for chunk in quiet.chunks(960) {
                    f.capture.as_mut().unwrap().push_samples(chunk);
                }
                assert_eq!(
                    coverage_speech_evidence(&f.state)
                        .availability()
                        .observed_samples(),
                    Some(864_000)
                );
                f.state.pump_capture_windows(&f.events);
                while let Ok(request) = f.requests.try_recv() {
                    f.state
                        .complete_whisper_window(&f.events, completion(&request, vec![]), 18.0);
                }
                assert!(f.state.window_plan.admission_horizon() >= f.occurrence.sample_end);
            }
            let before = f
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_of(&f.occurrence)
                .cloned();
            assert_eq!(
                before.is_some(),
                sealed,
                "the sealed branch requires a real ledger receipt"
            );
            assert!(reconcile_silero_ledger(
                &mut f.state,
                &f.events,
                &f.physical,
                &[
                    apple_word("late", 434_496, 470_976),
                    apple_word("must not overwrite", 350_000, 366_000),
                ]
            ));
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.text_of(&f.occurrence),
                Some(if sealed { "whisper" } else { "whisper late" })
            );
            assert_eq!(ledger.seal_of(&f.occurrence), before.as_ref());
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
            drop(ledger);
            if sealed {
                let events = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
                assert!(events.iter().any(|event| matches!(
                    event,
                    EngineEvent::LedgerMutation {
                        receipt: MutationReceipt::KeepVisibleUnanchored {
                            reason: NoAuthorityReason::LateAppleWordSealedOwner,
                            ..
                        },
                        ..
                    }
                )));
            }
        }
    }

    #[test]
    fn partial_apple_revision_retains_one_source_and_its_unresolved_alternative() {
        let mut f = fixture();
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[apple_word("one", 246_464, 290_000)]
        ));
        let before = f
            .state
            .acoustic_ledger
            .lock()
            .unwrap()
            .slots_of(&f.occurrence)
            .unwrap()
            .to_vec();
        assert_eq!(before.len(), 1);
        while f.receiver.try_recv().is_ok() {}
        // A later partial clock does not establish a complete replacement for
        // the earlier native source. Keep the candidate and the original PCM.
        assert!(reconcile_silero_ledger(
            &mut f.state,
            &f.events,
            &f.physical,
            &[apple_word("two", 262_784, 300_000)]
        ));
        let ledger = f.state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&f.occurrence), Some("one"));
        assert_eq!(ledger.slots_of(&f.occurrence).unwrap(), before.as_slice());
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .any(|alternative| alternative.candidate == "two"
                    && alternative.sources.contains(&before[0])),
            "the rejected revision retains its actual competing source: {:#?}",
            ledger.slot_alternatives()
        );
        assert!(
            ledger
                .layer_trail()
                .iter()
                .any(|entry| entry.candidate_label == "two" && !entry.decision.grants_mutation())
        );
        assert_eq!(ledger.conservation().residue(), 0);
        ledger.assert_slot_labels();
        drop(ledger);
        let receipts = std::iter::from_fn(|| f.receiver.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation { receipt, .. } => Some(receipt),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!receipts.is_empty());
        assert!(receipts.iter().all(|receipt| !receipt.grants_mutation()));
    }

    #[test]
    fn stop_recovery_routes_ef1fa_pad_words_to_held_owner() {
        // A recovery decode starts on a burst boundary. Room tone fills the
        // four 20 ms hops before the first pad word's speech; without that
        // pause, continuous speech before 126 464 leaves `pad1` possibly cut
        // at the window start, and the ledger rightly waits for a window that
        // hears it whole.
        let mut pcm = vec![0.2; 719_000];
        pcm[122_880..126_720].fill(0.004);
        for sealed in [false, true] {
            let mut f = fixture_with_pcm(pcm.clone(), true);
            let held = OccurrenceIdentity::new(SESSION, 1, 56_832, 222_208);
            let mut physical = UtteranceLedger::new();
            physical.open_or_extend(SESSION, 1, held.sample_start, held.sample_end);
            physical.close_open(held.sample_end);
            physical.open_or_extend(
                SESSION,
                1,
                f.occurrence.sample_start,
                f.occurrence.sample_end,
            );
            physical.close_open(f.occurrence.sample_end);
            assert!(reconcile_silero_ledger(
                &mut f.state,
                &f.events,
                &physical,
                &[apple_word("held", 58_000, 70_000)]
            ));
            if sealed {
                f.state.return_outstanding_whisper_without_label(&f.events);
                assert!(f.state.acoustic_ledger.lock().unwrap().is_sealed(&held));
            }
            let before = f
                .state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_of(&held)
                .cloned();
            let request = f.requests.try_recv().unwrap();
            let mut payload = completion(
                &request,
                vec![
                    pin("pad1", 126_464, 164_864),
                    pin("pad2", 164_864, 174_464),
                    pin("pad3", 174_464, 246_464),
                    pin("debt", 250_000, 265_000),
                ],
            )
            .payload
            .unwrap();
            // Authentic bounded independent DT scope from terminal_witness_windows:
            // pad group at 56832..222208 and neighbor through510464, 3s context.
            f.state.apple_live_enabled = true;
            payload.identity.request_id = (1_u64 << 62) + 1;
            payload.identity.range.sample_start = 0;
            payload.identity.range.sample_end = 654_464;
            payload.provider_id = crate::stt::tail_provider::TailProviderId::AppleDictation;
            payload.evidence.source = crate::stt::tail_provider::TailEvidenceSource::AppleDictation;
            payload.evidence.segment_grain = crate::stt::tail_provider::TailSegmentGrain::Phrase;
            for segment in &mut payload.segments {
                segment.grain = crate::stt::tail_provider::TailSegmentGrain::Phrase;
            }
            payload.validate().unwrap();
            let scope = f.state.window_by_samples(0, 654_464).unwrap();
            assert_eq!(scope.samples, pcm[..654_464]);
            let identity = payload.identity.clone();
            admit_dictation_witness_payload(
                &mut f.state,
                &f.events,
                &identity,
                &payload,
                &[held.clone(), f.occurrence.clone()],
                &[],
                12_000,
            );
            let events = std::iter::from_fn(|| f.receiver.try_recv().ok()).collect::<Vec<_>>();
            assert!(!events.iter().any(|event| matches!(event,
                EngineEvent::Warning { code, .. } if code == "seal_coverage_text_recovery_refused")));
            assert!(!events.iter().any(|event| matches!(
                event,
                EngineEvent::LedgerMutation {
                    receipt: MutationReceipt::KeepVisibleUnanchored {
                        reason: NoAuthorityReason::OverlapWithoutWordPins,
                        ..
                    },
                    ..
                }
            )));
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.seal_of(&held), before.as_ref());
            if sealed {
                assert_eq!(
                    ledger.text_of(&held),
                    Some("held"),
                    "DT cannot reopen a sealed owner"
                );
            } else {
                assert_eq!(ledger.text_of(&held), Some("held pad1 pad2 pad3"));
            }
            assert!(ledger.text_of(&f.occurrence).unwrap().contains("debt"));
            ledger.assert_slot_labels();
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    /// A jittered re-decode of a spoken word lies on the same measured burst,
    /// so it is the same acoustic word (D4): the later copy is a replay of
    /// that range and the surface the first completion admitted stays, as
    /// case and punctuation equivalence does on a seam (58f4b33c).
    #[test]
    fn jittered_replay_keeps_one_slot_and_the_first_surface_in_either_completion_order() {
        for reversed in [false, true] {
            let mut f = fixture_for_words(&[
                pin("Alpha,", 246_464, 262_784),
                pin("delta", 434_496, 470_976),
            ]);
            let first = f.requests.try_recv().unwrap();
            let second = f.requests.try_recv().unwrap();
            let mut results = vec![
                completion(&first, vec![pin("Alpha,", 246_464, 262_784)]),
                completion(
                    &second,
                    vec![
                        pin("alpha", 248_000, 266_000),
                        pin("delta", 434_496, 470_976),
                    ],
                ),
            ];
            if reversed {
                results.reverse();
            }
            for result in results {
                f.state.complete_whisper_window(&f.events, result, 10.7);
            }
            // The owning first frame wins independently of completion order.
            capture_more(&mut f, &[0.00001; 1000]);
            f.state.pump_capture_windows(&f.events);
            let owning = f.requests.try_recv().unwrap();
            assert_eq!(owning.provider_request.identity.range.sample_start, 288_000);
            f.state.complete_whisper_window(
                &f.events,
                completion(&owning, vec![pin("delta", 434_496, 470_976)]),
                15.0,
            );
            let (winner, loser, kept) = ("Alpha, delta", "alpha", (246_464, 262_784));
            assert_words(&f, winner);
            let ledger = f.state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.slots_of(&f.occurrence).unwrap().len(), 2);
            // The replay refusal is filed on the kept word's range.
            let trail = || {
                ledger
                    .layer_trail()
                    .iter()
                    .filter(|entry| entry.observation.occurrence.same_capture(&f.occurrence))
            };
            assert!(!trail().any(|entry| {
                entry.candidate_label == loser
                    && matches!(
                        entry.decision,
                        MutationReceipt::Refuse {
                            reason: RefuseReason::ReplayedRangeIdentity,
                            ..
                        }
                    )
            }));
            assert!(
                ledger
                    .slots_of(&f.occurrence)
                    .unwrap()
                    .iter()
                    .any(|slot| (slot.sample_start, slot.sample_end) == kept),
                "the kept word keeps the PCM its first completion declared: {kept:?}"
            );
            assert!(
                !trail().any(|entry| matches!(
                    entry.decision,
                    MutationReceipt::Refuse {
                        reason: RefuseReason::ReplacedByWhisper,
                        ..
                    }
                )),
                "a replay of one word is not a correction of it"
            );
        }
    }

    #[test]
    fn normal_single_window_apple_label_seals_at_hands_free_epoch_close() {
        let (events, mut receiver) = mpsc::unbounded_channel();
        let (sender, mut requests) = mpsc::channel(8);
        let mut state = AppleSealState::new_with_tail_patch_for_session(
            RATE,
            SESSION.into(),
            1,
            sender,
            Arc::new(Mutex::new(AcousticLedger::new())),
            Some(EnergyCalibration {
                version: "single-window".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            }),
        );
        state.audio.push(&vec![0.2; 192_000]);
        let owner = OccurrenceIdentity::new(SESSION, 1, 48_000, 96_000);
        let mut physical = UtteranceLedger::new();
        physical.open_or_extend(SESSION, 1, 48_000, 96_000);
        physical.close_open(96_000);
        assert!(reconcile_silero_ledger(
            &mut state,
            &events,
            &physical,
            &[apple_word("normal", 60_000, 80_000)]
        ));
        assert!(!state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
        let mut epoch = EpochGate::armed(RATE, 1.0);
        assert!(matches!(
            epoch.feed_pcm(&[0.2; 480], 96_000, true),
            EpochDecision::Wake { .. }
        ));
        assert!(matches!(
            epoch.feed_pcm(&vec![0.0; 48_000], 144_000, false),
            EpochDecision::Sleep { .. }
        ));
        state.capture_stopping = true;
        state.pump_capture_windows(&events);
        let request = requests.try_recv().unwrap();
        assert!(requests.try_recv().is_err());
        state.complete_whisper_window(
            &events,
            completion(&request, vec![pin("normal", 60_000, 80_000)]),
            2.0,
        );
        state.close_admission_horizon(&events, 144_000);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("normal"));
        assert!(ledger.is_sealed(&owner));
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
        assert!(
            !std::iter::from_fn(|| receiver.try_recv().ok()).any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "live_refinement_stop_deadline"))
        );
    }
