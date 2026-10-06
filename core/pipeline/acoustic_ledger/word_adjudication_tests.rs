//! Integrator-owned counterexamples for local adjudication, independent of ASR.
use super::*;

fn fixture() -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new("local-word-trial", 1, 0, 160_000);
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(16_000);
    let calibration = EnergyCalibration::new("trial-test", 1.0, 1);
    assert!(
        ledger
            .qualify(
                &AcousticEvidence {
                    occurrence: owner.clone(),
                    duration_ms: 10_000.0,
                    energy_integral: 100.0,
                    mean_rms_dbfs: -20.0,
                    peak_dbfs: -10.0,
                    vad_open_sample: Some(0),
                    vad_close_sample: Some(160_000),
                    evidence_calibration_version: calibration.version.clone(),
                },
                &calibration
            )
            .is_qualified()
    );
    ledger.schedule_frontier(
        owner.clone(),
        [ObservationProducer::Apple, ObservationProducer::Whisper],
    );
    (ledger, owner)
}

fn offer(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    producer: ObservationProducer,
    generation: u64,
    label: &str,
    decode: Option<(u64, u64)>,
) {
    let mut pin = WordPin::new(48_000, 64_000, label);
    if let Some((s, e)) = decode {
        pin = pin.with_decode_window(s, e);
    }
    ledger.admit_word_slots(
        &ObservationIdentity::new(producer, 8, generation, owner.clone()),
        &[pin],
    );
}

fn disputed() -> (AcousticLedger, OccurrenceIdentity) {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        0,
        "56",
        None,
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "1286",
        Some((0, 128_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "1286",
        Some((8_000, 136_000)),
    );
    assert_eq!(ledger.text_of(&owner), Some("56"));
    (ledger, owner)
}

#[test]
fn wrong_apple_is_not_a_veto_against_a_confirmed_trial() {
    let (mut ledger, owner) = disputed();
    let trial = ledger
        .next_word_trial(true)
        .expect("unresolved local conflict");
    let pins = [WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 160_000)];
    let obs = ObservationIdentity::new(ObservationProducer::Whisper, 99, 3, owner.clone());
    ledger.admit_word_trial(&trial, &obs, &pins, &pins);
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    assert!(
        ledger
            .word_choices()
            .iter()
            .any(|c| c.reason == "trial_confirmed" && c.accepted)
    );
    assert!(!ledger.has_word_conflicts());
}

#[test]
fn late_correct_apple_survives_rank_refusal_and_can_win_a_trial() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "56",
        Some((0, 128_000)),
    );
    ledger.admit_word_slots(
        &ObservationIdentity::new(ObservationProducer::Apple, 8, 2, owner.clone()),
        &[WordPin::new(48_000, 64_000, "1286")],
    );
    assert_eq!(ledger.text_of(&owner), Some("56"));
    let trial = ledger
        .next_word_trial(true)
        .expect("late Apple disagreement retained");
    let pins = [WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 160_000)];
    ledger.admit_word_trial(
        &trial,
        &ObservationIdentity::new(ObservationProducer::Whisper, 99, 3, owner.clone()),
        &pins,
        &pins,
    );
    assert_eq!(ledger.text_of(&owner), Some("1286"));
}

#[test]
fn third_trial_variant_keeps_the_incumbent_and_does_not_retry_forever() {
    let (mut ledger, owner) = disputed();
    let trial = ledger.next_word_trial(true).unwrap();
    let pins = [WordPin::new(48_000, 64_000, "999").with_decode_window(0, 160_000)];
    ledger.admit_word_trial(
        &trial,
        &ObservationIdentity::new(ObservationProducer::Whisper, 99, 3, owner.clone()),
        &pins,
        &pins,
    );
    assert_eq!(ledger.text_of(&owner), Some("56"));
    assert!(ledger.next_word_trial(true).is_none());
    for generation in 4..14 {
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            generation,
            "999",
            Some((0, 160_000)),
        );
        assert!(ledger.next_word_trial(true).is_none());
    }
}

#[test]
fn a_manual_edit_invalidates_an_outstanding_trial() {
    let (mut ledger, owner) = disputed();
    let trial = ledger.next_word_trial(true).unwrap();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::ManualHuman,
        4,
        "ludzka",
        None,
    );
    let pins = [WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 160_000)];
    let receipt = ledger.admit_word_trial(
        &trial,
        &ObservationIdentity::new(ObservationProducer::Whisper, 99, 5, owner.clone()),
        &pins,
        &pins,
    );
    assert!(!receipt.grants_mutation());
    assert_eq!(ledger.text_of(&owner), Some("ludzka"));
}

#[test]
fn trial_context_cannot_add_or_rewrite_a_neighbor() {
    let (mut ledger, owner) = disputed();
    let neighbor = [WordPin::new(80_000, 96_000, "tekstów").with_decode_window(0, 160_000)];
    ledger.admit_word_slots(
        &ObservationIdentity::new(ObservationProducer::Whisper, 8, 3, owner.clone()),
        &neighbor,
    );
    let trial = ledger.next_word_trial(true).unwrap();
    let pins = [
        WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 160_000),
        WordPin::new(80_000, 96_000, "zmyślonych").with_decode_window(0, 160_000),
    ];
    ledger.admit_word_trial(
        &trial,
        &ObservationIdentity::new(ObservationProducer::Whisper, 99, 4, owner.clone()),
        &pins,
        &pins,
    );
    assert_eq!(ledger.text_of(&owner), Some("1286 tekstów"));
}

#[test]
fn rewritten_surfaces_do_not_fabricate_independent_source_agreement() {
    let (mut ledger, owner) = fixture();
    for (producer, generation, raw, surface, decode) in [
        (ObservationProducer::Apple, 0, "pierwsza", "zgodne", None),
        (
            ObservationProducer::Whisper,
            1,
            "druga",
            "zgodne",
            Some((0, 128_000)),
        ),
    ] {
        let obs = ObservationIdentity::new(producer, 8, generation, owner.clone());
        let mut original = WordPin::new(48_000, 64_000, raw);
        if let Some((s, e)) = decode {
            original = original.with_decode_window(s, e);
        }
        ledger.stage_word_evidence(&obs, &[original.clone()], None, "unknown");
        let mut rewritten = original;
        rewritten.text = surface.into();
        rewritten.surface_rewritten = true;
        ledger.admit_word_slots(&obs, &[rewritten]);
    }
    assert_eq!(ledger.text_of(&owner), Some("zgodne"));
    assert!(
        ledger.has_word_conflicts(),
        "two raw hypotheses already disagree despite one surface"
    );
    assert!(!ledger.word_choices().last().unwrap().lexical_resolved);
    let support = &ledger.word_choices().last().unwrap().support;
    assert!(
        support
            .iter()
            .any(|h| h.original_text.as_deref() == Some("pierwsza"))
    );
    assert!(
        support
            .iter()
            .any(|h| h.original_text.as_deref() == Some("druga"))
    );
    assert!(ledger.next_word_trial(true).is_some());
}

#[test]
fn context_quality_is_bounded_at_extreme_sample_offsets() {
    use super::word_adjudication::context_quality;
    assert_eq!(context_quality(2, 3, (0, 9)), 666_666);
    assert_eq!(context_quality(0, 1, (0, 9)), 0);
    assert_eq!(context_quality(0, 10, (0, 9)), 0);
    assert!(context_quality(u64::MAX / 3, u64::MAX / 2, (0, u64::MAX)) <= 1_000_000);
}

/// Old geometry fixtures now provide an explicit resolving observation instead
/// of granting a lexical correction solely because Whisper arrived later.
pub(crate) fn corroborate_candidate(
    ledger: &mut AcousticLedger,
    observation: &mut ObservationIdentity,
    pins: &[WordPin],
) -> MutationReceipt {
    let mut receipt = ledger.admit_word_slots(observation, pins);
    while let Some(trial) = ledger.next_word_trial(true) {
        assert_eq!(
            trial.owner, observation.occurrence,
            "fixture conflict scope"
        );
        *observation = ledger.next_word_observation(
            ObservationProducer::Whisper,
            10_000 + trial.id,
            &trial.owner,
        );
        receipt = ledger.admit_word_trial(&trial, observation, pins, pins);
    }
    receipt
}

#[test]
fn provisional_apple_can_evolve_until_another_source_has_spoken() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        0,
        "weryfikowałeś",
        None,
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        1,
        "zweryfikowałeś",
        None,
    );
    assert_eq!(ledger.text_of(&owner), Some("zweryfikowałeś"));
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "zweryfikowałeś",
        Some((0, 128_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        3,
        "inne",
        None,
    );
    assert_eq!(ledger.text_of(&owner), Some("zweryfikowałeś"));
    assert!(ledger.next_word_trial(true).is_some());
}

#[test]
fn a_same_label_update_and_trial_cannot_downgrade_a_complete_word_at_a_voiced_fence() {
    use crate::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CAPTURE_ENERGY_PRODUCER,
        CaptureEvidenceIdentity,
    };
    use crate::stt::tail_provider::TailSampleRange;
    let (mut ledger, owner) = fixture();
    ledger.record_speech_evidence(&AcousticSpeechEvidence::measured(
        CaptureEvidenceIdentity::new(&owner.session, owner.capture_epoch),
        CAPTURE_ENERGY_PRODUCER,
        AcousticAvailability::Observed {
            observed_samples: 160_000,
        },
        vec![TailSampleRange {
            session: owner.session.clone(),
            capture_epoch: owner.capture_epoch,
            sample_start: 48_000,
            sample_end: 64_000,
        }],
    ));
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        0,
        "1286",
        Some((0, 128_000)),
    );
    let original = ledger.slots_of(&owner).unwrap()[0].clone();
    assert!(ledger.complete_word_slot(&original));
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "1286",
        Some((0, 64_000)),
    );
    assert_eq!(
        ledger.slots_of(&owner).unwrap(),
        std::slice::from_ref(&original)
    );
    assert!(!ledger.coarse_word_source(&original));
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "56",
        Some((1_000, 64_000)),
    );
    let choice = ledger.word_choices().last().unwrap();
    assert!(choice.candidate.complete);
    assert!(!choice.candidate.acoustic_boundaries_complete);
    assert!(!choice.lexical_resolved);
    let trial = ledger
        .next_word_trial(true)
        .expect("unresolved lexical evidence remains explicit");
    let pins = [WordPin::new(48_000, 64_000, "56").with_decode_window(0, 64_000)];
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, 99, &owner);
    ledger.admit_word_trial(&trial, &observation, &pins, &pins);
    assert_eq!(ledger.slots_of(&owner).unwrap(), &[original]);
    assert!(ledger.word_finality(&owner)[0].unresolved);
    assert!(!ledger.word_choices().last().unwrap().lexical_resolved);
}

#[test]
fn stale_apple_cannot_revert_a_newer_provisional_word_or_open_a_conflict() {
    for decode in [None, Some((0, 128_000))] {
        let (mut ledger, owner) = fixture();
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Apple,
            2,
            "nowsze",
            None,
        );
        let before = ledger.slots_of(&owner).unwrap().to_vec();
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Apple,
            1,
            "starsze",
            decode,
        );
        assert_eq!(ledger.slots_of(&owner).unwrap(), before);
        assert_eq!(ledger.text_of(&owner), Some("nowsze"));
        assert!(!ledger.has_word_conflicts());
        let choice = ledger.word_choices().last().unwrap();
        assert!(!choice.accepted);
        assert!(!choice.lexical_resolved);
        assert_eq!(choice.reason, "stale_apple_generation");
    }
}

#[test]
fn late_apple_respects_duplicate_and_sealed_fences_before_adjudication() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        0,
        "1286",
        Some((0, 128_000)),
    );
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 8, 1, owner.clone());
    let pin = WordPin::new(48_000, 64_000, "1286");
    ledger.admit_word_slots(&apple, std::slice::from_ref(&pin));
    let choices = ledger.word_choices().len();
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    let repeated = ledger.admit_word_slots(&apple, std::slice::from_ref(&pin));
    assert!(matches!(
        repeated,
        MutationReceipt::Refuse {
            reason: RefuseReason::BatchDuplicate,
            ..
        }
    ));
    assert_eq!(ledger.word_choices().len(), choices);
    for producer in [ObservationProducer::Apple, ObservationProducer::Whisper] {
        ledger.note_frontier_return(&owner, producer);
    }
    let seal = ledger.seal(&owner).unwrap().clone();
    for generation in [1, 2] {
        let late =
            ObservationIdentity::new(ObservationProducer::Apple, 8, generation, owner.clone());
        let receipt = ledger.admit_word_slots(&late, std::slice::from_ref(&pin));
        assert!(matches!(
            receipt,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::LateAppleWordSealedOwner,
                ..
            }
        ));
        assert_eq!(ledger.word_choices().len(), choices);
        assert_eq!(ledger.slots_of(&owner).unwrap(), before);
        assert_eq!(ledger.seal_of(&owner), Some(&seal));
    }
}
