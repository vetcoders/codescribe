//! Integrator regressions for the production occurrence qualification path.
//! Synthetic capture proves admission geometry, not recognition accuracy.

use super::{
    AdmitWindow, AppleSealState, EnergyCalibration, EngineEvent, LedgerObservationProducer,
    MutationReceipt, OccurrenceIdentity, qualify_owned_occurrence,
};
use crate::audio::capture_receipt::CaptureLevelAccumulator;
use crate::stt::tail_provider::{TailSampleRange, TailSegmentGrain, TimedTailSegment};

const RATE: u32 = 1_000;
const FLOOR: f32 = 0.05;

fn capture(session: &str, pcm: &[f32]) -> AppleSealState {
    let mut state = AppleSealState::new_for_session(RATE, session.into(), 1);
    state.existence_floor_rms = Some(FLOOR);
    state.energy_calibration = Some(EnergyCalibration::new(
        "measured-floor-regression",
        f64::from(FLOOR).powi(2) * 64.0,
        20,
    ));
    state.audio.push(pcm);
    let mut capture = CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE);
    capture.push_samples(pcm);
    state
}

#[test]
fn long_sub_floor_noise_cannot_accumulate_into_an_occurrence() {
    let state = capture("sub-floor-noise", &[0.04; 1_000]);
    let owner = OccurrenceIdentity::new("sub-floor-noise", 1, 0, 1_000);

    assert!(!qualify_owned_occurrence(&state, &owner));
    assert!(!state.acoustic_ledger.lock().unwrap().is_qualified(&owner));
}

#[test]
fn short_burst_does_not_borrow_energy_from_background_noise() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..20].fill(0.06);
    let state = capture("short-burst", &pcm);
    let owner = OccurrenceIdentity::new("short-burst", 1, 0, 1_000);

    assert!(!qualify_owned_occurrence(&state, &owner));
}

#[test]
fn voice_outside_the_owned_span_cannot_qualify_its_noise() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..80].fill(0.06);
    let state = capture("outside-voice", &pcm);
    let owner = OccurrenceIdentity::new("outside-voice", 1, 100, 500);

    assert!(!qualify_owned_occurrence(&state, &owner));
}

#[test]
fn repeated_real_bursts_keep_both_physical_occurrences() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..80].fill(0.06);
    pcm[500..580].fill(0.06);
    let state = capture("two-real-bursts", &pcm);
    let first = OccurrenceIdentity::new("two-real-bursts", 1, 0, 500);
    let second = OccurrenceIdentity::new("two-real-bursts", 1, 500, 1_000);

    assert!(qualify_owned_occurrence(&state, &first));
    assert!(qualify_owned_occurrence(&state, &second));
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(ledger.serial_of(&first).is_some());
    assert!(ledger.serial_of(&second).is_some());
}

fn route_unanchored(pcm: &[f32], label: &str) -> Vec<EngineEvent> {
    let session = "measured-unanchored-pin";
    let mut state = capture(session, pcm);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    state.route_overlap_pins(
        &tx,
        AdmitWindow {
            request_id: 1,
            sample_start: 0,
            sample_end: pcm.len() as u64,
        },
        &[],
        &[TimedTailSegment {
            text: label.into(),
            range: TailSampleRange {
                session: session.into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: pcm.len() as u64,
            },
            grain: TailSegmentGrain::Word,
            confidence: None,
        }],
        LedgerObservationProducer::Whisper,
    );
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[test]
fn sub_floor_pin_cannot_reappear_as_visible_unanchored_text() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..20].fill(0.06);
    for label in ["Dziękuję.", "dowolne"] {
        let events = route_unanchored(&pcm, label);
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::LedgerMutation {
                receipt: MutationReceipt::Refuse { .. },
                ..
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            EngineEvent::LedgerMutation {
                receipt: MutationReceipt::KeepVisibleUnanchored { .. },
                ..
            }
        )));
    }
}

#[test]
fn real_unanchored_sound_keeps_its_visible_evidence() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..80].fill(0.06);
    assert!(
        route_unanchored(&pcm, "dowolne")
            .iter()
            .any(|event| matches!(
                event,
                EngineEvent::LedgerMutation {
                    receipt: MutationReceipt::KeepVisibleUnanchored { .. },
                    ..
                }
            ))
    );
}

#[test]
fn witness_return_after_admission_horizon_closes_word_conflict_before_seal() {
    use super::super::silero_fusion::UtteranceLedger;
    use super::{RoutedPin, RoutedWords, handle_dictation_witness_return, reconcile_silero_ledger};
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailProviderPayload, TailRequestIdentity, TailTimingQuality,
    };

    let session = "take-69de62f8-witness-conflict";
    let mut pcm = vec![0.04; 1_200];
    pcm[100..400].fill(0.20);
    let mut state = capture(session, &pcm);
    state.seal_ledger_enabled = true;
    state.apple_live_enabled = true;
    state.dictation_witness_armed = true;
    state.capture_stopping = true;
    let (tail_tx, _tail_rx) = tokio::sync::mpsc::channel(8);
    state.tail_patch = Some(tail_tx);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let mut physical = UtteranceLedger::new();
    physical.open_or_extend(session, 1, 100, 400);
    physical.close_open(400);
    assert!(reconcile_silero_ledger(&mut state, &tx, &physical, &[]));
    let owner = OccurrenceIdentity::new(session, 1, 100, 400);

    let whisper_pin = RoutedPin {
        index: 0,
        pin: OccurrenceIdentity::new(session, 1, 140, 360),
        text: "plan".into(),
        confidence: None,
    };
    assert!(state.admit_routed_words_with_decode_window(
        &tx,
        1,
        &owner,
        (1, Some((0, 1_200))),
        RoutedWords {
            pins: std::slice::from_ref(&whisper_pin),
            neighbours: &[],
        },
        LedgerObservationProducer::Whisper,
    ));
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .schedule_observer(owner.clone(), LedgerObservationProducer::AppleDictation);
    state.dictation_witness_waiting.push(owner.clone());
    state.dictation_witness_owners.push(owner.clone());
    state.dictation_witness_inflight = true;
    state.close_admission_horizon(&tx, u64::MAX);

    let witness_identity = TailRequestIdentity {
        request_id: 99,
        range: TailSampleRange {
            session: session.into(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 1_200,
        },
    };
    let witness_payload = TailProviderPayload {
        identity: witness_identity.clone(),
        text: "rod".into(),
        segments: vec![TimedTailSegment {
            text: "rod".into(),
            range: TailSampleRange {
                session: session.into(),
                capture_epoch: 1,
                sample_start: 140,
                sample_end: 360,
            },
            grain: TailSegmentGrain::Word,
            confidence: None,
        }],
        avg_logprob: None,
        compression_ratio: None,
        provider_id: TailProviderId::AppleDictation,
        elapsed_ms: 12,
        evidence: TailProviderEvidence {
            source: TailEvidenceSource::AppleDictation,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: None,
            segment_grain: TailSegmentGrain::Word,
            vad_selected_samples: None,
        },
    };
    handle_dictation_witness_return(&mut state, &tx, (witness_identity, Ok(witness_payload)));
    state.publish_resolved_word_seals(&tx);
    state.seal_remaining_at_session_end(&tx);

    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(
        ledger.is_sealed(&owner),
        "owner must seal without SealRefusal::FrontierOpen after witness return"
    );
    assert_eq!(ledger.text_of(&owner), Some("plan"));
}

#[test]
fn continuous_voiced_onset_before_silero_warmup_admits_leading_whisper_words() {
    use super::super::silero_fusion::{SileroIngress, UtteranceLedger};
    use super::{reconcile_silero_ledger, retain_whisper_words_with_own_speech};
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};

    const SESSION: &str = "take-69de62f8-voiced-onset";
    const SAMPLE_RATE: u32 = 48_000;
    const EXISTENCE_FLOOR: f32 = 0.003;
    const TOTAL_SAMPLES: usize = 600_000;

    let mut pcm = vec![0.0001_f32; TOTAL_SAMPLES];
    // Isolated pre-roll click (51840..54720) separated by a 400ms quiet valley.
    pcm[51_840..54_720].fill(0.02);
    // Continuous voiced speech anchor 73920..520704 where Silero's RNN crossed
    // SpeechStart late at 109056 (padded to 89856 by UtteranceLedger).
    pcm[73_920..520_704].fill(0.08);

    let mut state = AppleSealState::new_for_session(SAMPLE_RATE, SESSION.into(), 1);
    state.seal_ledger_enabled = true;
    state.existence_floor_rms = Some(EXISTENCE_FLOOR);
    state.energy_calibration = Some(EnergyCalibration::new(
        "take-69de62f8-onset-calibration",
        1.0,
        960,
    ));
    state.audio.push(&pcm);
    let mut capture = CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, SAMPLE_RATE);
    for chunk in pcm.chunks(960) {
        capture.push_samples(chunk);
    }

    let mut ingress = SileroIngress::new(SAMPLE_RATE, SESSION, 1);
    ingress.note_observed_pcm(TOTAL_SAMPLES as u64, TOTAL_SAMPLES as u64);
    ingress.observe_boundaries(&[
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 109_056,
            speech_probability: 0.85,
        },
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechEnd,
            sample: 517_632,
            speech_probability: 0.12,
        },
    ]);
    state.fusion = Some(ingress);

    let mut physical = UtteranceLedger::new();
    physical.open_or_extend(SESSION, 1, 89_856, 545_280);
    physical.close_open(545_280);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (tail_tx, _tail_rx) = tokio::sync::mpsc::channel(8);
    state.tail_patch = Some(tail_tx);

    assert!(reconcile_silero_ledger(&mut state, &tx, &physical, &[]));
    let expected_owner = OccurrenceIdentity::new(SESSION, 1, 73_920, 545_280);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .is_qualified(&expected_owner),
        "reconciled Silero utterance must clamp its start back to the continuous voiced onset 73920"
    );

    let pin = |text: &str, sample_start: u64, sample_end: u64| TimedTailSegment {
        text: text.into(),
        range: TailSampleRange {
            session: SESSION.into(),
            capture_epoch: 1,
            sample_start,
            sample_end,
        },
        grain: TailSegmentGrain::Word,
        confidence: None,
    };
    let candidates = vec![
        pin("klik", 51_840, 54_720),
        pin("A", 74_880, 81_600),
        pin("nie", 81_600, 92_160),
        pin("dałoby", 92_160, 108_480),
        pin("się", 108_480, 113_280),
    ];
    let kept = retain_whisper_words_with_own_speech(&mut state, &tx, 1, &candidates);
    let kept_texts: Vec<&str> = kept.iter().map(|word| word.text.as_str()).collect();
    assert_eq!(
        kept_texts,
        vec!["A", "nie", "dałoby", "się"],
        "isolated click before the quiet valley stays refused while continuous voiced onset words are admitted"
    );

    let routes = state.route_overlap_pins(
        &tx,
        AdmitWindow {
            request_id: 1,
            sample_start: 0,
            sample_end: 432_000,
        },
        &[(1, expected_owner.clone())],
        &kept,
        LedgerObservationProducer::Whisper,
    );
    let routed_texts: Vec<&str> = routes[0]
        .exclusive
        .iter()
        .map(|pin| pin.text.as_str())
        .collect();
    assert_eq!(routed_texts, vec!["A", "nie", "dałoby", "się"]);
}
