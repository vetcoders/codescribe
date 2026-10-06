//! Integrator-owned geometric counterexamples. These execute the real ledger
//! but are synthetic evidence; actual audio acceptance is a separate GUI gate.
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity, WordPin,
};

fn fixture() -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new("gui-word-geometry", 1, 0, 160_000);
    let calibration = EnergyCalibration::new("gui-word-geometry", 1.0, 1);
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(16_000);
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
    pins: &[WordPin],
) {
    let observation = ObservationIdentity::new(producer, 8, generation, owner.clone());
    ledger.admit_word_slots_for_tests(&observation, pins);
}

#[test]
fn five_close_physical_iwo_survive_other_observer_and_exact_replay() {
    let (mut ledger, owner) = fixture();
    let pins = (0..5)
        .map(|i| {
            WordPin::new(32_000 + i * 3_840, 35_200 + i * 3_840, "Iwo")
                .with_decode_window(0, 160_000)
        })
        .collect::<Vec<_>>();
    // Five disjoint pins, only 40ms apart. Text equality is not occurrence identity.
    offer(&mut ledger, &owner, ObservationProducer::Whisper, 1, &pins);
    let original = ledger
        .slots_of(&owner)
        .unwrap()
        .iter()
        .map(|p| (p.sample_start, p.sample_end))
        .collect::<Vec<_>>();
    assert_eq!(original.len(), 5);
    for i in 0..5 {
        let apple = WordPin::new(pins[i].sample_start, pins[i].sample_end, "Iwo");
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Apple,
            i as u64 + 2,
            &[apple],
        );
    }
    offer(&mut ledger, &owner, ObservationProducer::Whisper, 1, &pins);
    assert_eq!(ledger.text_of(&owner), Some("Iwo Iwo Iwo Iwo Iwo"));
    let retained = ledger
        .slots_of(&owner)
        .unwrap()
        .iter()
        .map(|p| (p.sample_start, p.sample_end))
        .collect::<Vec<_>>();
    assert_eq!(
        retained, original,
        "same observer replay cannot erase or create PCM occurrences"
    );
}

#[test]
fn exact_other_observer_pin_does_not_duplicate_one_word() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        1,
        &[WordPin::new(48_000, 64_000, "Iwo")],
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        &[WordPin::new(48_000, 64_000, "Iwo").with_decode_window(0, 160_000)],
    );
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
}

#[test]
fn successful_return_cannot_erase_separate_negation() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        &[
            WordPin::new(32_000, 40_000, "Nie").with_decode_window(0, 160_000),
            WordPin::new(40_640, 56_000, "podoba").with_decode_window(0, 160_000),
            WordPin::new(56_640, 64_000, "mi").with_decode_window(0, 160_000),
            WordPin::new(64_640, 72_000, "się").with_decode_window(0, 160_000),
        ],
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        &[
            WordPin::new(40_640, 56_000, "Podoba").with_decode_window(0, 160_000),
            WordPin::new(56_640, 64_000, "mi").with_decode_window(0, 160_000),
            WordPin::new(64_640, 72_000, "się").with_decode_window(0, 160_000),
        ],
    );
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 4);
    assert!(
        ledger
            .text_of(&owner)
            .unwrap()
            .to_lowercase()
            .starts_with("nie ")
    );
}

#[test]
fn complete_return_clears_speech_debt_while_retaining_lexical_disagreement() {
    for (end, expected_debt) in [(192_000, false), (136_000, true)] {
        let owner = OccurrenceIdentity::new("lexical-return-debt", 1, 16_000, 160_000);
        let calibration = EnergyCalibration::new("lexical-return-debt", 1.0, 1);
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 9_000.0,
                        energy_integral: 100.0,
                        mean_rms_dbfs: -20.0,
                        peak_dbfs: -10.0,
                        vad_open_sample: Some(16_000),
                        vad_close_sample: Some(160_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            1,
            &[
                WordPin::new(32_000, 64_000, "kot").with_decode_window(0, 144_000),
                WordPin::new(80_000, 112_000, "idzie").with_decode_window(0, 144_000),
            ],
        );
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Apple,
            2,
            &[WordPin::new(32_000, 64_000, "kod")],
        );
        assert!(ledger.require_text_recovery(&owner));
        offer(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            3,
            &[
                WordPin::new(32_000, 67_200, "kot").with_decode_window(0, end),
                WordPin::new(80_000, 112_000, "idzie").with_decode_window(0, end),
            ],
        );
        ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
        assert_eq!(ledger.text_of(&owner), Some("kot idzie"));
        assert!(
            ledger.word_choices().iter().any(|choice| !choice.accepted),
            "the lexical dispute is retained"
        );
        assert_eq!(
            ledger.text_recovery_pending(&owner),
            expected_debt,
            "successful complete word evidence pays speech debt, partial owner evidence does not"
        );
    }
}

#[test]
fn actual_live_boundary_geometry_cannot_render_a_hybrid_partition() {
    // These producer coordinates come from actual GUI take03. The calibration
    // is synthetic; this counterexample asserts transaction conservation, not
    // audio recognition accuracy or a new identity rule.
    let owner = OccurrenceIdentity::new("gui-take03-partition", 1, 1_182_720, 1_697_792);
    let calibration = EnergyCalibration::new("gui-take03-partition", 1.0, 1);
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(48_000);
    assert!(
        ledger
            .qualify(
                &AcousticEvidence {
                    occurrence: owner.clone(),
                    duration_ms: 10_730.667,
                    energy_integral: 100.0,
                    mean_rms_dbfs: -20.0,
                    peak_dbfs: -10.0,
                    vad_open_sample: Some(owner.sample_start),
                    vad_close_sample: Some(owner.sample_end),
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
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        11,
        &[WordPin::new(1_587_456, 1_645_056, "rozstrzyga tych dwóch")],
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        12,
        &[WordPin::new(1_645_056, 1_665_216, "spraw")],
    );
    assert_eq!(ledger.text_of(&owner), Some("rozstrzyga tych dwóch spraw"));
    let pins = [
        WordPin::new(1_593_152, 1_633_472, "rozstrzyga"),
        WordPin::new(1_633_472, 1_641_152, "tych"),
        WordPin::new(1_641_152, 1_658_432, "dwóch"),
        WordPin::new(1_658_432, 1_673_792, "spraw."),
    ]
    .map(|pin| pin.with_decode_window(1_313_792, 1_697_792));
    offer(&mut ledger, &owner, ObservationProducer::Whisper, 13, &pins);
    let text = ledger.text_of(&owner).unwrap();
    assert!(
        text == "rozstrzyga tych dwóch spraw" || text == "rozstrzyga tych dwóch spraw.",
        "preserve the complete incumbent or admit a complete authorized partition; hybrid: {text}"
    );
}
