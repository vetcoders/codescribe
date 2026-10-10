//! Integrator-owned counterexamples for local adjudication, independent of ASR.
use super::*;

pub(crate) fn measured_ledger(owner: &OccurrenceIdentity, pcm: &[f32]) -> AcousticLedger {
    measured_ledger_at_rate(owner, pcm, 16_000)
}

fn measured_ledger_at_rate(
    owner: &OccurrenceIdentity,
    pcm: &[f32],
    sample_rate: u32,
) -> AcousticLedger {
    use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
    assert!(owner.sample_start < owner.sample_end && owner.sample_end <= pcm.len() as u64);
    let energy = CaptureEnergyOwner::bind(&owner.session, owner.capture_epoch);
    let mut writer = CaptureLevelAccumulator::bound_to(&energy);
    for chunk in pcm.chunks(320) {
        writer.push_samples(chunk);
    }
    let speech =
        energy.session_active_speech_ranges(&owner.session, owner.capture_epoch, sample_rate);
    assert_eq!(
        speech.availability().observed_samples(),
        Some(pcm.len() as u64)
    );
    let samples = &pcm[owner.sample_start as usize..owner.sample_end as usize];
    let integral = samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>();
    let peak = samples
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0_f64, f64::max);
    assert!(integral > 0.0 && peak > 0.0);
    let calibration = EnergyCalibration::new("word-fixture-pcm", 1.0, 1);
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(sample_rate);
    assert!(
        ledger
            .qualify(
                &AcousticEvidence {
                    occurrence: owner.clone(),
                    duration_ms: samples.len() as f64 * 1000.0 / f64::from(sample_rate),
                    energy_integral: integral,
                    mean_rms_dbfs: 20.0 * (integral / samples.len() as f64).sqrt().log10(),
                    peak_dbfs: 20.0 * peak.log10(),
                    vad_open_sample: Some(owner.sample_start),
                    vad_close_sample: Some(owner.sample_end),
                    evidence_calibration_version: calibration.version.clone(),
                },
                &calibration
            )
            .is_qualified()
    );
    ledger.record_speech_evidence(&speech);
    ledger.record_decode_fence_energy(&energy, pcm.len() as u64, None);
    ledger
}

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
fn dictionary_witnesses_require_distinct_complete_current_word_windows() {
    let owner = OccurrenceIdentity::new("dictionary-witness", 1, 0, 160_000);
    let mut pcm = vec![0.0; 160_000];
    pcm[48_000..64_000].fill(0.2);
    let mut ledger = measured_ledger(&owner, &pcm);
    ledger.schedule_frontier(
        owner.clone(),
        [ObservationProducer::Apple, ObservationProducer::Whisper],
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        0,
        "Alfaxone",
        None,
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "Alfaxone",
        Some((0, 128_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "Alfaxone",
        Some((0, 128_000)),
    );
    let target = SlotTarget::from(&ledger.slots_of(&owner).unwrap()[0]);
    assert_eq!(
        ledger
            .dictionary_witnesses(&owner, std::slice::from_ref(&target))
            .len(),
        1,
        "request replay is not independent"
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        3,
        "Alfaxone",
        Some((8_000, 136_000)),
    );
    let target = SlotTarget::from(&ledger.slots_of(&owner).unwrap()[0]);
    let witnesses = ledger.dictionary_witnesses(&owner, std::slice::from_ref(&target));
    assert_eq!(witnesses.len(), 2);
    assert!(witnesses.iter().all(|(_, raw)| raw == "Alfaxone"));
    let mut stale = target.clone();
    stale.sample_start += 1;
    assert!(ledger.dictionary_witnesses(&owner, &[stale]).is_empty());
    assert!(ledger.dictionary_witnesses(&owner, &[]).is_empty());
}

#[test]
fn dictionary_witnesses_refuse_missing_pins_and_unresolved_lexical_conflict() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Apple,
        0,
        "Alfaxone",
        None,
    );
    let target = SlotTarget::from(&ledger.slots_of(&owner).unwrap()[0]);
    assert!(
        ledger
            .dictionary_witnesses(&owner, std::slice::from_ref(&target))
            .is_empty()
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "Naloxone",
        Some((0, 128_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "Naloxone",
        Some((8_000, 136_000)),
    );
    assert!(ledger.has_word_conflicts());
    assert!(ledger.dictionary_witnesses(&owner, &[target]).is_empty());
    assert_eq!(ledger.text_of(&owner), Some("Alfaxone"));
}

#[test]
fn incomplete_decoder_fence_does_not_spend_a_later_complete_trial() {
    use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};

    let (mut ledger, owner) = disputed();
    let mut pcm = vec![0.0_f32; 160_000];
    pcm[48_000..64_000].fill(0.2);
    let energy = CaptureEnergyOwner::bind(&owner.session, owner.capture_epoch);
    let mut writer = CaptureLevelAccumulator::bound_to(&energy);
    for chunk in pcm.chunks(320) {
        writer.push_samples(chunk);
    }
    ledger.record_speech_evidence(&energy.session_active_speech_ranges(
        &owner.session,
        owner.capture_epoch,
        16_000,
    ));
    ledger.record_decode_fence_energy(&energy, pcm.len() as u64, None);

    let clipped = OccurrenceIdentity::new(&owner.session, owner.capture_epoch, 16_000, 64_000);
    assert!(ledger.decode_word_fence_incomplete(&owner, 16_000, 64_000, 48_000, 64_000));
    assert!(
        ledger
            .next_word_trial_in(false, Some(&clipped), None)
            .is_none(),
        "an edge-incomplete frame must not spend the component's only trial"
    );
    assert_eq!(ledger.text_of(&owner), Some("56"));
    assert!(ledger.has_word_conflicts());

    let complete = OccurrenceIdentity::new(&owner.session, owner.capture_epoch, 16_000, 144_000);
    let trial = ledger
        .next_word_trial_in(false, Some(&complete), None)
        .expect("the later fresh complete frame can still resolve the dispute");
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, 9, &owner);
    let pins = [WordPin::new(48_000, 64_000, "1286").with_decode_window(16_000, 144_000)];
    assert!(
        ledger
            .admit_word_trial(&trial, &observation, &pins, &pins)
            .grants_mutation()
    );
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    assert!(!ledger.has_word_conflicts());
    assert!(
        ledger
            .next_word_trial_in(false, Some(&complete), None)
            .is_none()
    );
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn ordinary_third_grid_window_adjudicates_without_phase_dependent_extra_work() {
    let (mut ledger, owner) = fixture();
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
    ledger.admit_word_slots(&apple, &[WordPin::new(128_000, 136_000, "56")]);

    // Same input sequence whether completions arrive live or in the Stop drain.
    // Fixed padding3s would exclude this third frame despite full source cover.
    for (generation, start, end, text) in [
        (1, 0, 144_000, "1286"),
        (2, 48_000, 192_000, "999"),
        (3, 96_000, 240_000, "1286"),
    ] {
        let frame = OccurrenceIdentity::new(owner.session.clone(), owner.capture_epoch, start, end);
        let trial = ledger.next_word_trial_in(false, Some(&frame), None);
        let observation = ObservationIdentity::new(
            ObservationProducer::Whisper,
            generation,
            generation,
            owner.clone(),
        );
        let pins = [WordPin::new(128_000, 136_000, text).with_decode_window(start, end)];
        if generation == 3 {
            let trial =
                trial.expect("two prior observations authorize this ordinary fresh witness");
            let receipt = ledger.admit_word_trial(&trial, &observation, &pins, &pins);
            assert!(receipt.grants_mutation());
        } else {
            assert!(
                trial.is_none(),
                "ordinary frames cannot spend a trial before two prior witnesses"
            );
        }
        ledger.admit_word_slots(&observation, &pins);
    }
    assert_eq!(ledger.text_of(&owner), Some("1286"));
    assert!(!ledger.has_word_conflicts());
    let replay =
        OccurrenceIdentity::new(owner.session.clone(), owner.capture_epoch, 96_000, 240_000);
    assert!(
        ledger
            .next_word_trial_in(false, Some(&replay), None)
            .is_none()
    );
    assert_eq!(ledger.text_of(&owner), Some("1286"));
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
fn expanded_word_requires_decode_coverage_of_its_current_pin() {
    let (mut ledger, owner) = fixture();
    for (generation, end, decode) in [(1, 64_000, (0, 128_000)), (2, 96_000, (0, 144_000))] {
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 8, generation, owner.clone());
        ledger.admit_word_slots(
            &observation,
            &[WordPin::new(48_000, end, "1286").with_decode_window(decode.0, decode.1)],
        );
    }
    let sources = ledger.slots_of(&owner).unwrap().to_vec();
    assert_eq!(sources[0].sample_end, 96_000);
    let observation = ObservationIdentity::new(ObservationProducer::Whisper, 8, 3, owner.clone());
    let pins = [WordPin::new(48_000, 64_000, "56").with_decode_window(0, 80_000)];
    ledger.prepare_word_evidence(&observation, &pins);
    assert!(
        !ledger.asr_source_scope_complete(&observation, &sources),
        "covering the original shorter pin cannot authorize cutting off the expanded word"
    );
    ledger.admit_word_slots(&observation, &pins);
    assert_eq!(ledger.slots_of(&owner).unwrap(), sources);
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

/// Fixture decoder results must identify another captured context explicitly;
/// renaming the original request does not provide a resolving observation.
pub(crate) fn corroborate_candidate(
    ledger: &mut AcousticLedger,
    observation: &mut ObservationIdentity,
    pins: &[WordPin],
    witness_frame: (u64, u64),
) -> MutationReceipt {
    let mut receipt = ledger.admit_word_slots(observation, pins);
    while let Some(trial) = ledger.next_word_trial(true) {
        assert_eq!(
            trial.owner, observation.occurrence,
            "fixture conflict scope"
        );
        assert!(witness_frame.0 < witness_frame.1);
        let captured = ledger
            .speech_evidence
            .as_ref()
            .and_then(|speech| speech.availability().observed_samples())
            .expect("a corroborating fixture needs measured capture PCM");
        assert!(witness_frame.1 <= captured);
        assert!(pins.iter().all(|pin| {
            pin.decode_sample_start.zip(pin.decode_sample_end) != Some(witness_frame)
                && witness_frame.0 <= pin.sample_start
                && pin.sample_end <= witness_frame.1
        }));
        let witness = pins
            .iter()
            .cloned()
            .map(|pin| pin.with_decode_window(witness_frame.0, witness_frame.1))
            .collect::<Vec<_>>();
        *observation = ledger.next_word_observation(
            ObservationProducer::Whisper,
            10_000 + trial.id,
            &trial.owner,
        );
        receipt = ledger.admit_word_trial(&trial, observation, &witness, &witness);
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
    let choices = ledger.word_choices().len();
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

#[test]
fn sealed_words_release_revision_hypotheses_and_keep_finality() {
    let (mut ledger, first) = fixture();
    let calibration = EnergyCalibration::new("trial-test", 1.0, 1);
    for index in 0..256u64 {
        let start = index * 160_000;
        let owner = OccurrenceIdentity::new(&first.session, 1, start, start + 160_000);
        if index != 0 {
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: owner.clone(),
                            duration_ms: 10_000.0,
                            energy_integral: 100.0,
                            mean_rms_dbfs: -20.0,
                            peak_dbfs: -10.0,
                            vad_open_sample: Some(start),
                            vad_close_sample: Some(start + 160_000),
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
        }
        let pin = WordPin::new(start + 48_000, start + 64_000, "1286");
        let apple = ObservationIdentity::new(ObservationProducer::Apple, index, 0, owner.clone());
        ledger.admit_word_slots(&apple, std::slice::from_ref(&pin));
        let whisper =
            ObservationIdentity::new(ObservationProducer::Whisper, index, 0, owner.clone());
        ledger.admit_word_slots(&whisper, &[pin.with_decode_window(start, start + 160_000)]);
        assert!(!ledger.word_choices().is_empty());
        for producer in [ObservationProducer::Apple, ObservationProducer::Whisper] {
            ledger.note_frontier_return(&owner, producer);
        }
        let seal = ledger.seal(&owner).unwrap().clone();
        assert!(!seal.word_finality.is_empty());
        assert_eq!(ledger.text_of(&owner), Some("1286"));
        assert!(
            ledger.word_choices().is_empty(),
            "closed word {index} kept revision payloads"
        );
        assert!(!ledger.has_word_conflicts());
        assert_eq!(ledger.seal_of(&owner), Some(&seal));
    }
    assert_eq!(ledger.len(), 256);
    assert_eq!(ledger.conservation().residue(), 0);
}

/// Regression family: the `56` take-over and "obiecujący" → "odwzujący".
/// Two edge-of-window recognitions agree on a non-word; neither sits in the
/// middle third of its decode window, so neither holds publication rights
/// over the banded incumbent.
#[test]
fn edge_window_agreement_cannot_overwrite_a_banded_incumbent() {
    let (mut ledger, owner) = fixture();
    // Banded incumbent: pin 48k..64k inside decode (0,144k) → full margin.
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "obiecujący",
        Some((0, 144_000)),
    );
    assert_eq!(ledger.text_of(&owner), Some("obiecujący"));
    // Two agreeing edge recognitions: margins 4k and 8k of 144k windows.
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "odwzujący",
        Some((44_000, 188_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        3,
        "odwzujący",
        Some((40_000, 184_000)),
    );
    assert_eq!(
        ledger.text_of(&owner),
        Some("obiecujący"),
        "edge agreement must inform, never overwrite"
    );
    assert!(
        ledger
            .word_choices()
            .iter()
            .any(|choice| choice.reason == "outside_publication_band"),
        "the refusal names the publication band"
    );
}

/// The same correction from inside the band is accepted: the band grants the
/// rights the edge was denied.
#[test]
fn banded_recognition_corrects_the_incumbent() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "obiecujący",
        Some((0, 144_000)),
    );
    // Edge proposal first (informs), banded confirmation second (writes).
    // The confirming window differs from the incumbent's — a replay of the
    // same decode is not fresh evidence — and its pin 48k..64k sits in the
    // middle third of (12k,120k): margins 36k/56k of a 108k window.
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        2,
        "odwzujący",
        Some((44_000, 188_000)),
    );
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        3,
        "odwzujący",
        Some((12_000, 120_000)),
    );
    assert_eq!(ledger.text_of(&owner), Some("odwzujący"));
}

/// First placement stays ungated: at recording start there is no earlier
/// window to own those seconds, and refusing edge words would lose them.
#[test]
fn first_placement_from_a_window_edge_still_lands() {
    let (mut ledger, owner) = fixture();
    offer(
        &mut ledger,
        &owner,
        ObservationProducer::Whisper,
        1,
        "początek",
        Some((44_000, 188_000)),
    );
    assert_eq!(ledger.text_of(&owner), Some("początek"));
}

/// Measured 2026-10-08 on a pl-PL replay: Apple's word pins ran 0.1-0.4 s
/// early against Whisper's, so a midpoint grouping paired Whisper "Niczego"
/// with Apple "Niczego nie" and Whisper "nie" with Apple "testował". A policy
/// letting that complete window rewrite the Apple group published "Niczego
/// testował" — the negation gone. Whatever fusion lands next must keep it.
#[test]
fn an_offset_whisper_window_cannot_drop_a_negation_apple_heard() {
    let (mut ledger, owner) = fixture();
    ledger.admit_word_slots(
        &ObservationIdentity::new(ObservationProducer::Apple, 8, 0, owner.clone()),
        &[
            WordPin::new(11_520, 20_640, "Niczego"),
            WordPin::new(20_640, 23_040, "nie"),
            WordPin::new(23_040, 35_040, "testował"),
        ],
    );
    let pins = [
        WordPin::new(8_000, 22_080, "Niczego"),
        WordPin::new(22_080, 27_200, "nie"),
        WordPin::new(27_200, 41_600, "testował,"),
    ]
    .map(|pin| pin.with_decode_window(0, 144_000));
    ledger.admit_word_slots(
        &ObservationIdentity::new(ObservationProducer::Whisper, 9, 1, owner.clone()),
        &pins,
    );
    let text = ledger.text_of(&owner).unwrap().to_owned();
    let words = text.split_whitespace().collect::<Vec<_>>();
    assert_eq!(
        words
            .iter()
            .filter(|word| word.to_lowercase() == "nie")
            .count(),
        1,
        "{text}"
    );
    let negation = words.iter().position(|word| *word == "nie").unwrap();
    assert!(words[negation + 1].starts_with("testował"), "{text}");
}

#[test]
fn a_shared_grid_refusal_does_not_spend_the_single_fresh_decode_budget() {
    let owner = OccurrenceIdentity::new("grid-refusal-fresh-budget", 1, 0, 160_000);
    let mut pcm = vec![0.0; 160_000];
    pcm[48_000..64_000].fill(0.2);
    let mut ledger = measured_ledger(&owner, &pcm);
    for (producer, generation, label, decode) in [
        (ObservationProducer::Apple, 0, "56", None),
        (ObservationProducer::Whisper, 1, "1286", Some((0, 128_000))),
        (
            ObservationProducer::Whisper,
            2,
            "1286",
            Some((8_000, 136_000)),
        ),
    ] {
        offer(&mut ledger, &owner, producer, generation, label, decode);
    }
    let coverage = OccurrenceIdentity::new(&owner.session, 1, 16_000, 160_000);
    let shared = ledger
        .next_word_trial_in(false, Some(&coverage), None)
        .unwrap();
    let observation = ObservationIdentity::new(ObservationProducer::Whisper, 38, 0, owner.clone());
    let wrong = [WordPin::new(48_000, 64_000, "I").with_decode_window(16_000, 160_000)];
    assert!(
        !ledger
            .admit_word_trial(&shared, &observation, &wrong, &wrong)
            .grants_mutation()
    );
    assert_eq!(ledger.text_of(&owner), Some("56"));
    let fresh = ledger
        .next_word_trial_before_horizon(false, None, Some((16_000, 0..160_000)), owner.sample_end)
        .expect("a reused wrong grid return did not request another native decode");
    ledger.close_word_trial(&fresh, "inference_failed");
    assert!(
        ledger
            .next_word_trial_before_horizon(
                false,
                None,
                Some((16_000, 0..160_000)),
                owner.sample_end,
            )
            .is_none(),
        "the one freshly requested decode has now been spent"
    );
}

#[test]
fn measured_numeric_context_ties_require_a_fresh_bounded_witness() {
    use super::word_adjudication::context_quality;
    // clip10 components2/3, translated by -5040000 capture samples. This
    // preserves their exact context ratios and distinct PCM identities.
    for (index, old_start, old_end, new_start, new_end, old_q, new_q) in [
        (2, 243_840, 284_160, 253_440, 278_400, 1_000_000, 693_333),
        (3, 284_160, 306_240, 278_400, 312_000, 873_333, 933_333),
    ] {
        let owner = OccurrenceIdentity::new(format!("numeric-trial-{index}"), 1, 227_968, 452_736);
        let mut pcm = vec![0.0; 624_000];
        pcm[old_start.min(new_start) as usize..old_end.max(new_end) as usize].fill(0.2);
        let mut ledger = measured_ledger(&owner, &pcm);
        let first = ObservationIdentity::new(ObservationProducer::Whisper, 36, 0, owner.clone());
        let old = [WordPin::new(old_start, old_end, "Czerin").with_decode_window(0, 432_000)];
        assert_eq!(context_quality(old_start, old_end, (0, 432_000)), old_q);
        assert!(ledger.admit_word_slots(&first, &old).grants_mutation());
        let later = ObservationIdentity::new(ObservationProducer::Whisper, 36, 1, owner.clone());
        let candidate =
            [WordPin::new(new_start, new_end, "9,").with_decode_window(144_000, 576_000)];
        assert_eq!(
            context_quality(
                old_start.min(new_start),
                old_end.max(new_end),
                (144_000, 576_000)
            ),
            new_q
        );
        assert!(
            !ledger
                .admit_word_slots(&later, &candidate)
                .grants_mutation()
        );
        assert_eq!(
            ledger.text_of(&owner),
            Some("Czerin"),
            "q is not lexical confidence"
        );
        let trial = ledger
            .next_word_trial_in(false, None, Some((48_000, 0..624_000)))
            .unwrap();
        let left = trial
            .source_ranges
            .iter()
            .map(|r| r.sample_start)
            .min()
            .unwrap()
            - 48_000;
        let right = trial
            .source_ranges
            .iter()
            .map(|r| r.sample_end)
            .max()
            .unwrap()
            + 48_000;
        assert!(right - left <= 144_000);
        let fresh = ObservationIdentity::new(ObservationProducer::Whisper, 99, 0, owner.clone());
        let pins = [WordPin::new(new_start, new_end, "9,").with_decode_window(left, right)];
        assert!(
            ledger
                .admit_word_trial(&trial, &fresh, &pins, &pins)
                .grants_mutation()
        );
        assert_eq!(ledger.text_of(&owner), Some("9,"));
        assert_eq!(ledger.slots_of(&owner).unwrap().len(), 1);
        assert!(!ledger.has_word_conflicts());
        assert!(
            ledger
                .next_word_trial_in(false, None, Some((48_000, 0..624_000)))
                .is_none()
        );
    }
}
