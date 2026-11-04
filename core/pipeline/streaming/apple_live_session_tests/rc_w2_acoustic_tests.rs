/// W2 acoustic checkpoint: the terminal coverage entrypoints, the capture
/// energy fallback, and ledger admission — exercised through the production
/// functions rather than through a helper called twice.
///
/// Nothing here revives a retired production helper to satisfy an old test.
    use super::*;
    use crate::audio::capture_receipt::{
        AcousticAvailability, CAPTURE_ENERGY_PRODUCER, CaptureEvidenceIdentity,
    };
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
    use crate::pipeline::acoustic_ledger::{AcousticEvidenceGap, RefuseReason};

    const RATE: u32 = 16_000;
    /// `ACOUSTIC_SPEECH_PAD_SECS` (64 ms) at [`RATE`].
    const PAD: u64 = 1_024;
    /// `SEAL_COVERAGE_INCOMPLETE_MS` (250 ms) at [`RATE`].
    const THRESHOLD: u64 = 4_000;
    /// Calibrated existence floor of the fixture's capture device. Fixture
    /// speech is 0.25 RMS and fixture silence is digital zero.
    const EXISTENCE_FLOOR_RMS: f32 = 0.01;

    // capture_receipt owns a per-test-thread clock for every energy consumer.
    // No module-local mutex can protect a process-global reset/feed/read sequence.

    fn at(secs: f32) -> u64 {
        (secs * RATE as f32) as u64
    }

    fn state_for(session: &str, capture_secs: f32) -> AppleSealState {
        let mut state = AppleSealState::new_for_session(RATE, session.into(), 0);
        state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, 1));
        state.existence_floor_rms = Some(EXISTENCE_FLOOR_RMS);
        if capture_secs > 0.0 {
            state
                .audio
                .push(&vec![0.25f32; (capture_secs * RATE as f32) as usize]);
        }
        state
    }

    /// [`state_for`] whose capture also reached the energy owner, the way a
    /// recorder delivers it: the same PCM, read against the calibrated floor.
    fn measured_state_for(session: &str, capture_secs: f32) -> AppleSealState {
        let mut state = state_for(session, 0.0);
        let pcm = vec![0.25f32; (capture_secs * RATE as f32) as usize];
        state.audio.push(&pcm);
        CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE).push_samples(&pcm);
        state
    }

    // Root-owned stop controls: capture, measurement and decode share one PCM.
    fn forensic_stop_recovery_capture(
        session: &str,
        unowned_burst: bool,
    ) -> (AppleSealState, Vec<OccurrenceIdentity>, Vec<f32>) {
        let stride = at(3.5);
        let gap_start = 5 * stride + at(0.5);
        let gap_end = gap_start + at(1.0);
        let end = if unowned_burst {
            gap_end
        } else {
            4 * stride + at(2.0)
        } + at(0.5);
        let mut pcm = vec![0.00001; end as usize];
        for index in 0..5 {
            let start = index * stride + at(0.6);
            let end = index * stride + at(1.2);
            pcm[start as usize..end as usize].fill(0.25);
        }
        if unowned_burst {
            pcm[gap_start as usize..gap_end as usize].fill(0.25);
        }
        let mut state = state_for(session, 0.0);
        state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let speech = coverage_speech_evidence(&state);
        assert_eq!(speech.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(speech.availability().observed_samples(), Some(end));
        assert_eq!(speech.ranges().len(), 5 + usize::from(unowned_burst));
        let (tx, _rx) = mpsc::unbounded_channel();
        let owners = (0..5)
            .map(|index| {
                let start = index * stride;
                commit_debt_occurrence(
                    &mut state,
                    &tx,
                    start,
                    start + at(2.0),
                    &format!("apple{index}"),
                )
            })
            .collect();
        (state, owners, pcm)
    }

    fn forensic_stop_recovery_word(
        request: &TailProviderRequest,
        owner: &OccurrenceIdentity,
    ) -> TailProviderPayload {
        let mut payload = gap_payload(request);
        payload.segments[0].grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        payload.segments[0].range.sample_start = owner.sample_start + at(0.6);
        payload.segments[0].range.sample_end = owner.sample_start + at(1.2);
        payload.evidence.segment_grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        payload
    }

    #[test]
    fn forensic_stop_recovery_words_settle_each_source_after_live_deadline() {
        for expired in [false, true] {
            for unowned in [false, true] {
                let (mut state, owners, _pcm) =
                    forensic_stop_recovery_capture("stop-word-scope", unowned);
                let sources = {
                    let ledger = state.acoustic_ledger.lock().unwrap();
                    owners
                        .iter()
                        .map(|owner| ledger.slots_of(owner).unwrap()[0].clone())
                        .collect::<Vec<_>>()
                };
                if expired {
                    let deadline = LocalExecutionOwner::default().begin_drain(Duration::ZERO);
                    assert!(deadline <= Instant::now());
                }
                let (tx, mut rx) = mpsc::unbounded_channel();
                let unowned_gap = unowned_speech_gap(&state, &owners);
                let _offered = settle_claimed_windows(
                    &mut state,
                    &tx,
                    &owners,
                    unowned_gap,
                    |request, fresh| Some(iwo_words(request, fresh)),
                );
                let receipt = reconcile_terminal_coverage(&mut state, &tx);
                assert_eq!(
                    receipt.status,
                    SealCoverageStatus::Complete,
                    "expired={expired}/unowned={unowned}: {receipt:?}"
                );
                assert_eq!(receipt.coverage_ratio(), Some(1.0));
                let mut ledger = state.acoustic_ledger.lock().unwrap();
                assert!(
                    ledger
                        .pending_text_recoveries(&state.session_id, state.capture_epoch)
                        .is_empty()
                );
                for (owner, source) in owners.iter().zip(&sources) {
                    assert!(ledger.is_sealed(owner));
                    assert_eq!(ledger.text_of(owner), Some("Iwo"));
                    let slots = ledger.slots_of(owner).unwrap();
                    assert_eq!(slots.len(), 1);
                    assert_eq!(
                        (slots[0].sample_start, slots[0].sample_end),
                        (owner.sample_start + at(0.6), owner.sample_start + at(1.2))
                    );
                    assert_eq!(slots[0].producer, LedgerObservationProducer::Whisper);
                    let accounting = ledger
                        .slot_operations()
                        .iter()
                        .filter(|op| {
                            op.observation.producer == LedgerObservationProducer::Whisper
                                && op.sources.contains(source)
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(
                        accounting.len(),
                        1,
                        "each physical source is accounted once"
                    );
                    assert!(accounting[0].source_ranges.contains(owner));
                    assert!(accounting[0].outputs.contains(&slots[0]));
                }
                assert!(
                    ledger
                        .seal_terminal(&state.session_id, state.capture_epoch)
                        .is_ok()
                );
                assert_eq!(ledger.conservation().residue(), 0);
                drop(ledger);
                let seals = std::iter::from_fn(|| rx.try_recv().ok())
                    .filter_map(|event| match event {
                        EngineEvent::LedgerSeal { receipt } => Some(receipt),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(seals.len(), 5 + usize::from(unowned));
                assert_eq!(state.session_conservation().residue(), 0);
            }
        }
    }

    #[test]
    fn forensic_stop_recovery_unproved_returns_keep_source_debt() {
        for expired in [false, true] {
            for defect in [
                "phrase",
                "segmentless",
                "foreign_envelope",
                "foreign_pin",
                "reversed",
                "empty",
                "outside",
                "failed",
            ] {
                let (mut state, owners, _pcm) =
                    forensic_stop_recovery_capture("stop-unproved-scope", false);
                if expired {
                    let deadline = LocalExecutionOwner::default().begin_drain(Duration::ZERO);
                    assert!(deadline <= Instant::now());
                }
                let (tx, mut rx) = mpsc::unbounded_channel();
                let _offered =
                    settle_claimed_windows(&mut state, &tx, &owners, None, |request, fresh| {
                        defect_payload(request, fresh, defect)
                    });
                if defect == "phrase" {
                    assert!(
                        !state.whisper_span_refused.contains(&owners[4]),
                        "an earlier partial intersection must not veto the containing tail frame"
                    );
                }
                let receipt = reconcile_terminal_coverage(&mut state, &tx);
                assert_eq!(
                    receipt.status,
                    SealCoverageStatus::Incomplete,
                    "{expired}/{defect}: {receipt:?}"
                );
                let mut ledger = state.acoustic_ledger.lock().unwrap();
                assert_eq!(
                    ledger.pending_text_recoveries(&state.session_id, state.capture_epoch),
                    owners
                );
                for (index, owner) in owners.iter().enumerate() {
                    assert!(ledger.text_recovery_pending(owner));
                    assert!(!ledger.is_sealed(owner));
                    let expected = format!("apple{index}");
                    assert_eq!(
                        ledger.text_of(owner),
                        Some(expected.as_str()),
                        "{expired}/{defect}"
                    );
                }
                assert_eq!(
                    ledger.seal_terminal(&state.session_id, state.capture_epoch),
                    Err(SealRefusal::TextRecoveryPending)
                );
                assert_eq!(ledger.conservation().residue(), 0);
                drop(ledger);
                assert!(
                    !std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
                        event,
                        EngineEvent::LedgerSeal { .. } | EngineEvent::UtteranceFinal { .. }
                    ))
                );
                assert_eq!(state.session_conservation().residue(), 0);
            }
        }
    }

    fn crossing(kind: VadBoundaryKind, sample: u64) -> VadBoundaryEvidence {
        VadBoundaryEvidence {
            kind,
            sample,
            speech_probability: match kind {
                VadBoundaryKind::SpeechStart => 0.9,
                VadBoundaryKind::SpeechEnd => 0.1,
            },
        }
    }

    /// One second of speech inside a ten-second take, plus the padded ownership
    /// window the fusion ledger legitimately mints across the whole take.
    ///
    /// The two sets disagree by an order of magnitude on purpose: every
    /// assertion below is only meaningful because a regression to the padded
    /// set would change the answer.
    fn one_burst_in_a_ten_second_take(session: &str) -> AppleSealState {
        let mut state = state_for(session, 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        // The same ten seconds `state_for` pushed into the audio buffer reached
        // the VAD. Synthetic crossings still stand in for the model read.
        ingress.note_observed_pcm(at(10.0), at(10.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        ]);
        ingress.observe(Some((0, at(10.0))), true, at(10.0));
        state.fusion = Some(ingress);
        state
    }

    fn two_bursts(session: &str) -> AppleSealState {
        let mut state = state_for(session, 10.0);
        let mut ingress = SileroIngress::new(RATE, session, 0);
        ingress.note_observed_pcm(at(10.0), at(10.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
            crossing(VadBoundaryKind::SpeechStart, at(4.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(5.0)),
        ]);
        ingress.observe(Some((0, at(10.0))), true, at(10.0));
        state.fusion = Some(ingress);
        state
    }

    fn gap_payload(request: &TailProviderRequest) -> TailProviderPayload {
        use crate::stt::tail_provider::{
            TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
            TailTimingQuality,
        };
        TailProviderPayload {
            identity: request.identity.clone(),
            text: "Iwo".into(),
            segments: vec![TimedTailSegment {
                confidence: None,
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                text: "Iwo".into(),
                range: request.identity.range.clone(),
            }],
            avg_logprob: None,
            compression_ratio: None,
            provider_id: TailProviderId::Fake,
            elapsed_ms: 0,
            evidence: TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: None,
                vad_selected_samples: None,
            },
        }
    }

    pub(super) fn offer_stopping_windows(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ) -> Vec<TailPatchRequest> {
        let (tail_tx, mut tail_rx) = mpsc::channel(16);
        state.tail_patch = Some(tail_tx);
        state.capture_stopping = true;
        state.pump_capture_windows(ev_tx);
        let mut offered = Vec::new();
        while let Ok(request) = tail_rx.try_recv() {
            let buffered = state
                .window_by_samples(request.admit_sample_start, request.admit_sample_end)
                .expect("planned window is inside the capture buffer");
            assert_eq!(
                request.audio, buffered.samples,
                "the offer carries the captured PCM"
            );
            offered.push(request);
        }
        offered
    }

    pub(super) fn complete_planned_window(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request: &TailPatchRequest,
        payload: Option<TailProviderPayload>,
    ) {
        state.complete_whisper_window(
            ev_tx,
            TailPatchCompletion {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity.clone()),
                payload,
                member_occurrences: request.member_occurrences.clone(),
            },
            0.0,
        );
    }

    pub(super) fn window_contains(request: &TailPatchRequest, start: u64, end: u64) -> bool {
        start >= request.admit_sample_start && end <= request.admit_sample_end && start < end
    }

    fn iwo_words(request: &TailPatchRequest, owners: &[OccurrenceIdentity]) -> TailProviderPayload {
        let mut payload = forensic_stop_recovery_word(&request.provider_request, &owners[0]);
        payload.segments = owners
            .iter()
            .map(|owner| {
                forensic_stop_recovery_word(&request.provider_request, owner)
                    .segments
                    .remove(0)
            })
            .collect();
        payload.text = owners.iter().map(|_| "Iwo").collect::<Vec<_>>().join(" ");
        payload
    }

    /// One exact word outside every owner. `None` would mark those owners as
    /// text-recovery again; this pin is not speech and not an exclusive label.
    fn silence_word(
        request: &TailPatchRequest,
        owners: &[OccurrenceIdentity],
    ) -> TailProviderPayload {
        let mut payload = gap_payload(&request.provider_request);
        payload.evidence.segment_grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        let mut start = request.admit_sample_start;
        let mut chosen = None;
        while start.saturating_add(1_600) <= request.admit_sample_end {
            let end = start + 1_600;
            let overlaps_owner = owners
                .iter()
                .any(|owner| start < owner.sample_end && owner.sample_start < end);
            if !overlaps_owner {
                chosen = Some((start, end));
                break;
            }
            start += 1_600;
        }
        let (start, end) = chosen.expect("planned window has a span outside every owner");
        payload.segments[0].grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        payload.segments[0].range.sample_start = start;
        payload.segments[0].range.sample_end = end;
        payload.segments[0].text = "quiet".into();
        payload.text = "quiet".into();
        payload
    }

    fn unowned_speech_gap(
        state: &AppleSealState,
        owners: &[OccurrenceIdentity],
    ) -> Option<TailSampleRange> {
        coverage_speech_evidence(state)
            .ranges()
            .iter()
            .find(|gap| {
                !owners.iter().any(|owner| {
                    gap.sample_start >= owner.sample_start && gap.sample_end <= owner.sample_end
                })
            })
            .cloned()
    }

    /// Offer the actual measured owner words in every containing grid result.
    /// The publication owner selects each pin; context copies carry no vote.
    /// Include an unowned measured gap word in its containing result too.
    fn settle_claimed_windows(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        owners: &[OccurrenceIdentity],
        gap: Option<TailSampleRange>,
        mut claim: impl FnMut(&TailPatchRequest, &[OccurrenceIdentity]) -> Option<TailProviderPayload>,
    ) -> Vec<TailPatchRequest> {
        let offered = offer_stopping_windows(state, ev_tx);
        let mut gap_sent = false;
        for request in &offered {
            let mut fresh = Vec::new();
            for owner in owners {
                if window_contains(request, owner.sample_start, owner.sample_end) {
                    fresh.push(owner.clone());
                }
            }
            let send_gap = !gap_sent
                && gap
                    .as_ref()
                    .is_some_and(|gap| window_contains(request, gap.sample_start, gap.sample_end));
            if send_gap {
                gap_sent = true;
                let mut gap_word = gap_payload(&request.provider_request).segments.remove(0);
                gap_word.range = gap.clone().expect("send_gap");
                gap_word.grain = crate::stt::tail_provider::TailSegmentGrain::Word;
                let mut payload = if fresh.is_empty() {
                    let mut payload = gap_payload(&request.provider_request);
                    payload.evidence.segment_grain =
                        crate::stt::tail_provider::TailSegmentGrain::Word;
                    payload.segments.clear();
                    payload
                } else {
                    claim(request, &fresh)
                        .expect("successful owner return when appending unowned word")
                };
                payload.segments.push(gap_word);
                payload.text = payload
                    .segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                payload.validate().unwrap();
                complete_planned_window(state, ev_tx, request, Some(payload));
                continue;
            }
            if fresh.is_empty() {
                complete_planned_window(state, ev_tx, request, Some(silence_word(request, owners)));
                continue;
            }
            complete_planned_window(state, ev_tx, request, claim(request, &fresh));
        }
        assert!(
            owners.iter().all(|owner| {
                offered
                    .iter()
                    .any(|request| window_contains(request, owner.sample_start, owner.sample_end))
            }),
            "every owner is wholly inside an offered window"
        );
        if let Some(gap) = &gap {
            assert!(
                offered
                    .iter()
                    .any(|request| { window_contains(request, gap.sample_start, gap.sample_end) }),
                "unowned speech is wholly inside an offered window"
            );
        }
        offered
    }

    fn defect_payload(
        request: &TailPatchRequest,
        owners: &[OccurrenceIdentity],
        defect: &str,
    ) -> Option<TailProviderPayload> {
        if defect == "failed" {
            return None;
        }
        let mut payload = iwo_words(request, owners);
        match defect {
            "phrase" => {
                payload.evidence.segment_grain =
                    crate::stt::tail_provider::TailSegmentGrain::Phrase;
                for segment in &mut payload.segments {
                    segment.grain = crate::stt::tail_provider::TailSegmentGrain::Phrase;
                }
            }
            "segmentless" => payload.segments.clear(),
            "foreign_envelope" => payload.identity.request_id += 1,
            "foreign_pin" => {
                for segment in &mut payload.segments {
                    segment.range.capture_epoch += 1;
                }
            }
            "reversed" => {
                for segment in &mut payload.segments {
                    segment.range.sample_start = segment.range.sample_end + 1;
                }
            }
            "empty" => {
                payload.text.clear();
                for segment in &mut payload.segments {
                    segment.text.clear();
                }
            }
            "outside" => {
                if let Some(segment) = payload.segments.last_mut() {
                    segment.range.sample_end = request.admit_sample_end + 1;
                }
            }
            _ => unreachable!("unknown forensic defect"),
        }
        Some(payload)
    }

    #[test]
    fn completed_live_horizon_keeps_incumbents_and_never_redecodes_them_at_stop() {
        use crate::pipeline::acoustic_ledger::WordPin;
        use crate::stt::tail_provider::TailSegmentGrain;
        let mut state = state_for("expired-live-word-horizon", 0.0);
        let mut pcm = vec![0.0; at(10.0) as usize];
        for (start, end) in [(at(1.0), at(2.0)), (at(3.0), at(4.0))] {
            pcm[start as usize..end as usize].fill(0.25);
        }
        state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let owner = OccurrenceIdentity::new(&state.session_id, state.capture_epoch, 0, at(10.0));
        let trail_dir = tempfile::tempdir().unwrap();
        let trail = crate::pipeline::trail::TrailSink::open_in(
            trail_dir.path(),
            &state.session_id,
            state.capture_epoch,
            512,
        )
        .unwrap();
        assert!(qualify_owned_occurrence(&state, &owner));
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            ledger.schedule_frontier(owner.clone(), [LedgerObservationProducer::Whisper]);
            ledger.record_speech_evidence(&coverage_speech_evidence(&state));
            ledger.admit_word_slots(
                &LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    1,
                    0,
                    owner.clone(),
                ),
                &[
                    WordPin::new(at(1.0), at(2.0), "1286"),
                    WordPin::new(at(3.0), at(4.0), "nie"),
                ],
            );
        }
        state.pending_events.insert(
            1,
            PendingAppleSeal {
                occurrence: owner.clone(),
                raw_text: "1286 nie".into(),
                layer1_baseline: "1286 nie".into(),
                start_ts: 0.0,
                end_ts: 10.0,
                segments: Vec::new(),
            },
        );
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(16);
        state.tail_patch = Some(tail_tx);
        state.capture_stopping = true;
        state.pump_capture_windows(&tx);
        let mut offered = Vec::new();
        while let Ok(request) = tail_rx.try_recv() {
            assert_eq!(
                request.audio,
                pcm[request.admit_sample_start as usize..request.admit_sample_end as usize]
            );
            offered.push(request);
        }
        assert_eq!(offered.len(), 2);
        assert_eq!(
            (offered[0].admit_sample_start, offered[0].admit_sample_end),
            (0, at(9.0))
        );
        assert_eq!(
            (offered[1].admit_sample_start, offered[1].admit_sample_end),
            (at(3.0), at(10.0))
        );
        assert!(state.window_plan.is_finished());
        for (index, request) in offered.iter().enumerate() {
            let mut payload = gap_payload(&request.provider_request);
            payload.evidence.segment_grain = TailSegmentGrain::Word;
            payload.segments = [(at(1.0), at(2.0), "56"), (at(3.0), at(4.0), "tak")]
                .into_iter()
                .filter(|(start, end, _)| window_contains(request, *start, *end))
                .map(|(start, end, text)| TimedTailSegment {
                    confidence: None,
                    grain: TailSegmentGrain::Word,
                    text: text.into(),
                    range: TailSampleRange {
                        sample_start: start,
                        sample_end: end,
                        ..request.provider_request.identity.range.clone()
                    },
                })
                .collect();
            payload.text = payload
                .segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            payload.validate().unwrap();
            assert_eq!(payload.identity, request.provider_request.identity);
            complete_planned_window(&mut state, &tx, request, Some(payload));
            if index == 0 {
                state.close_admission_horizon(&tx, owner.sample_end);
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert!(ledger.has_word_conflicts());
                assert!(
                    !ledger.is_sealed(&owner),
                    "an accepted covering request has not returned"
                );
            }
        }
        state.close_admission_horizon(&tx, owner.sample_end);
        let expected_seal = {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger.is_sealed(&owner),
                "expired disputes must not hold the old owner open"
            );
            assert_eq!(ledger.text_of(&owner), Some("1286 nie"));
            assert!(ledger.next_word_trial(true).is_none());
            let seal = ledger.seal_of(&owner).unwrap().clone();
            assert_eq!(seal.word_finality.len(), 2);
            assert!(seal.word_finality.iter().all(|word| {
                word.unresolved
                    && word
                        .trial
                        .as_ref()
                        .is_some_and(|trial| trial.reason == "admission_horizon_closed")
            }));
            assert_eq!(ledger.conservation().residue(), 0);
            seal
        };
        let admitted = state.windows_admitted;
        reconcile_terminal_coverage(&mut state, &tx);
        state.pump_capture_windows(&tx);
        assert_eq!(
            state.windows_admitted, admitted,
            "Stop must not offer another decode"
        );
        assert!(
            tail_rx.try_recv().is_err(),
            "Stop must never decode expired disputes"
        );
        drop(trail);
        let rows = crate::pipeline::trail::read_trail(
            &crate::pipeline::trail::trail_path(trail_dir.path(), &state.session_id).unwrap(),
        )
        .unwrap();
        let replayed = crate::pipeline::trail::replay_decisions(&rows, |_, _, _| {}).unwrap();
        assert_eq!(replayed.text_of(&owner), Some("1286 nie"));
        assert_eq!(replayed.seal_of(&owner), Some(&expected_seal));
        assert_eq!(replayed.conservation().residue(), 0);
    }

    /// A long take whose owners stay unsealed behind the admission horizon:
    /// Whisper disputes Apple on every owner, every planned window returned,
    /// each returned word decode was already replayed, and text recovery is
    /// still owed. 4d7ae454 (33 min, 19 X 2026) ended with 197 such owners
    /// and the worker spending a full core walking them on every tick.
    fn stuck_recovery_take(
        owners: u64,
    ) -> (
        AppleSealState,
        mpsc::UnboundedSender<EngineEvent>,
        mpsc::UnboundedReceiver<EngineEvent>,
        OccurrenceIdentity,
    ) {
        use crate::pipeline::acoustic_ledger::WordPin;
        use crate::stt::tail_provider::TailSegmentGrain;
        // Eight disputed words per owner; the take averaged about thirteen.
        let stride = at(5.0);
        let pins = |index: u64| {
            let start = index * stride + at(1.0);
            (0..8_u64).map(move |word| {
                let at_word = start + at(0.2) + word * at(0.4);
                (
                    at_word,
                    at_word + at(0.3),
                    format!("jeden{word}"),
                    format!("raz{word}"),
                )
            })
        };
        let mut state = state_for("stuck-recovery-take", 0.0);
        let mut pcm = vec![0.0; (owners * stride + at(1.0)) as usize];
        // Stop holds the whole take; the live buffer would evict its start.
        state.audio = LiveAudioBuffer::new(RATE, pcm.len() as f32 / RATE as f32 + 1.0);
        for index in 0..owners {
            for (start, end, _, _) in pins(index) {
                pcm[start as usize..end as usize].fill(0.25);
            }
        }
        state.audio.push(&pcm);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        for chunk in pcm.chunks(320) {
            writer.push_samples(chunk);
        }
        let occurrences = (0..owners)
            .map(|index| {
                let start = index * stride + at(1.0);
                OccurrenceIdentity::new(
                    &state.session_id,
                    state.capture_epoch,
                    start,
                    start + at(3.6),
                )
            })
            .collect::<Vec<_>>();
        for (index, owner) in occurrences.iter().enumerate() {
            let index = index as u64;
            assert!(qualify_owned_occurrence(&state, owner));
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            ledger.schedule_frontier(owner.clone(), [LedgerObservationProducer::Whisper]);
            ledger.record_speech_evidence(&coverage_speech_evidence(&state));
            ledger.admit_word_slots(
                &LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    index + 1,
                    0,
                    owner.clone(),
                ),
                &pins(index)
                    .map(|(start, end, apple, _)| WordPin::new(start, end, apple))
                    .collect::<Vec<_>>(),
            );
            let whisper =
                ledger.next_word_observation(LedgerObservationProducer::Whisper, index + 1, owner);
            // A complete window over the owner would pay its text debt
            // (decision of Maciej, 2026-10-21). Stuck owners are owners no
            // returned window heard whole: this one misses the first 100 ms.
            ledger.admit_word_slots(
                &whisper,
                &pins(index)
                    .map(|(start, end, _, heard)| {
                        WordPin::new(start, end, heard)
                            .with_decode_window(owner.sample_start + at(0.1), owner.sample_end)
                    })
                    .collect::<Vec<_>>(),
            );
            ledger.require_text_recovery(owner);
            drop(ledger);
            state.pending_events.insert(
                index + 1,
                PendingAppleSeal {
                    occurrence: owner.clone(),
                    raw_text: String::new(),
                    layer1_baseline: String::new(),
                    start_ts: 0.0,
                    end_ts: 0.0,
                    segments: Vec::new(),
                },
            );
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(4096);
        state.tail_patch = Some(tail_tx);
        state.capture_stopping = true;
        state.pump_capture_windows(&tx);
        let mut offered = Vec::new();
        while let Ok(request) = tail_rx.try_recv() {
            offered.push(request);
        }
        assert!(offered.len() as u64 >= owners);
        for request in &offered {
            complete_planned_window(&mut state, &tx, request, None);
            let mut payload = gap_payload(&request.provider_request);
            payload.evidence.segment_grain = TailSegmentGrain::Word;
            payload.segments = (0..owners)
                .flat_map(pins)
                .filter(|(start, end, _, _)| window_contains(request, *start, *end))
                .map(|(start, end, _, heard)| TimedTailSegment {
                    confidence: None,
                    grain: TailSegmentGrain::Word,
                    text: heard,
                    range: TailSampleRange {
                        sample_start: start,
                        sample_end: end,
                        ..request.provider_request.identity.range.clone()
                    },
                })
                .collect();
            for owner in &occurrences {
                if range_overlaps_occurrence(&request.provider_request.identity.range, owner) {
                    state
                        .replayed_window_owners
                        .insert((request.provider_request.identity.request_id, owner.clone()));
                }
            }
            state.retained_word_decodes.push_back(payload);
        }
        let last = occurrences.last().unwrap().clone();
        state.close_admission_horizon(&tx, last.sample_end);
        (state, tx, rx, last)
    }

    /// Ten times the stuck owners must cost about ten times per tick, not a
    /// hundred: no walk over owners may also walk every component or every
    /// retained decode. The baseline 80c7f3c96 measured 38x, this cut 13x.
    ///
    /// Still linear in unsealed owners on every tick, and not fixed here:
    /// `unsealed_word_owners` (clone of every owner plus the pending-event map,
    /// several calls per tick), the `close_admission_horizon` sweep (frontier
    /// and trail lookups per owner), `publish_resolved_word_seals` (one seal
    /// attempt per owner) and `replay_completed_windows` (one ordered lookup
    /// per retained decode). Bounding these to the live window needs owners
    /// that stop being visited once settled; repaying their text recovery
    /// live removes most of them altogether.
    #[test]
    fn long_take_tick_cost_grows_with_owners_not_owners_times_components() {
        fn tick_cost(owners: u64) -> (std::time::Duration, usize, usize) {
            let (mut state, tx, mut rx, last) = stuck_recovery_take(owners);
            while rx.try_recv().is_ok() {}
            let (unsealed, components) = {
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert!(ledger.text_recovery_pending(&last));
                assert!(!ledger.is_sealed(&last));
                (
                    ledger.unsealed_qualified_occurrences().count(),
                    ledger.word_component_count().1,
                )
            };
            let started = std::time::Instant::now();
            for _ in 0..40 {
                state.replay_completed_windows(&tx);
                state.close_admission_horizon(&tx, last.sample_end);
            }
            let elapsed = started.elapsed();
            assert!(
                rx.try_recv().is_err(),
                "ticks over settled owners must not emit anything"
            );
            (elapsed, unsealed, components)
        }
        // The capture energy clock belongs to the test thread: one take each.
        let measured = |owners| {
            std::thread::spawn(move || tick_cost(owners))
                .join()
                .unwrap()
        };
        let rounds = (0..3)
            .map(|_| (measured(25), measured(250)))
            .collect::<Vec<_>>();
        let small = rounds.iter().map(|(small, _)| small.0).min().unwrap();
        let large = rounds.iter().map(|(_, large)| large.0).min().unwrap();
        let (_, small_unsealed, _) = rounds[0].0;
        let (_, large_unsealed, conflicts) = rounds[0].1;
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        eprintln!(
            "40 ticks: {small_unsealed} stuck owners {small:?}, {large_unsealed} stuck owners {large:?}, ratio {ratio:.1}, conflicting components {conflicts}"
        );
        assert_eq!(small_unsealed, 25);
        assert_eq!(large_unsealed, 250);
        assert!(
            ratio < 25.0,
            "10x the stuck owners cost {ratio:.1}x per tick: work grows with owners x components"
        );
    }

    #[test]
    fn stop_shares_one_provider_decode_between_covered_word_disputes() {
        // The forward grid cannot manufacture a third Whisper vote. One
        // independent DT result settles the shared physical word targets.
        super::native_forward_publication_tests::assert_one_independent_witness_settles_owned_word_disputes();
    }

    /// A provider segment that starts before the uncovered gap is not speech
    /// this gap owns. The stop path must leave that gap as one named refusal
    /// the conservation receipt counts, and must not commit the straddling text.
    #[test]
    fn straddling_gap_segment_stays_a_named_unrecovered_refusal() {
        let mut state = two_bursts("straddle-gap");
        let expected = coverage_speech_evidence(&state).ranges().to_vec();
        assert!(!expected.is_empty());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let offered = offer_stopping_windows(&mut state, &tx);
        assert!(!offered.is_empty());
        for request in &offered {
            let mut payload = gap_payload(&request.provider_request);
            let start = payload.segments[0].range.sample_start;
            payload.segments[0].range.sample_start = start.saturating_sub(500);
            payload.segments[0].text = "Straddle".into();
            payload.text = "Straddle".into();
            complete_planned_window(&mut state, &tx, request, Some(payload));
        }
        let receipt = reconcile_terminal_coverage(&mut state, &tx);
        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert!(
            !state
                .acoustic_ledger
                .lock()
                .unwrap()
                .rendered_text()
                .contains("Straddle"),
            "a segment that starts before the gap must not become committed text"
        );
        let _ = warning_codes(&mut rx);
        let conservation = state.session_conservation();
        assert_eq!(
            conservation
                .observations_refused_by_reason
                .get("unrecovered_speech")
                .copied(),
            Some(expected.len() as u64),
            "each material gap the stop path could not recover needs one named refusal: {conservation:?}"
        );
        assert_eq!(conservation.residue(), 0, "{conservation:?}");
    }

    /// Five committed debt occurrences, each wider than the Silero speech
    /// inside it, plus one speech burst that has no occurrence. The shape is
    /// take 9608b50e: the label is committed, the speech inside it is still
    /// debt, and a provider that answers with a segment wholly inside the
    /// requested range must recover the occurrence itself.
    fn commit_debt_occurrence(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        sample_start: u64,
        sample_end: u64,
        label: &str,
    ) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            sample_start,
            sample_end,
        );
        assert!(
            qualify_owned_occurrence(state, &occurrence),
            "the fixture occurrence must qualify before it can owe recovery"
        );
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            ledger.schedule_frontier(
                occurrence.clone(),
                [
                    LedgerObservationProducer::Apple,
                    LedgerObservationProducer::Lexicon,
                    LedgerObservationProducer::Whisper,
                ],
            );
            assert!(ledger.require_text_recovery(&occurrence));
        }
        let apple = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    sample_start,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(
            apple.is_some_and(|receipt| receipt.grants_mutation()),
            "Apple must commit the provisional label"
        );
        let lexicon = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    sample_start,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(lexicon.is_some(), "Lexicon closes the scheduled frontier");
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.text_recovery_pending(&occurrence));
        assert!(!ledger.is_sealed(&occurrence));
        occurrence
    }

    fn five_debt_occurrences(session: &str) -> (AppleSealState, Vec<OccurrenceIdentity>) {
        let stride = at(3.5);
        let gap_start = 5 * stride + at(0.5);
        let gap_end = gap_start + at(1.0);
        let capture_end = gap_end + at(0.5);
        let mut state = state_for(session, 0.0);
        state.audio.push(&vec![0.25f32; capture_end as usize]);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), state.capture_epoch);
        ingress.note_observed_pcm(capture_end, capture_end);
        let mut boundaries = Vec::new();
        for index in 0..5 {
            let speech_start = index * stride + at(0.6);
            let speech_end = index * stride + at(1.2);
            boundaries.push(crossing(VadBoundaryKind::SpeechStart, speech_start));
            boundaries.push(crossing(VadBoundaryKind::SpeechEnd, speech_end));
        }
        boundaries.push(crossing(VadBoundaryKind::SpeechStart, gap_start));
        boundaries.push(crossing(VadBoundaryKind::SpeechEnd, gap_end));
        ingress.observe_boundaries(&boundaries);
        state.fusion = Some(ingress);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut occurrences = Vec::new();
        for index in 0..5 {
            let start = index * stride;
            occurrences.push(commit_debt_occurrence(
                &mut state,
                &tx,
                start,
                start + at(2.0),
                // The ordinal identifies the fixture, not another spoken word.
                &format!("apple{index}"),
            ));
        }
        (state, occurrences)
    }

    fn event_trace(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> String {
        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                EngineEvent::LedgerMutation {
                    observation,
                    receipt,
                    label,
                } => lines.push(format!(
                    "mutation {} {}..{} {} {label}",
                    observation.producer.as_str(),
                    observation.occurrence.sample_start,
                    observation.occurrence.sample_end,
                    receipt.as_str()
                )),
                EngineEvent::Warning { code, message } => {
                    lines.push(format!("warn {code}: {message}"))
                }
                _ => {}
            }
        }
        lines.join("\n")
    }

    #[test]
    fn debt_stop_path_recovers_each_occurrence_span_not_its_speech_subrange() {
        let (mut state, occurrences, _pcm) = forensic_stop_recovery_capture("debt-span", true);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let unowned_gap = unowned_speech_gap(&state, &occurrences);
        let offered = settle_claimed_windows(
            &mut state,
            &tx,
            &occurrences,
            unowned_gap,
            |request, fresh| Some(iwo_words(request, fresh)),
        );
        let receipt = reconcile_terminal_coverage(&mut state, &tx);
        let trace = event_trace(&mut rx);
        assert!(
            offered.len() >= occurrences.len(),
            "stop path offered no window per occurrence\n{trace}"
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        let pending = ledger.pending_text_recoveries(&state.session_id, state.capture_epoch);
        assert!(
            pending.is_empty(),
            "debt stayed pending after wholly contained segments\npending={pending:?}\n{trace}"
        );
        assert_eq!(receipt.status, SealCoverageStatus::Complete, "{receipt:?}");
        assert!(receipt.covered_samples > 0, "{receipt:?}");
        for occurrence in &occurrences {
            assert_eq!(ledger.text_of(occurrence), Some("Iwo"));
            assert!(!ledger.text_recovery_pending(occurrence));
        }
        drop(ledger);
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .seal_terminal(&state.session_id, state.capture_epoch)
                    .is_ok(),
                "cleared debt and complete coverage must allow the terminal seal"
            );
        }
        assert_eq!(state.session_conservation().residue(), 0);
    }

    /// The live tail-patch drain and text recovery share one execution owner.
    /// An expired live deadline must not cancel the recovery phase.
    #[test]
    fn debt_recovery_runs_after_the_live_drain_deadline_expired() {
        let (mut state, occurrences, _pcm) =
            forensic_stop_recovery_capture("debt-after-drain", true);
        let deadline = LocalExecutionOwner::default().begin_drain(Duration::ZERO);
        assert!(deadline <= Instant::now());
        let (tx, _rx) = mpsc::unbounded_channel();
        let unowned_gap = unowned_speech_gap(&state, &occurrences);
        let _offered = settle_claimed_windows(
            &mut state,
            &tx,
            &occurrences,
            unowned_gap,
            |request, fresh| Some(iwo_words(request, fresh)),
        );
        let receipt = reconcile_terminal_coverage(&mut state, &tx);
        assert_eq!(receipt.status, SealCoverageStatus::Complete, "{receipt:?}");
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .pending_text_recoveries(&state.session_id, state.capture_epoch)
                .is_empty(),
            "an expired live drain cancelled recovery of {} occurrences",
            occurrences.len()
        );
    }

    #[test]
    fn recovery_segment_that_escapes_the_occurrence_stays_a_refusal() {
        let (mut state, occurrences) = five_debt_occurrences("debt-escape");
        let occurrence = occurrences[0].clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let offered = offer_stopping_windows(&mut state, &tx);
        assert!(!offered.is_empty());
        for request in &offered {
            let mut payload = gap_payload(&request.provider_request);
            let end = payload.segments[0].range.sample_end;
            payload.segments[0].range.sample_end = end.saturating_add(500);
            payload.segments[0].text = "Escaped".into();
            payload.text = "Escaped".into();
            complete_planned_window(&mut state, &tx, request, Some(payload));
        }
        let _receipt = reconcile_terminal_coverage(&mut state, &tx);
        let trace = event_trace(&mut rx);
        assert!(
            offered.iter().any(|request| {
                window_contains(request, occurrence.sample_start, occurrence.sample_end)
            }),
            "escape was not judged against a window that wholly contains the occurrence\n{trace}"
        );
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger.text_recovery_pending(&occurrence),
                "an escaping segment cleared debt\n{trace}"
            );
            assert_ne!(ledger.text_of(&occurrence), Some("Escaped"));
            assert!(
                !ledger.rendered_text().contains("Escaped"),
                "escaped text entered the document"
            );
            assert_eq!(
                ledger.seal_terminal(&state.session_id, state.capture_epoch),
                Err(SealRefusal::TextRecoveryPending)
            );
        }
        assert_eq!(
            state
                .session_conservation()
                .observations_refused_by_reason
                .get("unrecovered_speech")
                .copied(),
            Some(
                coverage_speech_evidence(&state)
                    .ranges()
                    .iter()
                    .filter(|gap| gap.sample_end.saturating_sub(gap.sample_start) > THRESHOLD)
                    .count() as u64
            ),
            "each material speech range is one named refusal\n{trace}"
        );
    }

    #[test]
    fn owned_terminal_repair_still_admits_both_exact_gaps() {
        let mut state = two_bursts("owned-repair");
        let ranges = coverage_speech_evidence(&state).ranges().to_vec();
        assert_eq!(ranges.len(), 2);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let offered = offer_stopping_windows(&mut state, &tx);
        let mut claimed = vec![false; ranges.len()];
        for request in &offered {
            let mut fresh = Vec::new();
            for (index, range) in ranges.iter().enumerate() {
                if !claimed[index] && window_contains(request, range.sample_start, range.sample_end)
                {
                    claimed[index] = true;
                    fresh.push(range.clone());
                }
            }
            if fresh.is_empty() {
                complete_planned_window(&mut state, &tx, request, None);
                continue;
            }
            let mut payload = gap_payload(&request.provider_request);
            payload.segments = fresh
                .into_iter()
                .map(|gap| TimedTailSegment {
                    confidence: None,
                    grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                    text: "Iwo".into(),
                    range: gap,
                })
                .collect();
            payload.text = "Iwo".into();
            complete_planned_window(&mut state, &tx, request, Some(payload));
        }
        let receipt = reconcile_terminal_coverage(&mut state, &tx);
        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        assert!(ranges.iter().all(|range| {
            offered
                .iter()
                .any(|request| window_contains(request, range.sample_start, range.sample_end))
        }));
        for range in &ranges {
            assert_eq!(
                state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .text_of(&OccurrenceIdentity::from(range)),
                Some("Iwo")
            );
        }
        assert!(
            !warning_codes(&mut rx)
                .iter()
                .any(|code| code == "seal_coverage_gap_inference_failed")
        );
    }

    #[test]
    fn multigap_expiry_uses_one_budget_and_cannot_publish_late_native_success() {
        let mut state = two_bursts("expired-repair");
        let expected_ranges = coverage_speech_evidence(&state).ranges().to_vec();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let offered = offer_stopping_windows(&mut state, &tx);
        assert!(!offered.is_empty());
        for request in &offered {
            complete_planned_window(&mut state, &tx, request, None);
        }
        let receipt = reconcile_terminal_coverage(&mut state, &tx);
        assert!(
            state.retained_word_decodes.is_empty(),
            "a late native witness is not retained"
        );
        assert_eq!(state.tail_patch_jobs_skipped, offered.len() as u64);
        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert_eq!(receipt.uncovered_speech_ranges, expected_ranges);
        assert_eq!(receipt.covered_samples, 0);
        assert_eq!(receipt.session_id, "expired-repair");
        assert_eq!(receipt.capture_epoch, 0);
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(
                event,
                EngineEvent::LedgerMutation { .. } | EngineEvent::LedgerSeal { .. }
            ));
        }
    }

    #[test]
    fn terminal_failure_and_foreign_identity_preserve_uncovered_pcm() {
        for foreign in [false, true] {
            let mut state = two_bursts("original-repair");
            let expected_ranges = coverage_speech_evidence(&state).ranges().to_vec();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let offered = offer_stopping_windows(&mut state, &tx);
            assert!(!offered.is_empty());
            for request in &offered {
                if foreign {
                    let mut payload = gap_payload(&request.provider_request);
                    payload.identity.range.session = "successor".into();
                    complete_planned_window(&mut state, &tx, request, Some(payload));
                } else {
                    complete_planned_window(&mut state, &tx, request, None);
                }
            }
            let receipt = reconcile_terminal_coverage(&mut state, &tx);
            assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
            assert_eq!(receipt.uncovered_speech_ranges, expected_ranges);
            assert_eq!(receipt.covered_samples, 0);
            if foreign {
                let codes = warning_codes(&mut rx);
                assert!(
                    codes
                        .iter()
                        .any(|code| code == "tail_patch_identity_mismatch"),
                    "foreign completion identity is refused: {codes:?}"
                );
            } else {
                assert_eq!(state.tail_patch_jobs_skipped, offered.len() as u64);
                while let Ok(event) = rx.try_recv() {
                    let granting = match &event {
                        EngineEvent::LedgerSeal { .. } => true,
                        EngineEvent::LedgerMutation { receipt, .. } => receipt.grants_mutation(),
                        _ => false,
                    };
                    assert!(!granting, "inference failure must not seal or grant");
                }
            }
        }
    }

    /// Commit the exact acoustic range as an occurrence, through the same
    /// admission path the terminal gap repair uses.
    fn commit_the_burst(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            at(1.0) - PAD,
            at(2.0) + PAD,
        );
        let receipt = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Whisper,
                    1,
                    0,
                    occurrence.clone(),
                ),
                label: "Iwo",
                energy: EnergyAdmission::QualifyFinalPassGap,
            },
        );
        assert!(
            receipt.is_some_and(|receipt| receipt.grants_mutation()),
            "the fixture must actually commit an occurrence, or coverage proves nothing"
        );
        occurrence
    }

    fn warning_codes(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<String> {
        let mut codes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::Warning { code, .. } = event {
                codes.push(code);
            }
        }
        codes
    }

    fn coverage_receipts(
        rx: &mut mpsc::UnboundedReceiver<EngineEvent>,
    ) -> Vec<SealCoverageReceipt> {
        let mut receipts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::SealCoverage { receipt, .. } = event {
                receipts.push(receipt);
            }
        }
        receipts
    }

    /// The actual repair entrypoint, on a take whose committed occurrence covers
    /// the acoustic speech exactly.
    ///
    /// Negative control, and the reason this test is not a tautology: the same
    /// ledger, asked about the padded ownership windows instead, answers
    /// `Incomplete` with a 127 000-sample hole. If `reconcile_terminal_coverage`
    /// ever drifts back to that set, `Complete` below stops holding.
    #[test]
    fn repair_measures_the_acoustic_set_and_leaves_a_covered_take_alone() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-repair-covered");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = warning_codes(&mut rx);

        // What the padded ownership set *would* answer if it were still allowed
        // to be the speech measurement. It is not — production reads
        // `coverage_speech_evidence` — so this is minted explicitly here as the
        // counterfactual the assertion below depends on.
        let padded_answer = {
            let ledger = state.acoustic_ledger.lock().unwrap();
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &AcousticSpeechEvidence::measured(
                    CaptureEvidenceIdentity::new(&state.session_id, state.capture_epoch),
                    crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER,
                    AcousticAvailability::Observed {
                        observed_samples: state.audio.session_sample_end(),
                    },
                    fusion_utterance_ranges(&state),
                ),
                THRESHOLD,
            )
        };
        assert_eq!(
            padded_answer.status,
            SealCoverageStatus::Incomplete,
            "the padded ownership set must disagree, or this test proves nothing"
        );
        assert_eq!(padded_answer.speech_samples, at(10.0));

        let receipt = reconcile_terminal_coverage(&mut state, &tx);

        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        assert!(receipt.uncovered_speech_ranges.is_empty());
        assert_eq!(
            receipt.speech_samples,
            at(1.0) + 2 * PAD,
            "coverage is measured against the crossings, not the ten-second window"
        );
        assert_eq!(receipt.incomplete_threshold_samples, THRESHOLD);
        assert!(
            warning_codes(&mut rx)
                .iter()
                .all(|code| !code.starts_with("seal_coverage_gap")),
            "a covered take must not send Whisper after gaps that were never speech"
        );
    }

    /// The actual publication entrypoint reports the acoustic set, records it on
    /// the ledger, and emits it once. The padded sum stays a diagnostic.
    #[test]
    fn publish_reports_the_acoustic_set_and_records_it_on_the_ledger() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-publish");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = coverage_receipts(&mut rx);

        let receipt = publish_terminal_coverage(&state, &tx);

        assert_eq!(receipt.speech_samples, at(1.0) + 2 * PAD);
        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        assert_eq!(
            sum_range_samples(&fusion_utterance_ranges(&state)),
            at(10.0),
            "ownership keeps its padded window; only the coverage question narrowed"
        );
        assert_eq!(
            coverage_receipts(&mut rx),
            vec![receipt.clone()],
            "one publication, carrying exactly the receipt that was returned"
        );
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .latest_seal_coverage()
                .cloned(),
            Some(receipt),
            "terminal admission reads this receipt; it must be the recorded one"
        );
    }

    /// Repair and publication are two different production functions. They must
    /// answer with the same speech set on the same state, or repair chases gaps
    /// the published receipt never claimed.
    ///
    /// This replaces the W1 test that called one helper twice: both named
    /// entrypoints are invoked here, and each one's own receipt is compared.
    #[test]
    fn repair_and_publish_answer_with_one_speech_set() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-one-set");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = coverage_receipts(&mut rx);

        let published_before = publish_terminal_coverage(&state, &tx);
        let repaired = reconcile_terminal_coverage(&mut state, &tx);
        let published_after = publish_terminal_coverage(&state, &tx);

        for other in [&repaired, &published_after] {
            assert_eq!(other.speech_samples, published_before.speech_samples);
            assert_eq!(other.covered_samples, published_before.covered_samples);
            assert_eq!(
                other.incomplete_threshold_samples,
                published_before.incomplete_threshold_samples
            );
            assert_eq!(other.status, published_before.status);
            assert_eq!(
                other.uncovered_speech_ranges,
                published_before.uncovered_speech_ranges
            );
        }
    }

    /// Uncovered speech whose PCM cannot be resolved is reported, not decoded
    /// and not quietly dropped. No witness is manufactured for audio the process
    /// no longer holds.
    #[test]
    fn repair_reports_unresolvable_gap_pcm_instead_of_inventing_a_witness() {
        // Capture energy measured three seconds of speech; the live buffer holds
        // none of it, which is exactly the retention-loss case.
        let mut state = state_for("rc-w2-unresolvable", 0.0);
        assert!(state.fusion.is_none());
        let mut accumulator = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        accumulator.push_samples(&vec![0.25f32; at(3.0) as usize]);
        let (tx, _rx) = mpsc::unbounded_channel();

        let receipt = reconcile_terminal_coverage(&mut state, &tx);

        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert_eq!(receipt.speech_samples, at(3.0));
        assert_eq!(receipt.covered_samples, 0);
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .conservation()
                .refusals_by_reason
                .get("unrecovered_speech")
                .copied(),
            Some(1),
            "speech the buffer does not hold is unrecovered_speech"
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger.rendered_text().is_empty(),
            "no text may enter the ledger from a range whose PCM was never read"
        );
        assert_eq!(ledger.qualified_occurrences().count(), 0);
    }

    /// The capture energy fallback measures the hops the capture path actually
    /// recorded — not a flag, and not the whole take.
    #[test]
    fn capture_energy_fallback_measures_recorded_hops() {
        let state = state_for("rc-w2-energy", 2.0);
        assert!(state.fusion.is_none());
        let mut accumulator = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        accumulator.push_samples(&vec![0.25f32; at(1.0) as usize]);
        accumulator.push_samples(&vec![0.0f32; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);

        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: at(2.0)
            }
        );
        let speech = evidence.ranges();
        assert_eq!(speech.len(), 1, "the silent second is not speech");
        assert_eq!(speech[0].sample_start, 0);
        assert_eq!(speech[0].sample_end, at(1.0));
        assert_eq!(speech[0].session, "rc-w2-energy");
        assert_eq!(speech[0].capture_epoch, state.capture_epoch);
    }

    /// The gap this cut closes. Captured zeros and no samples at all both
    /// produce an empty speech set, and they must no longer produce the same
    /// finality: the first is a measurement whose answer is silence, the second
    /// is no measurement.
    #[test]
    fn measured_silence_and_absent_evidence_reach_different_finality() {
        let silent_state = state_for("rc-w2-silent", 1.0);
        let mut accumulator = CaptureLevelAccumulator::bound_to(&silent_state.capture_energy);
        accumulator.push_samples(&vec![0.0f32; at(1.0) as usize]);
        let silent_evidence = coverage_speech_evidence(&silent_state);

        let absent_state = state_for("rc-w2-absent", 1.0);
        let absent_evidence = coverage_speech_evidence(&absent_state);

        assert!(
            silent_evidence.ranges().is_empty(),
            "silence is never speech"
        );
        assert!(
            absent_evidence.ranges().is_empty(),
            "absence is never speech"
        );
        assert_eq!(silent_evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(absent_evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            silent_evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: at(1.0)
            }
        );
        assert_eq!(
            absent_evidence.availability(),
            AcousticAvailability::NotObserved
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let silent = publish_terminal_coverage(&silent_state, &tx);
        let absent = publish_terminal_coverage(&absent_state, &tx);
        assert_eq!(silent.speech_samples, 0);
        assert_eq!(absent.speech_samples, 0);
        assert_eq!(
            silent.status,
            SealCoverageStatus::Complete,
            "a ladder that ran and heard nothing has covered everything there was"
        );
        assert_eq!(
            absent.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
            "a ladder that never ran cannot certify the take"
        );
        assert_eq!(silent.coverage_ratio(), Some(1.0));
        assert_eq!(
            absent.coverage_ratio(),
            None,
            "absence of measurement has no ratio"
        );
        assert_eq!(silent.observed_samples, Some(at(1.0)));
        assert_eq!(absent.observed_samples, None);

        // And the finality consumers agree: one may seal, the other may not.
        assert!(
            silent_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w2-silent", 0)
                .is_err(),
            "an empty ledger still has no occurrence to seal"
        );
        assert_eq!(
            absent_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w2-absent", 0),
            Err(SealRefusal::CoverageIncomplete),
            "unavailable measurement refuses before the ledger even looks for occurrences"
        );
    }

    /// rc-w3-acoustic-validity: three capture qualities, three honest outcomes,
    /// all through `publish_terminal_coverage` and the ledger's own finality.
    ///
    /// Valid silence still succeeds — that is the outcome the validity repair
    /// must not cost. An all-invalid capture and a mixed valid/invalid capture
    /// both refuse, and both name invalid measurement rather than a gap or an
    /// absent observer.
    #[test]
    fn valid_silence_all_invalid_and_mixed_capture_reach_distinct_outcomes() {
        let silent_state = state_for("rc-w3-quality-silent", 1.0);
        let mut silent_writer = CaptureLevelAccumulator::bound_to(&silent_state.capture_energy);
        silent_writer.push_samples(&vec![0.0f32; at(1.0) as usize]);

        let invalid_state = state_for("rc-w3-quality-invalid", 1.0);
        let mut invalid_writer = CaptureLevelAccumulator::bound_to(&invalid_state.capture_energy);
        invalid_writer.push_samples(&vec![f32::NAN; at(1.0) as usize]);

        let mixed_state = state_for("rc-w3-quality-mixed", 2.0);
        let mut mixed_writer = CaptureLevelAccumulator::bound_to(&mixed_state.capture_energy);
        mixed_writer.push_samples(&vec![0.0f32; at(1.0) as usize]);
        mixed_writer.push_samples(&vec![f32::INFINITY; at(1.0) as usize]);

        let (tx, _rx) = mpsc::unbounded_channel();
        let silent = publish_terminal_coverage(&silent_state, &tx);
        let invalid = publish_terminal_coverage(&invalid_state, &tx);
        let mixed = publish_terminal_coverage(&mixed_state, &tx);

        assert_eq!(
            silent.status,
            SealCoverageStatus::Complete,
            "a ladder that ran over finite silence covered everything there was"
        );
        assert_eq!(silent.coverage_ratio(), Some(1.0));
        assert_eq!(silent.observed_samples, Some(at(1.0)));

        for (label, receipt) in [("all invalid", &invalid), ("mixed", &mixed)] {
            assert_eq!(
                receipt.status,
                SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
                "{label} capture measured nothing it can stand behind"
            );
            assert_eq!(receipt.coverage_ratio(), None, "{label} has no ratio");
            assert_eq!(receipt.observed_samples, None, "{label} has no extent");
            assert_eq!(receipt.availability, "invalid_measurement");
            assert_eq!(receipt.speech_producer, CAPTURE_ENERGY_PRODUCER);
        }

        // The three receipts are genuinely different documents, not one status
        // rendered three ways.
        assert_ne!(silent.status, invalid.status);
        assert_eq!(
            invalid.status, mixed.status,
            "both invalid captures name the same reason; only the diagnostic \
             prefix differs, and that never reaches the receipt"
        );

        // Finality agrees with the receipts.
        assert_eq!(
            invalid_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w3-quality-invalid", 0),
            Err(SealRefusal::CoverageIncomplete),
            "invalid measurement refuses the terminal seal"
        );
        assert_eq!(
            mixed_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w3-quality-mixed", 0),
            Err(SealRefusal::CoverageIncomplete),
            "a valid second in front of an invalid one does not rescue the seal"
        );
    }

    /// rc-w3-acoustic-validity: the extent guard is not Silero-specific.
    ///
    /// The capture energy ladder sits upstream of the retained buffer, so in
    /// production it cannot fall behind it. This pins the guard's second arm
    /// anyway: whichever observer answers, an extent shorter than the capture
    /// is reported as an unobserved remainder rather than covered silence.
    #[test]
    fn a_short_capture_energy_extent_cannot_certify_the_unheard_remainder() {
        let state = state_for("rc-w3-validity-short-ladder", 10.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.0f32; at(4.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Discontinuous {
                observed_samples: at(4.0)
            },
            "four measured seconds of a ten-second capture leave six unheard"
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let receipt = publish_terminal_coverage(&state, &tx);
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );
        assert_eq!(receipt.coverage_ratio(), None);
    }

    /// The terminal coverage threshold is 250 ms of the capture clock, and the
    /// published receipt carries the exact figure it was judged against.
    #[test]
    fn terminal_coverage_threshold_stays_at_two_hundred_fifty_milliseconds() {
        assert_eq!(SEAL_COVERAGE_INCOMPLETE_MS, 250);
        assert_eq!(
            u64::from(48_000_u32) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
            12_000,
            "the immutable ledger threshold at 48 kHz"
        );
        assert_eq!(
            u64::from(RATE) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
            THRESHOLD
        );

        let mut state = one_burst_in_a_ten_second_take("rc-w2-threshold");
        let (tx, _rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        assert_eq!(
            publish_terminal_coverage(&state, &tx).incomplete_threshold_samples,
            THRESHOLD
        );
        assert_eq!(
            reconcile_terminal_coverage(&mut state, &tx).incomplete_threshold_samples,
            THRESHOLD
        );
    }

    #[test]
    fn long_quiet_take_does_not_reoffer_settled_prefix_at_stop() {
        use crate::stt::tail_provider::TailSegmentGrain;

        let mut state = one_burst_in_a_ten_second_take("settled-prefix-stop");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let owner = commit_the_burst(&mut state, &tx);
        let seal = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_of(&owner)
            .expect("the utterance seals while recording, before the quiet tail")
            .clone();
        assert!(!state.capture_stopping);

        // A retained ASR frame includes a word hallucinated over measured
        // quiet. Stop must not re-offer this historical frame once its real
        // speech is settled, even as a refused mutation/diagnostic proposal.
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: 42,
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start: 0,
                    sample_end: at(9.0),
                },
            },
            sample_rate: RATE,
            language: Some("pl".into()),
        };
        let mut payload = gap_payload(&request);
        payload.evidence.segment_grain = TailSegmentGrain::Word;
        payload.text = "Dziękuję.".into();
        payload.segments = vec![TimedTailSegment {
            confidence: None,
            grain: TailSegmentGrain::Word,
            text: "Dziękuję.".into(),
            range: TailSampleRange {
                sample_start: at(4.0),
                sample_end: at(4.5),
                ..request.identity.range.clone()
            },
        }];
        payload.validate().unwrap();
        state.retain_word_decode(&payload);
        assert_eq!(state.retained_word_decodes.len(), 1);

        let quiet = vec![0.0; at(60.0) as usize];
        for _ in 0..30 {
            state.audio.push(&quiet);
        }
        let captured = state.audio.session_sample_end();
        state
            .fusion
            .as_mut()
            .unwrap()
            .note_observed_pcm(captured - at(10.0), captured);
        while rx.try_recv().is_ok() {}
        let (requests, mut requested) = mpsc::channel(4);
        state.tail_patch = Some(requests);
        state.capture_stopping = true;
        let admitted = state.windows_admitted;

        let coverage = reconcile_terminal_coverage(&mut state, &tx);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert_eq!(state.windows_admitted, admitted);
        assert!(
            requested.try_recv().is_err(),
            "no new model request for the prefix"
        );
        assert!(
            !std::iter::from_fn(|| rx.try_recv().ok())
                .any(|event| matches!(event, EngineEvent::LedgerMutation { .. })),
            "Stop must not re-offer historical settled/quiet words, even to refuse them"
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&owner), Some("Iwo"));
        assert_eq!(ledger.seal_of(&owner), Some(&seal));
        assert_eq!(ledger.conservation().residue(), 0);
    }

    /// Fix d0670e5ac; kills M3b-07 (settled frames re-enter Stop admission).
    /// Literal settled speech and a stale quiet-tail decode survive two Stops
    /// without another mutation, duplicated document text or a changed seal.
    #[test]
    fn m3b_repeated_stop_preserves_literal_settled_transcript_without_reoffers() {
        use crate::stt::tail_provider::TailSegmentGrain;

        let mut state = one_burst_in_a_ten_second_take("m3b-settled-stop");
        let (events, mut receiver) = mpsc::unbounded_channel();
        let owner = commit_the_burst(&mut state, &events);
        let seal = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_of(&owner)
            .unwrap()
            .clone();
        assert_eq!(state.acoustic_ledger.lock().unwrap().rendered_text(), "Iwo");

        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: 700,
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start: 0,
                    sample_end: 144_000,
                },
            },
            sample_rate: 16_000,
            language: Some("pl".into()),
        };
        let mut stale = gap_payload(&request);
        stale.text = "Jutro.".into();
        stale.evidence.segment_grain = TailSegmentGrain::Word;
        stale.segments = vec![TimedTailSegment {
            text: "Jutro.".into(),
            confidence: None,
            grain: TailSegmentGrain::Word,
            range: TailSampleRange {
                sample_start: 64_000,
                sample_end: 72_000,
                ..request.identity.range.clone()
            },
        }];
        stale.validate().unwrap();
        state.retain_word_decode(&stale);
        assert_eq!(state.retained_word_decodes.len(), 1);
        while receiver.try_recv().is_ok() {}
        state.capture_stopping = true;

        for _ in 0..2 {
            let coverage = reconcile_terminal_coverage(&mut state, &events);
            assert_eq!(coverage.status, SealCoverageStatus::Complete);
            let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
            assert!(
                !emitted
                    .iter()
                    .any(|event| matches!(event, EngineEvent::LedgerMutation { .. })),
                "Stop re-offered a settled historical frame, even if the ledger refused it"
            );
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.rendered_text(), "Iwo");
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            assert_eq!(ledger.seal_of(&owner), Some(&seal));
            assert_eq!(ledger.conservation().residue(), 0);
        }
    }

    #[test]
    fn terminal_retained_recovery_still_accounts_for_unowned_speech() {
        use crate::stt::tail_provider::TailSegmentGrain;

        for speech_end in [2.0, 1.0625] {
            let mut state = state_for(&format!("unsettled-retained-{speech_end}"), 10.0);
            CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE)
                .push_samples(&vec![0.25; at(10.0) as usize]);
            let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
            ingress.note_observed_pcm(at(10.0), at(10.0));
            ingress.observe_boundaries(&[
                crossing(VadBoundaryKind::SpeechStart, at(1.0)),
                crossing(VadBoundaryKind::SpeechEnd, at(speech_end)),
            ]);
            ingress.observe(Some((0, at(10.0))), true, at(10.0));
            state.fusion = Some(ingress);
            let (tx, _rx) = mpsc::unbounded_channel();
            let range = coverage_speech_evidence(&state).ranges()[0].clone();
            if speech_end < 2.0 {
                assert!(range.sample_end - range.sample_start < THRESHOLD);
            }
            let request = TailProviderRequest {
                identity: TailRequestIdentity {
                    request_id: 43,
                    range: TailSampleRange {
                        session: state.session_id.clone(),
                        capture_epoch: state.capture_epoch,
                        sample_start: 0,
                        sample_end: at(9.0),
                    },
                },
                sample_rate: RATE,
                language: Some("pl".into()),
            };
            let mut payload = gap_payload(&request);
            payload.evidence.segment_grain = TailSegmentGrain::Word;
            payload.segments[0].grain = TailSegmentGrain::Word;
            payload.segments[0].range = range.clone();
            payload.validate().unwrap();
            state.retain_word_decode(&payload);
            state.capture_stopping = true;

            let coverage = reconcile_terminal_coverage(&mut state, &tx);
            assert_eq!(coverage.status, SealCoverageStatus::Complete);
            assert_eq!(
                state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .text_of(&OccurrenceIdentity::from(&range)),
                Some("Iwo"),
                "settled-history filtering must retain real speech, including sub-threshold words"
            );
        }
    }

    /// A sealed occurrence is finished. A later machine observation of the same
    /// PCM is refused as a replay, and the committed label does not move.
    #[test]
    fn a_sealed_occurrence_refuses_a_later_machine_observation() {
        // Three seconds, so the committed burst's padded range is inside the
        // retained PCM the qualification step has to read.
        let mut state = state_for("rc-w2-post-seal", 3.0);
        let (tx, _rx) = mpsc::unbounded_channel();
        let occurrence = commit_the_burst(&mut state, &tx);

        assert!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .is_some(),
            "the fixture must reach a seal before the replay means anything"
        );

        let replay = admit_ledger_label(
            &mut state,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    2,
                    0,
                    occurrence.clone(),
                ),
                label: "Iwo poprawione",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );

        assert!(matches!(
            replay,
            Some(MutationReceipt::Refuse {
                reason: RefuseReason::SealedReplay,
                ..
            })
        ));
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&occurrence)
                .map(str::to_owned),
            Some("Iwo".to_string()),
            "a refused replay may not rewrite sealed text"
        );
    }

    /// The retired char-diff verdict is diagnostics. A `skipped` job still
    /// forwards its payload, and that payload still reaches occurrence
    /// admission — proven end to end, through the two production functions that
    /// carry it, not through the receipt struct alone.
    #[test]
    fn a_legacy_skip_verdict_still_reaches_ledger_admission() {
        let mut state = measured_state_for("rc-w2-legacy-skip", 2.0);
        let (tx, _rx) = mpsc::unbounded_channel();
        let occurrence =
            OccurrenceIdentity::new(state.session_id.clone(), state.capture_epoch, 0, at(1.0));
        let range = TailSampleRange {
            session: state.session_id.clone(),
            capture_epoch: state.capture_epoch,
            sample_start: 0,
            sample_end: at(1.0),
        };

        // Qualify the occurrence and open exactly the Whisper slot the job is
        // about, without pre-admitting any label.
        {
            let calibration = state.energy_calibration.clone().unwrap();
            let window = state.window_by_samples(0, at(1.0)).unwrap();
            let energy_integral = window
                .samples
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: 1_000.0,
                            energy_integral,
                            mean_rms_dbfs: -12.0,
                            peak_dbfs: -12.0,
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(at(1.0)),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration,
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(occurrence.clone(), vec![LedgerObservationProducer::Whisper]);
        }
        state.pending_events.insert(
            9,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: "Iwo".into(),
                layer1_baseline: "Iwo".into(),
                start_ts: 0.0,
                end_ts: 1.0,
                segments: Vec::new(),
            },
        );
        let (tail_tx, mut tail_rx) = mpsc::channel(1);
        state.tail_patch = Some(tail_tx);
        state.capture_stopping = true;
        state.pump_capture_windows(&tx);
        let request = tail_rx.try_recv().expect("planned stopping window");
        assert!(
            request
                .member_occurrences
                .iter()
                .any(|(_, owner)| owner == &occurrence),
            "the stopping window names the pending occurrence"
        );
        assert!(
            tail_rx.try_recv().is_err(),
            "a 2s stop emits one partial window"
        );
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        let identity = request.provider_request.identity.clone();
        assert_eq!(state.refinement_submitted.len(), 1);
        assert_eq!(state.tail_patch_awaiting_completion(), 1);

        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "odzysk".into(),
            segments: vec![TimedTailSegment {
                confidence: None,
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                text: "odzysk".into(),
                range: range.clone(),
            }],
            avg_logprob: None,
            compression_ratio: None,
            provider_id: crate::stt::tail_provider::TailProviderId::Fake,
            elapsed_ms: 0,
            evidence: crate::stt::tail_provider::TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: crate::stt::tail_provider::TailEvidenceSource::Whisper,
                revision: Some("rc-w2".into()),
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: crate::stt::tail_provider::TailTimingQuality::ExactSampleRange,
                avg_logprob: None,
                vad_selected_samples: None,
            },
        };

        let mut lane =
            AppleTailPatchLane::new(RATE, None, crate::stt::tail_provider::TailProviderId::Fake);
        let completion = lane.finish_for_worker(
            Some(TailPatchInFlight {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: identity,
                admit_sample_start: request.admit_sample_start,
                admit_sample_end: request.admit_sample_end,
                member_occurrences: request.member_occurrences.clone(),
            }),
            Ok(TailPatchJobResult {
                utterance_id: request.utterance_id,
                outcome: TailPatchOutcome::skipped(
                    SkipReasonCode::ChangeRatio,
                    "ratio 1.31 exceeds max 0.50",
                ),
                payload,
            }),
        );

        assert!(
            completion.payload.is_some(),
            "a skipped verdict must not discard the provider payload"
        );

        state.complete_whisper_window(&tx, completion, 1.0);

        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&occurrence)
                .map(str::to_owned),
            Some("odzysk".to_string()),
            "the payload a legacy skip 'rejected' is the one the ledger admitted"
        );
        assert_eq!(state.tail_patch_jobs_applied, 1);
        assert_eq!(state.tail_patch_jobs_skipped, 0);
    }

    /// Ported from the parked `mod tests`
    /// (`apple_segments_map_to_captured_pcm_samples_at_ingestion`): Apple
    /// segment seconds land on the session PCM clock at ingestion, and a
    /// segment reaching past captured audio is clamped rather than inventing
    /// samples. The current owner is `apple_segments_on_pcm_clock`.
    #[test]
    fn apple_segments_land_on_the_captured_pcm_clock() {
        let state = state_for("rc-w2-segments", 2.0);
        let mapped = apple_segments_on_pcm_clock(
            &state,
            &[
                TranscriptSegment {
                    confidence: None,
                    text: "Iwo".into(),
                    start_ts: 0.25,
                    end_ts: 0.75,
                },
                TranscriptSegment {
                    confidence: None,
                    text: "poza".into(),
                    start_ts: 1.5,
                    end_ts: 9.0,
                },
            ],
        );
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].range.sample_start, at(0.25));
        assert_eq!(mapped[0].range.sample_end, at(0.75));
        assert_eq!(mapped[0].range.session, "rc-w2-segments");
        assert_eq!(mapped[1].range.sample_start, at(1.5));
        assert_eq!(
            mapped[1].range.sample_end,
            at(2.0),
            "a segment cannot claim audio the session never captured"
        );
    }

    // ══════════════════════════════════════════════════════════════════
    // Ported from the parked `mod tests`: engine lifecycle over the single
    // Silero. `EpochGate`, `EpochDecision`, `EPOCH_PREROLL_SECS` and every
    // method used below are live production symbols with a live consumer
    // (`apple_stream_worker`); only the parked module around them was dead, so
    // these contracts move here unchanged rather than being retired.
    // ══════════════════════════════════════════════════════════════════

    /// Amplitude stand-in for the session Silero's `speech_live` bit, so the
    /// epoch state machine can be driven on synthetic PCM without loading the
    /// VAD model (a unit test must not depend on the model being present).
    fn amplitude_edge(samples: &[f32], threshold: f32) -> bool {
        samples.iter().any(|s| s.abs() >= threshold)
    }

    /// 200 Hz tone at `amplitude` — the "speech" side of the fixture.
    fn tone(secs: f32, amplitude: f32) -> Vec<f32> {
        let total = (secs * RATE as f32) as usize;
        (0..total)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                amplitude * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
            })
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * RATE as f32) as usize]
    }

    /// Drive the gate the way the worker does — chunk by chunk — keeping each
    /// decision next to the session cursor it was taken at. The last chunk of a
    /// block is short, so cursors are carried, never reconstructed from indices.
    fn drive(
        gate: &mut EpochGate,
        audio: &[f32],
        samples_seen: &mut u64,
    ) -> Vec<(u64, EpochDecision)> {
        let mut out = Vec::new();
        for chunk in audio.chunks(1024) {
            *samples_seen += chunk.len() as u64;
            out.push((
                *samples_seen,
                gate.feed_pcm(chunk, *samples_seen, amplitude_edge(chunk, 0.1)),
            ));
        }
        out
    }

    /// Speech opens an epoch, silence past the product threshold closes it, and
    /// the next speech edge wakes a new one whose base carries the pre-roll —
    /// without reaching back into the epoch that already closed.
    #[test]
    fn epoch_gate_sleeps_after_threshold_silence_and_wakes_with_preroll() {
        let mut gate = EpochGate::armed(RATE, 5.0);
        let mut seen = 0u64;

        let speech = drive(&mut gate, &tone(2.0, 0.5), &mut seen);
        assert!(
            matches!(
                speech.first(),
                Some((_, EpochDecision::Wake { preroll_from: 0 }))
            ),
            "first speech chunk must open epoch 0 (nothing was retained before it), got {:?}",
            speech.first()
        );
        assert!(
            speech[1..]
                .iter()
                .all(|(_, d)| *d == EpochDecision::Forward),
            "speech after the wake must forward, got {:?}",
            &speech[1..]
        );

        let quiet = drive(&mut gate, &silence(6.0), &mut seen);
        let sleep_at = quiet
            .iter()
            .position(|(_, d)| matches!(d, EpochDecision::Sleep { .. }))
            .expect("6 s of silence at a 5 s threshold must close the epoch");
        let (sleep_cursor, EpochDecision::Sleep { silence_secs }) = quiet[sleep_at] else {
            unreachable!("position() matched Sleep");
        };
        assert!(
            (5.0..5.2).contains(&silence_secs),
            "the epoch must close within one chunk of the 5 s threshold, closed at {silence_secs}s"
        );
        assert!(
            quiet[sleep_at + 1..]
                .iter()
                .all(|(_, d)| *d == EpochDecision::Idle),
            "after sleeping the engine rests until the next speech edge, got {:?}",
            &quiet[sleep_at + 1..]
        );

        let resume_cursor = seen;
        let woke = drive(&mut gate, &tone(1.0, 0.5), &mut seen);
        let (_, EpochDecision::Wake { preroll_from }) = woke[0] else {
            panic!("speech after rest must wake a new epoch, got {:?}", woke[0]);
        };
        let preroll = (EPOCH_PREROLL_SECS * RATE as f32) as u64;
        assert_eq!(
            preroll_from,
            resume_cursor.saturating_sub(preroll),
            "the new epoch is based one pre-roll ahead of the chunk that woke it"
        );
        assert!(
            preroll_from >= sleep_cursor,
            "pre-roll must not re-feed audio the closed epoch already carried \
             ({preroll_from} < {sleep_cursor})"
        );
    }

    /// `utterance_silence_sec: None` is the legacy contract: one stream for the
    /// whole take, no epoch decisions at all.
    #[test]
    fn epoch_gate_disarmed_never_sleeps_or_wakes() {
        let mut gate = EpochGate::disarmed();
        assert!(!gate.is_armed());
        let mut seen = 0u64;
        let mut decisions = drive(&mut gate, &tone(1.0, 0.5), &mut seen);
        decisions.extend(drive(&mut gate, &silence(30.0), &mut seen));
        decisions.extend(drive(&mut gate, &tone(1.0, 0.5), &mut seen));
        assert!(
            decisions.iter().all(|(_, d)| *d == EpochDecision::Forward),
            "disarmed gate must forward every chunk, got {:?}",
            decisions
                .iter()
                .filter(|(_, d)| *d != EpochDecision::Forward)
                .collect::<Vec<_>>()
        );
    }

    /// No Silero means no edges, and an armed gate with no edge source would
    /// rest forever on a stream that never opened. The lifecycle fails open.
    #[test]
    fn epoch_gate_without_speech_edges_falls_back_to_one_stream() {
        let mut gate = EpochGate::for_session(RATE, Some(5.0), false);
        assert!(
            !gate.is_armed(),
            "an armed gate with no edge source would sleep the engine forever"
        );
        assert_eq!(
            gate.feed_pcm(&[0.0; 1_024], 1_024, false),
            EpochDecision::Forward,
            "Silero absence must preserve continuous Apple PCM flow"
        );
        assert!(EpochGate::for_session(RATE, Some(5.0), true).is_armed());
        assert!(
            !EpochGate::for_session(RATE, None, true).is_armed(),
            "no hands-free silence setting is still the legacy single stream"
        );
    }

    /// Ported from the parked `mod tests`
    /// (`epoch_shift_lifts_segment_times_onto_the_session_pcm_clock`): bridge
    /// time restarts at zero for every epoch, so events leaving a non-zero
    /// epoch must be lifted onto the session PCM clock before any seal maps
    /// them to samples. Only the retired `AppleSealState::new` constructor kept
    /// this test parked; the contract and both owners are live.
    #[test]
    fn epoch_shift_lifts_segment_times_onto_the_session_pcm_clock() {
        let state = state_for("rc-w2-epoch-shift", 110.0);

        let shifted = shift_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Iwo".into(),
                segments: vec![TranscriptSegment {
                    confidence: None,
                    text: "Iwo".into(),
                    start_ts: 0.5,
                    end_ts: 2.0,
                }],
            }],
            100.0,
        );
        let LiveStreamEvent::PhraseFinal { segments, .. } = &shifted[0] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[0]
            );
        };
        // Both sums are exact in binary32, so this is equality, not tolerance.
        assert_eq!(segments[0].start_ts, 100.5);
        assert_eq!(segments[0].end_ts, 102.0);

        let on_pcm = apple_segments_on_pcm_clock(&state, segments);
        assert_eq!(on_pcm[0].range.sample_start, at(100.5));
        assert_eq!(on_pcm[0].range.sample_end, at(102.0));
    }

    /// Ported from the parked `mod tests`
    /// (`epoch_shift_at_base_zero_is_identity`): the first epoch is based at 0,
    /// so the shim is the identity there. This is what keeps a single-epoch
    /// take bit-identical to the legacy one-stream lane.
    #[test]
    fn epoch_shift_at_base_zero_is_identity() {
        let shifted = shift_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "Iwo".into(),
                    segments: vec![TranscriptSegment {
                        confidence: None,
                        text: "Iwo".into(),
                        start_ts: 0.5,
                        end_ts: 2.0,
                    }],
                },
                LiveStreamEvent::Summary {
                    text: "Iwo wraca".into(),
                    segments: vec![TranscriptSegment {
                        confidence: None,
                        text: "Iwo wraca".into(),
                        start_ts: 0.5,
                        end_ts: 4.0,
                    }],
                    ok: true,
                    error: None,
                },
            ],
            0.0,
        );
        let LiveStreamEvent::Partial { segments, .. } = &shifted[0] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[0]
            );
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 2.0));
        let LiveStreamEvent::Summary { segments, .. } = &shifted[1] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[1]
            );
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 4.0));
    }

    // Timed phrase observation and physical retention contracts.

    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            confidence: None,
            text: text.into(),
            start_ts,
            end_ts,
        }
    }

    #[test]
    fn progressive_valleys_keep_five_iwo_occurrences_before_native_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(RATE, "progressive-five-iwo".into(), 1);
        let (requests, _requests_rx) = mpsc::channel(CONSULTATION_QUEUE_CAP);
        let (_returns_tx, returns) = std_mpsc::channel();
        let mut consultation = LiveConsultationCapture {
            queue: ConsultationInputQueue::new(state.session_id.clone(), state.capture_epoch)
                .unwrap(),
            requests,
            returns,
            last_assessed: None,
            assessment_pending: false,
            answers_pending: 0,
            speech_open: false,
            refused: false,
        };
        state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, at(0.3)));
        state.fusion = Some(SileroIngress::new(
            RATE,
            &state.session_id,
            state.capture_epoch,
        ));
        state.fusion_seal_armed = true;
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        let mut cursor = 0u64;
        for repetition in 0..5 {
            let start = cursor as f32 / RATE as f32;
            for frame in 0..31 {
                let speech = frame < 20;
                let pcm = vec![if speech { 0.25 } else { 0.0 }; 512];
                writer.push_samples(&pcm);
                state.audio.push(&pcm);
                cursor += 512;
                let fusion = state.fusion.as_mut().unwrap();
                fusion.push_scripted_speech_prob_for_test(if speech { 0.9 } else { 0.01 });
                let ingest = fusion.ingest(&pcm, &pcm, cursor);
                consultation.observe(&ingest, cursor).unwrap();
                if frame == 19 {
                    emit_stream_events(
                        vec![LiveStreamEvent::Partial {
                            text: "Iwo".into(),
                            segments: vec![segment("Iwo", start + 0.1, start + 0.64)],
                        }],
                        &tx,
                        &mut state,
                        cursor as f32 / RATE as f32,
                    );
                }
                publish_apple_phrase_at_valley(
                    &mut state,
                    &tx,
                    cursor as f32 / RATE as f32,
                    Some(&mut consultation),
                );
            }
            assert_eq!(
                state
                    .fusion
                    .as_ref()
                    .unwrap()
                    .ledger()
                    .utterances()
                    .iter()
                    .filter(|utterance| utterance.closed)
                    .count(),
                repetition + 1
            );
            assert!(
                state.open_partial.is_empty(),
                "short measured valley publishes the phrase"
            );
            assert!(
                !consultation.speech_open,
                "Max receives the same physical close"
            );
            assert_eq!(consultation.queue.pending_groups(), 1);
        }
        let ledger = state.acoustic_ledger.lock().unwrap();
        let words = ledger
            .occurrences()
            .filter_map(|owner| ledger.slots_of(owner))
            .flatten()
            .filter(|word| word.text == "Iwo")
            .count();
        assert_eq!(words, 5);
        drop(ledger);
        let mut finals = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::UtteranceFinal { text, .. } = event {
                finals.push(text);
            }
        }
        assert_eq!(
            finals
                .iter()
                .flat_map(|text| text.split_whitespace())
                .filter(|word| *word == "Iwo")
                .count(),
            5
        );
    }

    #[test]
    fn partial_restart_needs_measured_valley_not_changed_words_or_timestamps() {
        let mut state = state_for("partial-valley", 5.0);
        state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, at(0.3)));
        state.open_partial = "Sam poukładać".into();
        state.open_partial_segments = vec![segment("Sam poukładać", 0.2, 1.0)];
        let next = [segment("Sam nie mamy", 2.0, 2.5)];
        assert!(!apple_partial_restart_has_valley(&state, &next));
        let mut fusion = SileroIngress::new(RATE, &state.session_id, state.capture_epoch);
        fusion.note_observed_pcm(at(5.0), at(5.0));
        fusion.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: at(0.1),
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: at(1.0),
                speech_probability: 0.01,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: at(2.0),
                speech_probability: 0.9,
            },
        ]);
        state.fusion = Some(fusion);
        assert!(apple_partial_restart_has_valley(&state, &next));
        assert!(!apple_partial_restart_has_valley(
            &state,
            &[segment("poprawka", 0.8, 1.2)]
        ));
        assert!(!apple_partial_restart_has_valley(
            &state,
            &[segment("bez pomiaru", 0.0, 0.0)]
        ));
        state
            .fusion
            .as_mut()
            .unwrap()
            .note_observed_pcm(100, at(6.0));
        assert!(
            !apple_partial_restart_has_valley(&state, &next),
            "a capture hole is not silence"
        );
    }

    #[test]
    fn partial_revision_live_same_onset_updates_without_synthetic_final() {
        for next in ["Iwo plan poprawiony", "Iwo"] {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut state = state_for("partial-revision", 3.0);
            emit_stream_events(
                vec![
                    LiveStreamEvent::Partial {
                        text: "Iwo stary plan".into(),
                        segments: vec![segment("Iwo", 0.0, 0.5)],
                    },
                    LiveStreamEvent::Partial {
                        text: next.into(),
                        segments: vec![segment("Iwo", 0.0, 0.5)],
                    },
                ],
                &tx,
                &mut state,
                3.0,
            );
            assert_eq!(state.open_partial, next);
            assert_eq!(state.preview_rev, 2);
            let mut count = 0;
            while let Ok(event) = rx.try_recv() {
                match event {
                    EngineEvent::Preview { .. } => count += 1,
                    EngineEvent::UnadmittedAppleWords { .. } => {}
                    other => panic!("revision emitted {other:?}"),
                }
            }
            assert_eq!(count, 2);
        }
    }

    #[test]
    fn partial_revision_timing_matrix_does_not_borrow_invalid_onsets_or_tails() {
        let previous = [segment("Iwo", 0.0, 0.5)];
        assert!(apple_partials_are_disjoint(
            &previous,
            &[segment("Iwo", 0.5, 1.0)]
        ));
        for bad in [
            segment("Iwo", 0.0, 0.0),
            segment("Iwo", f32::NAN, 0.5),
            segment("Iwo", -0.1, 0.5),
            segment("", 0.0, 0.5),
        ] {
            let invalid_first = [bad.clone(), segment("Iwo", 1.0, 2.0)];
            assert!(!apple_partials_are_disjoint(&previous, &invalid_first));
            assert!(!apple_partials_are_disjoint(
                &[previous[0].clone(), bad],
                &[segment("Iwo", 3.0, 4.0)]
            ));
        }
        assert!(!apple_partials_are_disjoint(
            &[previous[0].clone(), segment("ogon", 2.0, 3.0)],
            &[segment("Iwo", 1.0, 1.5)]
        ));
        assert!(!apple_partials_are_disjoint(&previous, &[]));
    }

    #[test]
    fn partial_revision_bridge_final_does_not_freeze_again_on_next_partial() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state_for("partial-final-once", 3.0);
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "Iwo".into(),
                    segments: vec![segment("Iwo", 0.0, 0.5)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "Iwo".into(),
                    segments: vec![segment("Iwo", 0.0, 0.5)],
                },
            ],
            &tx,
            &mut state,
            3.0,
        );
        assert!(state.open_partial.is_empty());
        while rx.try_recv().is_ok() {}
        let phrase_id = state.phrase_id;
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "Iwo".into(),
                segments: vec![segment("Iwo", 1.0, 1.5)],
            }],
            &tx,
            &mut state,
            3.0,
        );
        let mut previews = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                EngineEvent::Preview { .. } => previews += 1,
                EngineEvent::UnadmittedAppleWords { .. } => {}
                other => panic!("post-final partial emitted {other:?}"),
            }
        }
        assert_eq!(previews, 1);
        assert_eq!(state.phrase_id, phrase_id);
    }

    /// rc-w3-acoustic-validity: invalid PCM *after* valid PCM, through the real
    /// writer -> owner -> ledger chain.
    ///
    /// One second of finite zeros, then one second of NaN, on one bound
    /// accumulator. The reader used to refuse only when the non-finite count
    /// reached the whole observed extent (`16_000 >= 32_000` is false), so the
    /// NaN second was measured as silence, the hop was published with a zero
    /// RMS, and an empty ledger certified the take Complete.
    #[test]
    fn invalid_pcm_after_valid_pcm_is_not_certified_as_silence() {
        let state = state_for("rc-w3-validity-after", 2.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.0f32; at(1.0) as usize]);
        writer.push_samples(&vec![f32::NAN; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability().as_str(),
            "invalid_measurement",
            "a NaN second measured nothing, and a valid second in front of it \
             does not turn it into silence"
        );
        assert!(!evidence.is_observed());
        assert!(evidence.ranges().is_empty());

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
            "an unmeasurable region may not reach a successful seal"
        );
        assert_eq!(receipt.coverage_ratio(), None);
        assert_eq!(receipt.observed_samples, None);
    }

    /// rc-w3-acoustic-validity: invalid PCM *before* valid PCM.
    ///
    /// The mirror case, with infinities rather than NaN — `is_finite` rejects
    /// both, and both are substituted with zero before measurement. The valid
    /// tail's speech may not be published on evidence whose head was never
    /// measurable.
    #[test]
    fn invalid_pcm_before_valid_pcm_is_not_certified_as_silence() {
        let state = state_for("rc-w3-validity-before", 2.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![f32::NEG_INFINITY; at(1.0) as usize]);
        writer.push_samples(&vec![0.25f32; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(
            evidence.availability().as_str(),
            "invalid_measurement",
            "an infinite head is as unmeasurable as a NaN one"
        );
        assert!(
            evidence.ranges().is_empty(),
            "unavailable evidence publishes no speech, not even the valid tail's"
        );

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement)
        );
    }

    /// rc-w3-acoustic-validity: a measuring observer cannot overrule the
    /// capture owner's invalid verdict on the same PCM.
    ///
    /// Silero reports a crossing over a take whose capture writer measured
    /// nothing but NaN. Selection preferred any observer that "measured
    /// speech", so the downstream observer won and the invalid capture never
    /// reached the ledger.
    #[test]
    fn a_measuring_observer_cannot_overrule_an_invalid_capture() {
        let state = one_burst_in_a_ten_second_take("rc-w3-validity-bypass");
        assert!(
            coverage_speech_evidence(&state).observed_speech(),
            "the fixture must start with a downstream observer that measured speech"
        );

        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![f32::NAN; at(10.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(
            evidence.producer(),
            CAPTURE_ENERGY_PRODUCER,
            "the capture owner adjudicates the PCM it wrote; no later observer \
             may certify audio the writer measured as invalid"
        );
        assert_eq!(evidence.availability().as_str(), "invalid_measurement");

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement)
        );
    }

    /// rc-w3-acoustic-validity: a partly observed capture cannot certify its
    /// unobserved tail, even when no word was committed inside that tail.
    ///
    /// Forwarding stops after four seconds of a ten-second take. No chunk ever
    /// skips, so nothing is discontinuous — the observer's extent simply ends
    /// early. Its one measured burst is committed, so the coverage arithmetic
    /// is perfect and the six unheard seconds were the whole lie: the ledger
    /// compared committed and measured spans against the observer's extent and
    /// never against the capture the take actually produced.
    #[test]
    fn a_partly_observed_capture_cannot_certify_its_unobserved_tail() {
        let mut state = state_for("rc-w3-validity-tail", 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress.note_observed_pcm(at(4.0), at(4.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        ]);
        ingress.observe(Some((0, at(4.0))), true, at(4.0));
        state.fusion = Some(ingress);

        let (tx, _rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);

        let evidence = coverage_speech_evidence(&state);
        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_ne!(
            receipt.status,
            SealCoverageStatus::Complete,
            "six seconds of this capture never reached any observer; covering \
             the measured burst does not certify them silent"
        );
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );
        assert_eq!(receipt.coverage_ratio(), None);
    }

    /// Context words before the occurrence stay out of the label and do not
    /// refuse it. The short occurrence is heard inside the 9s planned window
    /// that wholly contains it.
    #[test]
    fn short_flush_hears_eight_seconds_and_context_words_stay_out_of_the_label() {
        let mut state = measured_state_for("context-window", 10.0);
        let occurrence_end = at(10.0);
        let occurrence_len = (0.31 * RATE as f32).round() as u64;
        let occurrence_start = occurrence_end - occurrence_len;
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            occurrence_start,
            occurrence_end,
        );
        {
            let calibration = state.energy_calibration.clone().unwrap();
            let window = state
                .window_by_samples(occurrence_start, occurrence_end)
                .unwrap();
            let energy_integral = window
                .samples
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: 310.0,
                            energy_integral,
                            mean_rms_dbfs: -12.0,
                            peak_dbfs: -12.0,
                            vad_open_sample: Some(occurrence_start),
                            vad_close_sample: Some(occurrence_end),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration,
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(occurrence.clone(), vec![LedgerObservationProducer::Whisper]);
        }
        state.pending_events.insert(
            1,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: String::new(),
                layer1_baseline: String::new(),
                start_ts: occurrence_start as f32 / RATE as f32,
                end_ts: occurrence_end as f32 / RATE as f32,
                segments: Vec::new(),
            },
        );
        // The occurrence ends at 10s. Two more seconds, without stopping, make
        // the second full 9s window (3s..12s) the one that wholly contains it.
        state.audio.push(&vec![0.25f32; at(2.0) as usize]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(4);
        state.tail_patch = Some(tail_tx);
        state.capture_stopping = false;
        state.pump_capture_windows(&tx);
        let mut offered = Vec::new();
        while let Ok(request) = tail_rx.try_recv() {
            offered.push(request);
        }
        let request = offered
            .into_iter()
            .find(|request| window_contains(request, occurrence_start, occurrence_end))
            .expect("a full 9s window contains the short occurrence");
        let heard = request.admit_sample_end - request.admit_sample_start;
        assert_eq!(
            heard,
            9 * u64::from(RATE),
            "containing window is one 9s grid"
        );
        assert!(request.admit_sample_start < occurrence_start);
        assert!(window_contains(&request, occurrence_start, occurrence_end));

        let context_end = occurrence_start.saturating_sub(400);
        let context_start = request.provider_request.identity.range.sample_start + 800;
        let inside_start = occurrence_start + 200;
        let inside_end = occurrence_end - 200;
        let payload = TailProviderPayload {
            identity: request.provider_request.identity.clone(),
            text: "znajdz ICX".into(),
            segments: vec![
                TimedTailSegment {
                    confidence: None,
                    grain: crate::stt::tail_provider::TailSegmentGrain::Word,
                    text: "znajdz".into(),
                    range: TailSampleRange {
                        session: state.session_id.clone(),
                        capture_epoch: state.capture_epoch,
                        sample_start: context_start,
                        sample_end: context_end,
                    },
                },
                TimedTailSegment {
                    confidence: None,
                    grain: crate::stt::tail_provider::TailSegmentGrain::Word,
                    text: "ICX".into(),
                    range: TailSampleRange {
                        session: state.session_id.clone(),
                        capture_epoch: state.capture_epoch,
                        sample_start: inside_start,
                        sample_end: inside_end,
                    },
                },
            ],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.1),
            provider_id: crate::stt::tail_provider::TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: crate::stt::tail_provider::TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Word,
                source: crate::stt::tail_provider::TailEvidenceSource::Whisper,
                revision: Some("context-window".into()),
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: crate::stt::tail_provider::TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        };
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity.clone()),
                payload: Some(payload),
                member_occurrences: request.member_occurrences.clone(),
            },
            5.0,
        );
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(&occurrence),
            None,
            "9.8s is right context of the live 3..12s frame"
        );
        state.capture_stopping = true;
        state.pump_capture_windows(&tx);
        let mut saw_intersecting = false;
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::LedgerMutation { receipt, .. } = event
                && matches!(
                    receipt,
                    MutationReceipt::Refuse {
                        reason: RefuseReason::IntersectingPinNotExclusive,
                        ..
                    }
                )
            {
                saw_intersecting = true;
            }
        }
        assert!(
            !saw_intersecting,
            "context words must not refuse the occurrence"
        );
        assert!(
            !state.whisper_span_refused.contains(&occurrence),
            "the occurrence is wholly inside its admit range"
        );
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&occurrence)
                .map(str::to_owned),
            Some("ICX".to_string())
        );
    }
