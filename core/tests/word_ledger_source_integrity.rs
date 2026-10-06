//! Admission regressions owned by the integrator. The number geometry comes
//! from the original gen-7/gen-9 trace, independently of the repair policy.
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity, WordPin,
};

fn ledger(rate: u32, start: u64, end: u64) -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new("word-source-regression", 1, start, end);
    let calibration = EnergyCalibration::new("word-source-regression", 1.0, 1);
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(rate);
    assert!(
        ledger
            .qualify(
                &AcousticEvidence {
                    occurrence: owner.clone(),
                    duration_ms: (end - start) as f64 * 1000.0 / f64::from(rate),
                    energy_integral: 100.0,
                    mean_rms_dbfs: -20.0,
                    peak_dbfs: -10.0,
                    vad_open_sample: Some(start),
                    vad_close_sample: Some(end),
                    evidence_calibration_version: calibration.version.clone(),
                },
                &calibration,
            )
            .is_qualified()
    );
    ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
    (ledger, owner)
}

fn admit(
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
fn authentic_1286_pin_survives_56_from_partial_window_and_neighbor_flows() {
    let (mut ledger, owner) = ledger(48_000, 2_875_392, 3_451_392);
    for (generation, start, end, pin_end) in [
        (3, 2_923_392, 3_355_392, 3_311_232),
        (5, 3_067_392, 3_451_392, 3_353_472),
        (7, 3_163_392, 3_595_392, 3_353_472),
    ] {
        admit(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            generation,
            &[WordPin::new(3_259_392, pin_end, "1286").with_decode_window(start, end)],
        );
    }
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        9,
        &[
            WordPin::new(3_333_312, 3_353_472, "56").with_decode_window(3_307_392, 3_739_392),
            WordPin::new(3_360_000, 3_410_000, "tekstów").with_decode_window(3_307_392, 3_739_392),
        ],
    );
    assert_eq!(ledger.text_of(&owner), Some("1286 tekstów"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
    assert!(!ledger.text_recovery_pending(&owner));
}

#[test]
fn partial_source_at_either_edge_cannot_shrink_an_existing_word() {
    for rate in [16_000_u32, 44_100, 48_000] {
        let r = u64::from(rate);
        for right in [false, true] {
            for jitter in [-1_i64, 0, 1] {
                let (mut ledger, owner) = ledger(rate, 0, 12 * r);
                admit(
                    &mut ledger,
                    &owner,
                    ObservationProducer::Whisper,
                    1,
                    &[WordPin::new(4 * r, 8 * r, "1286").with_decode_window(r, 11 * r)],
                );
                let boundary = (6 * r) as i64 + jitter;
                let pin = if right {
                    WordPin::new(4 * r, 5 * r, "56").with_decode_window(0, boundary as u64)
                } else {
                    WordPin::new(7 * r, 8 * r, "56").with_decode_window(boundary as u64, 12 * r)
                };
                admit(&mut ledger, &owner, ObservationProducer::Whisper, 2, &[pin]);
                assert_eq!(
                    ledger.text_of(&owner),
                    Some("1286"),
                    "rate={rate} right={right} jitter={jitter}"
                );
                assert!(!ledger.text_recovery_pending(&owner));
            }
        }
    }
}

#[test]
fn omitted_negation_remains_a_separate_physical_word() {
    let (mut ledger, owner) = ledger(16_000, 0, 64_000);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        &[
            WordPin::new(12_000, 18_000, "nie").with_decode_window(0, 64_000),
            WordPin::new(20_000, 36_000, "zrobił").with_decode_window(0, 64_000),
        ],
    );
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        &[WordPin::new(20_000, 36_000, "zrobił").with_decode_window(0, 64_000)],
    );
    assert_eq!(ledger.text_of(&owner), Some("nie zrobił"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
}

#[test]
fn a_missing_negation_can_be_inserted_after_apple() {
    let (mut ledger, owner) = ledger(16_000, 0, 64_000);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        1,
        &[WordPin::new(20_000, 36_000, "stoimy")],
    );
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        &[
            WordPin::new(12_000, 18_000, "nie").with_decode_window(0, 64_000),
            WordPin::new(20_000, 36_000, "stoimy").with_decode_window(0, 64_000),
        ],
    );
    assert_eq!(ledger.text_of(&owner), Some("nie stoimy"));
}

#[test]
fn duplicate_completions_do_not_create_words() {
    let (mut ledger, owner) = ledger(16_000, 0, 64_000);
    for _ in 0..10 {
        admit(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            1,
            &[WordPin::new(12_000, 36_000, "Iwo").with_decode_window(0, 64_000)],
        );
    }
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
}

#[test]
fn two_complete_windows_can_correct_an_earlier_wrong_number() {
    let (mut ledger, owner) = ledger(16_000, 0, 160_000);
    for (generation, label, window) in [
        (1, "56", (0, 128_000)),
        (2, "1286", (8_000, 136_000)),
        (3, "1286", (16_000, 144_000)),
    ] {
        admit(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            generation,
            &[WordPin::new(48_000, 64_000, label).with_decode_window(window.0, window.1)],
        );
        if generation == 2 {
            assert_eq!(ledger.text_of(&owner), Some("56"));
        }
    }
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    assert!(!ledger.text_recovery_pending(&owner));
}

#[test]
fn apple_and_whisper_agreement_is_not_overwritten_by_one_later_window() {
    let (mut ledger, owner) = ledger(16_000, 0, 160_000);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        1,
        &[WordPin::new(48_000, 64_000, "1286")],
    );
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        &[WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 128_000)],
    );
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        3,
        &[WordPin::new(48_000, 64_000, "56").with_decode_window(16_000, 144_000)],
    );
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    assert!(!ledger.text_recovery_pending(&owner));
}

#[test]
fn repeated_decode_with_new_generation_is_not_corroboration() {
    let (mut ledger, owner) = ledger(16_000, 0, 160_000);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        &[WordPin::new(48_000, 64_000, "1286").with_decode_window(0, 128_000)],
    );
    for generation in 2..12 {
        admit(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            generation,
            &[WordPin::new(48_000, 64_000, "56").with_decode_window(16_000, 144_000)],
        );
        assert_eq!(ledger.text_of(&owner), Some("1286"));
    }
}

#[test]
fn a_manual_word_survives_multiple_complete_asr_windows() {
    let (mut ledger, owner) = ledger(16_000, 0, 160_000);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::ManualHuman,
        1,
        &[WordPin::new(48_000, 64_000, "1286")],
    );
    for generation in 2..5 {
        admit(
            &mut ledger,
            &owner,
            ObservationProducer::Whisper,
            generation,
            &[WordPin::new(48_000, 64_000, "56").with_decode_window(generation * 8_000, 144_000)],
        );
    }
    assert_eq!(ledger.text_of(&owner), Some("1286"));
}

#[test]
fn authentic_thanks_cannot_replace_pomiedzy_as_a_single_new_witness() {
    let (mut ledger, owner) = ledger(48_000, 697_344, 820_736);
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        0,
        &[WordPin::new(697_344, 796_736, "pomiędzy")],
    );
    admit(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        &[WordPin::new(713_920, 761_920, "Dziękuję.").with_decode_window(593_920, 977_920)],
    );
    assert_eq!(ledger.text_of(&owner), Some("pomiędzy"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
    assert!(!ledger.text_recovery_pending(&owner));
}
