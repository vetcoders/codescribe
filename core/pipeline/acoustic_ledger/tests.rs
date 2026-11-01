use super::*;

#[test]
fn anchor_identity_distinguishes_words_sharing_one_burst_by_declared_span() {
    let burst = OccurrenceIdentity::new("shared", 1, 1_000, 5_000);
    let first = WordAnchorIdentity::single(burst.clone(), (1_000, 3_000));
    let second = WordAnchorIdentity::single(burst.clone(), (3_000, 5_000));
    let first_again = WordAnchorIdentity::single(burst, (900, 3_100));
    assert!(first.shares_word(&first));
    assert!(first.shares_word(&first_again));
    assert!(!first.shares_word(&second));
}

#[test]
fn anchor_identity_rejects_a_foreign_capture_with_the_same_coordinates() {
    let first = WordAnchorIdentity::single(
        OccurrenceIdentity::new("first", 1, 1_000, 2_000),
        (1_000, 2_000),
    );
    let foreign = WordAnchorIdentity::single(
        OccurrenceIdentity::new("foreign", 1, 1_000, 2_000),
        (1_000, 2_000),
    );
    assert!(!first.shares_word(&foreign));
}

#[test]
fn matching_tokens_do_not_identify_different_speech_anchors() {
    assert_eq!(normalize_word_token("Iwo,"), normalize_word_token("iwo"));
    let first = WordAnchorIdentity::single(
        OccurrenceIdentity::new("same", 1, 1_000, 2_000),
        (1_000, 2_000),
    );
    let second = WordAnchorIdentity::single(
        OccurrenceIdentity::new("same", 1, 3_000, 4_000),
        (3_000, 4_000),
    );
    assert!(!first.shares_word(&second));
}

#[test]
fn pcm_centre_assigns_an_owner_and_is_not_slot_identity() {
    // The short word contains the long range's centre.
    assert!(declared_range_contains_midpoint(490, 510, 0, 1_000));
    // Slot identity can hold while the host misses the subject's centre.
    // 100..102 centres at 101, which 100..101 does not contain.
    assert!(!declared_range_contains_midpoint(100, 101, 100, 102));
    // Seam: neither centre sits in the other range.
    assert!(!declared_range_contains_midpoint(0, 1_200, 1_000, 1_400));
    assert!(!declared_range_contains_midpoint(1_000, 1_400, 0, 1_200));
    // A point is not a centre. Raw `end - start` used to treat the start
    // as one, and a reversed range underflowed.
    assert!(!declared_range_contains_midpoint(0, 2_000, 1_000, 1_000));
    assert!(!declared_range_contains_midpoint(0, 2_000, 2_000, 1_000));
    assert!(!declared_range_contains_midpoint(0, 2_000, u64::MAX, 0));
}

#[test]
fn window_owner_index_keeps_coarse_sealed_owners_and_half_open_bounds() {
    let mut ledger = AcousticLedger::new();
    let calibration = EnergyCalibration::new("owner-index-test", 1.0, 1);
    let owners = [
        ("index", 1, 0, 100_000),
        ("index", 1, 20_000, 50_000),
        ("index", 1, 40_000, 70_000),
        ("index", 1, 60_000, 80_000),
        ("index", 1, 0, 40_000),
        ("index", 2, 40_000, 60_000),
        ("foreign", 1, 40_000, 60_000),
    ]
    .map(|(session, epoch, start, end)| OccurrenceIdentity::new(session, epoch, start, end));
    for owner in &owners {
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 1_000.0,
                        energy_integral: 100.0,
                        mean_rms_dbfs: -20.0,
                        peak_dbfs: -10.0,
                        vad_open_sample: Some(owner.sample_start),
                        vad_close_sample: Some(owner.sample_end),
                        evidence_calibration_version: calibration.version.clone()
                    },
                    &calibration
                )
                .is_qualified()
        );
    }
    ledger.schedule_frontier(owners[0].clone(), [ObservationProducer::Whisper]);
    ledger.admit(
        &ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owners[0].clone()),
        "nie",
    );
    ledger.note_frontier_return(&owners[0], ObservationProducer::Whisper);
    ledger.seal(&owners[0]).unwrap();
    let window = OccurrenceIdentity::new("index", 1, 40_000, 60_000);
    assert_eq!(
        ledger
            .qualified_occurrences_overlapping(&window)
            .cloned()
            .collect::<Vec<_>>(),
        owners[..3]
    );
    let empty = OccurrenceIdentity::new("index", 1, 40_000, 40_000);
    assert_eq!(ledger.qualified_occurrences_overlapping(&empty).count(), 0);
}

#[test]
fn producer_index_preserves_out_of_order_ties_and_duplicate_receipts() {
    let (mut ledger, owner) = whisper_only_qualified_ledger();
    for (request, generation) in [(90, 9), (30, 3), (91, 9), (90, 9), (20, 2)] {
        ledger.admit(
            &ObservationIdentity::new(
                ObservationProducer::Whisper,
                request,
                generation,
                owner.clone(),
            ),
            "nie",
        );
    }
    let next = ledger.next_word_observation(ObservationProducer::Whisper, 999, &owner);
    assert_eq!((next.request, next.generation), (90, 10));
    let entries = ledger.layer_trail_for(&owner).collect::<Vec<_>>();
    assert_eq!(entries.len(), 5);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.predecessor_ordinal)
            .collect::<Vec<_>>(),
        [None, Some(0), Some(1), Some(2), Some(3)]
    );
    ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
    let seal = ledger.seal(&owner).unwrap();
    assert_eq!(seal.layer_trail_ordinals, [0, 1, 2, 3, 4]);
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn closed_history_does_not_expand_live_owner_work() {
    let mut ledger = AcousticLedger::new();
    let calibration = EnergyCalibration::new("bounded-history-test", 1.0, 1);
    let mut checkpoints = Vec::new();
    for index in 0..4_096u64 {
        let start = index * 16_000;
        let owner = OccurrenceIdentity::new("bounded-history", 1, start, start + 16_000);
        let evidence = AcousticEvidence {
            occurrence: owner.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(start + 16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(
            ledger
                .admit(
                    &ObservationIdentity::new(
                        ObservationProducer::Whisper,
                        index,
                        0,
                        owner.clone()
                    ),
                    "nie"
                )
                .is_insert()
        );
        assert_eq!(ledger.unsealed_qualified_occurrences().count(), 1);
        if index == 63 || index == 4_095 {
            let started = std::time::Instant::now();
            for _ in 0..10_000 {
                std::hint::black_box(ledger.next_word_observation(
                    ObservationProducer::Whisper,
                    9999,
                    &owner,
                ));
                assert_eq!(ledger.layer_trail_for(&owner).count(), 1);
                assert_eq!(ledger.unsealed_qualified_occurrences().count(), 1);
                assert_eq!(ledger.qualified_occurrences_overlapping(&owner).count(), 1);
            }
            checkpoints.push((index + 1, started.elapsed().as_micros()));
        }
        ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
        ledger.seal(&owner).unwrap();
        assert_eq!(ledger.unsealed_qualified_occurrences().count(), 0);
        // Requalification must never reopen a closed owner.
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        assert_eq!(ledger.unsealed_qualified_occurrences().count(), 0);
    }
    assert_eq!(ledger.qualified_occurrences().count(), 4_096);
    assert_eq!(ledger.conservation().residue(), 0);
    eprintln!(
        "bounded_history: queries=10000 checkpoints={checkpoints:?} live_owners=0 archive_owners=4096"
    );
}
use crate::audio::capture_receipt::CaptureEvidenceIdentity;

fn occ(start: u64, end: u64) -> OccurrenceIdentity {
    OccurrenceIdentity::new("s1", 1, start, end)
}

fn obs(
    producer: ObservationProducer,
    generation: u64,
    occurrence: OccurrenceIdentity,
) -> ObservationIdentity {
    ObservationIdentity::new(producer, 7, generation, occurrence)
}

fn whisper_only_qualified_ledger() -> (AcousticLedger, OccurrenceIdentity) {
    let occurrence = occ(0, 16_000);
    let mut ledger = AcousticLedger::new();
    let calibration = EnergyCalibration::new("label-finality", 1.0, 1);
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
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
    (ledger, occurrence)
}

#[test]
fn late_rejected_source_cannot_create_new_recovery_debt() {
    for held_by in [
        ObservationProducer::Whisper,
        ObservationProducer::ManualHuman,
    ] {
        let (mut ledger, occurrence) = whisper_only_qualified_ledger();
        let held = obs(held_by, 3, occurrence.clone());
        let receipt = ledger.admit_word_slots(&held, &[WordPin::new(1_000, 14_000, "confirmed")]);
        assert!(
            receipt.grants_mutation(),
            "initial source must be committed: {receipt:?}"
        );
        assert!(!ledger.text_recovery_pending(&occurrence));
        let before = ledger.slots_of(&occurrence).unwrap().to_vec();
        let stale = obs(ObservationProducer::Apple, 1, occurrence.clone());
        let receipt = ledger.admit_word_slots(&stale, &[WordPin::new(1_000, 14_000, "weaker")]);
        assert!(
            !receipt.grants_mutation(),
            "stale source must be refused: {receipt:?}"
        );
        assert_eq!(ledger.slots_of(&occurrence).unwrap(), before.as_slice());
        assert_eq!(ledger.text_of(&occurrence), Some("confirmed"));
        assert!(
            ledger
                .slot_alternatives()
                .iter()
                .any(|alternative| alternative.observation == stale)
        );
        assert!(
            !ledger.text_recovery_pending(&occurrence),
            "rejected weaker hypothesis created terminal work over an already committed source: {held_by:?}"
        );
    }
}

#[test]
fn word_slots_compose_with_their_own_ranges_and_one_occurrence_serial() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let observation = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(
        ledger
            .admit_pinned_label(
                &observation,
                "Iwo znowu",
                &[
                    crate::pipeline::acoustic_ledger::WordPin::new(1_000, 5_000, "Iwo"),
                    crate::pipeline::acoustic_ledger::WordPin::new(8_000, 14_000, "znowu")
                ],
            )
            .is_insert()
    );
    ledger.assert_slot_labels();
    assert_eq!(ledger.text_of(&occurrence), Some("Iwo znowu"));
    let composed = ledger.compose(&occurrence).expect("qualified words");
    assert_eq!(
        composed
            .tokens
            .iter()
            .map(|token| token.token.as_str())
            .collect::<Vec<_>>(),
        vec!["Iwo", "znowu"]
    );
    assert_eq!(composed.tokens[0].token_sample_start, Some(1_000));
    assert_eq!(composed.tokens[0].token_sample_end, Some(5_000));
    assert_eq!(composed.tokens[1].token_sample_start, Some(8_000));
    assert_eq!(composed.tokens[1].token_sample_end, Some(14_000));
    for slot in ledger.slots_of(&occurrence).unwrap() {
        assert_eq!(slot.observation, observation);
        assert_eq!(slot.witness, SlotWitness::Unwitnessed);
    }
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn derived_slot_label_survives_correction_preservation_refusal_and_human_edit() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(ledger.admit(&apple, "  Iwo  ").is_insert());
    ledger.assert_slot_labels();
    assert_eq!(ledger.text_of(&occurrence), Some("  Iwo  "));
    let whisper = obs(ObservationProducer::Whisper, 1, occurrence.clone());
    assert!(ledger.admit(&whisper, "Iwo wraca").is_correct());
    ledger.assert_slot_labels();
    assert!(matches!(
        ledger.admit(
            &obs(ObservationProducer::Lexicon, 2, occurrence.clone()),
            "Iwo wraca"
        ),
        MutationReceipt::Preserve { .. }
    ));
    ledger.assert_slot_labels();
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    ledger.seal(&occurrence).unwrap();
    assert!(matches!(
        ledger.admit(
            &obs(ObservationProducer::Apple, 3, occurrence.clone()),
            "zmiana"
        ),
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    ledger.assert_slot_labels();
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::ManualHuman, 4, occurrence.clone()),
                "Iwo zostaje"
            )
            .is_correct()
    );
    ledger.assert_slot_labels();
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].sample_start, occurrence.sample_start);
    assert_eq!(slots[0].sample_end, occurrence.sample_end);
    assert_eq!(slots[0].producer, ObservationProducer::ManualHuman);
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn pinned_first_label_admits_overlapping_owner() {
    for producer in [ObservationProducer::Apple, ObservationProducer::Whisper] {
        let mut ledger = AcousticLedger::new();
        let older = occ(0, 16_000);
        let newer = occ(12_000, 24_000);
        assert!(
            ledger
                .admit(&obs(ObservationProducer::Apple, 0, older.clone()), "older")
                .is_insert()
        );
        let observation = obs(producer, 1, newer.clone());
        let words = [crate::pipeline::acoustic_ledger::WordPin::new(
            13_000, 14_000, "newer",
        )];
        let receipt = if producer == ObservationProducer::Apple {
            ledger.admit_pinned_label(&observation, "newer", &words)
        } else {
            ledger.admit_word_slots(&observation, &words)
        };
        assert!(receipt.is_insert());
        assert_eq!(ledger.text_of(&older), Some("older"));
        assert_eq!(ledger.text_of(&newer), Some("newer"));
        let slots = ledger.slots_of(&newer).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(
            (slots[0].sample_start, slots[0].sample_end),
            (13_000, 14_000)
        );
        assert_eq!(slots[0].observation, observation);
        assert_eq!(ledger.layer_trail_for(&newer).count(), 1);
        assert_eq!(ledger.conservation().residue(), 0);
        ledger.assert_slot_labels();
    }
}

#[test]
fn invalid_first_label_pins_cannot_bypass_overlap() {
    for words in [
        vec![],
        vec![crate::pipeline::acoustic_ledger::WordPin::new(
            13_000, 13_000, "newer",
        )],
        vec![crate::pipeline::acoustic_ledger::WordPin::new(
            1_000, 2_000, "newer",
        )],
        vec![crate::pipeline::acoustic_ledger::WordPin::new(
            13_000,
            14_000,
            "different",
        )],
        vec![
            crate::pipeline::acoustic_ledger::WordPin::new(13_000, 15_000, "new"),
            crate::pipeline::acoustic_ledger::WordPin::new(14_000, 16_000, "er"),
        ],
    ] {
        let mut ledger = AcousticLedger::new();
        let older = occ(0, 16_000);
        let newer = occ(12_000, 24_000);
        assert!(
            ledger
                .admit(&obs(ObservationProducer::Apple, 0, older.clone()), "older")
                .is_insert()
        );
        assert!(matches!(
            ledger.admit_pinned_label(
                &obs(ObservationProducer::Apple, 1, newer.clone()),
                "newer",
                &words,
            ),
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::OverlapWithoutWordPins,
                ..
            }
        ));
        assert_eq!(ledger.text_of(&older), Some("older"));
        assert_eq!(ledger.text_of(&newer), None);
        assert!(ledger.slots_of(&newer).is_none());
        assert_eq!(ledger.layer_trail_for(&newer).count(), 1);
        assert_eq!(ledger.conservation().residue(), 0);
    }
}

#[test]
fn forensic_pinned_label_unproven_retiming_then_stronger_word() {
    let (mut ledger, owner, _) = forensic_recovery_measured_ledger();
    let first = obs(ObservationProducer::Apple, 0, owner.clone());
    assert!(
        ledger
            .admit_pinned_label(&first, "Iwo", &[WordPin::new(0, 8_000, "inne")])
            .is_insert()
    );
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    assert_eq!(ledger.slots_of(&owner).unwrap()[0].sample_end, 16_000);
    let serial = ledger.serial_of(&owner).unwrap().clone();
    let next = ledger.next_word_observation(ObservationProducer::Apple, 7, &owner);
    let retimed = ledger.admit_pinned_label(&next, "Iwo", &[WordPin::new(2_000, 8_000, "Iwo")]);
    assert!(
        matches!(
            retimed,
            MutationReceipt::Refuse {
                reason: RefuseReason::SlotAdmissionRejected,
                ..
            }
        ),
        "unproven Apple geometry must not retime the coarse source: {retimed:?}"
    );
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    let apple = ledger.slots_of(&owner).unwrap().to_vec();
    assert_eq!(apple.len(), 1);
    assert_eq!((apple[0].sample_start, apple[0].sample_end), (0, 16_000));
    assert_eq!(apple[0].producer, ObservationProducer::Apple);
    let word = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    let corrected = ledger.admit_word_slots(
        &word,
        &forensic_recovery_word_pins("wraca", Some((0, 16_000))),
    );
    assert!(
        corrected.grants_mutation(),
        "stronger same-span wording: {corrected:?}"
    );
    assert_eq!(ledger.text_of(&owner), Some("wraca"));
    let slots = ledger.slots_of(&owner).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!((slots[0].sample_start, slots[0].sample_end), (2_000, 8_000));
    assert_eq!(slots[0].producer, ObservationProducer::Whisper);
    assert!(
        ledger
            .slot_operations()
            .iter()
            .any(|operation| operation.observation == word
                && operation.sources.contains(&apple[0])
                && operation.outputs.contains(&slots[0]))
    );
    assert_eq!(ledger.serial_of(&owner), Some(&serial));
    assert!(!ledger.is_sealed(&owner));
    ledger.assert_slot_labels();
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn forensic_pinned_label_seal_fences_automatic_but_allows_document_edit() {
    let (mut ledger, owner, _) = forensic_recovery_measured_ledger();
    let first = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    assert!(
        ledger
            .admit_word_slots(
                &first,
                &forensic_recovery_word_pins("Iwo wraca", Some((0, 16_000)))
            )
            .is_insert()
    );
    assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
    let seal = ledger.seal(&owner).unwrap().clone();
    let serial = ledger.serial_of(&owner).unwrap().clone();
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    let late_apple = ledger.next_word_observation(ObservationProducer::Apple, 7, &owner);
    assert!(matches!(
        ledger.admit_pinned_label(
            &late_apple,
            "zmiana",
            &[WordPin::new(2_000, 8_000, "zmiana")]
        ),
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    let late_word = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    let refused = ledger.admit_word_slots(
        &late_word,
        &forensic_recovery_word_pins("późne słowa", Some((0, 16_000))),
    );
    assert!(
        matches!(
            refused,
            MutationReceipt::KeepVisibleUnanchored {
                reason: NoAuthorityReason::LateWhisperWordSealedOwner,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(!refused.grants_mutation());
    assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
    assert_eq!(ledger.text_of(&owner), Some("Iwo wraca"));
    assert_eq!(ledger.seal_of(&owner), Some(&seal));
    assert_eq!(ledger.serial_of(&owner), Some(&serial));
    assert_eq!(ledger.post_seal_decisions(&owner).len(), 2);
    assert!(!ledger.text_recovery_pending(&owner));
    let edited = ledger
        .record_manual_document_revision(
            "s1",
            0,
            1,
            "cały zmieniony akapit",
            std::slice::from_ref(&owner),
            DocumentRevisionProvenance::UserEdit,
        )
        .expect("human document edit");
    assert!(ledger.authenticates_manual_document_revision(&edited));
    assert_eq!(edited.source_occurrences, vec![owner.clone()]);
    assert_eq!(edited.source_seal_receipts, vec![seal.receipt_id.clone()]);
    assert_eq!(edited.rendered_text, "cały zmieniony akapit");
    assert_eq!(ledger.manual_document_revisions().len(), 1);
    assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
    assert_eq!(ledger.text_of(&owner), Some("Iwo wraca"));
    assert_eq!(ledger.seal_of(&owner), Some(&seal));
    assert_eq!(ledger.serial_of(&owner), Some(&serial));
    assert!(!ledger.text_recovery_pending(&owner));
    ledger.assert_slot_labels();
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn word_ranges_cannot_rewrite_a_label_or_a_sealed_occurrence() {
    let (mut ledger, occurrence, _) = forensic_recovery_measured_ledger();
    let observation = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(
        ledger
            .admit_pinned_label(
                &observation,
                "Iwo",
                &[crate::pipeline::acoustic_ledger::WordPin::new(
                    0, 8_000, "inne"
                )]
            )
            .is_insert()
    );
    assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
    assert_eq!(ledger.slots_of(&occurrence).unwrap()[0].sample_end, 16_000);
    let next = ledger.next_word_observation(ObservationProducer::Apple, 0, &occurrence);
    assert!(matches!(
        ledger.admit_pinned_label(
            &next,
            "Iwo",
            &[crate::pipeline::acoustic_ledger::WordPin::new(
                1_000, 8_000, "Iwo"
            )]
        ),
        MutationReceipt::Refuse {
            reason: RefuseReason::SlotAdmissionRejected,
            ..
        }
    ));
    assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
    assert_eq!(ledger.slots_of(&occurrence).unwrap()[0].sample_end, 16_000);
    let whisper = ledger.next_word_observation(ObservationProducer::Whisper, 42, &occurrence);
    assert!(matches!(
        ledger.admit_word_slots(
            &whisper,
            &forensic_recovery_word_pins("Iwo", Some((0, 16_000)))
        ),
        MutationReceipt::Preserve { .. }
    ));
    assert_eq!(ledger.text_of(&occurrence), Some("Iwo"));
    assert_eq!(
        (
            ledger.slots_of(&occurrence).unwrap()[0].sample_start,
            ledger.slots_of(&occurrence).unwrap()[0].sample_end
        ),
        (2_000, 8_000)
    );
    assert!(!ledger.text_recovery_pending(&occurrence));
    let before = ledger.slots_of(&occurrence).unwrap().to_vec();
    ledger.assert_slot_labels();
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    let seal = ledger.seal(&occurrence).unwrap().clone();
    let late = ledger.next_word_observation(ObservationProducer::Apple, 0, &occurrence);
    assert!(matches!(
        ledger.admit_pinned_label(
            &late,
            "Iwo",
            &[crate::pipeline::acoustic_ledger::WordPin::new(
                2_000, 9_000, "Iwo"
            )]
        ),
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    ledger.assert_slot_labels();
    assert_eq!(ledger.slots_of(&occurrence).unwrap(), before.as_slice());
    assert_eq!(ledger.seal_of(&occurrence), Some(&seal));
    assert_eq!(ledger.conservation().residue(), 0);
}

fn debt_speech() -> AcousticSpeechEvidence {
    measured_speech(
        "s1",
        1,
        16_000,
        vec![TailSampleRange {
            session: "s1".to_string(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 16_000,
        }],
    )
}

#[test]
fn terminal_finality_observes_issued_receipts_without_minting_them() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occurrence.clone()),
        "words",
    );
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    let coverage = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 0);
    assert!(ledger.record_seal_coverage(coverage.clone()));
    ledger.seal(&occurrence).unwrap();
    let counts = (
        ledger.seals.len(),
        ledger.terminal_seals.len(),
        ledger.layer_trail().len(),
    );
    let refusal = ledger.terminal_finality("s1", 1).into_refusal().unwrap();
    assert_eq!(
        refusal.reason(),
        TerminalFinalityRefusalReason::TerminalReceiptMissing
    );
    assert_eq!(refusal.coverage(), Some(&coverage));
    assert_eq!(
        counts,
        (
            ledger.seals.len(),
            ledger.terminal_seals.len(),
            ledger.layer_trail().len()
        )
    );
    let issued = ledger.seal_terminal("s1", 1).unwrap();
    assert_eq!(
        ledger.terminal_finality("s1", 1),
        TerminalFinality::Sealed(issued)
    );
    for (session, epoch) in [("s1", 2), ("foreign", 1)] {
        let refusal = ledger
            .terminal_finality(session, epoch)
            .into_refusal()
            .unwrap();
        assert_eq!(refusal.session_id(), session);
        assert_eq!(refusal.capture_epoch(), epoch);
        assert!(refusal.coverage().is_none());
    }
}

#[test]
fn terminal_finality_rejects_a_receipt_before_new_same_capture_occurrence() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occurrence.clone()),
        "words",
    );
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    let coverage = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 0);
    ledger.record_seal_coverage(coverage.clone());
    ledger.seal_terminal("s1", 1).unwrap();
    assert!(matches!(
        ledger.terminal_finality("s1", 1),
        TerminalFinality::Sealed(_)
    ));
    let next = occ(16_000, 32_000);
    let calibration = EnergyCalibration::new("label-finality", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: next.clone(),
        duration_ms: 1_000.0,
        energy_integral: 100.0,
        mean_rms_dbfs: -20.0,
        peak_dbfs: -10.0,
        vad_open_sample: Some(16_000),
        vad_close_sample: Some(32_000),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    let count = ledger.terminal_seals.len();
    let refusal = ledger.terminal_finality("s1", 1).into_refusal().unwrap();
    assert_eq!(
        refusal.reason(),
        TerminalFinalityRefusalReason::TerminalReceiptMissing
    );
    assert_eq!(refusal.coverage(), Some(&coverage));
    assert!(!ledger.committed.contains_key(&next));
    assert_eq!(ledger.terminal_seals.len(), count);
}

#[test]
fn terminal_finality_complete_coverage_does_not_hide_missing_label_or_debt() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, occ(0, 8_000)),
        "partial span",
    );
    let speech = measured_speech(
        "s1",
        1,
        16_000,
        vec![TailSampleRange {
            session: "s1".into(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 8_000,
        }],
    );
    let coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    assert!(coverage.status.is_complete());
    ledger.record_seal_coverage(coverage.clone());
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    assert_eq!(
        ledger.seal_terminal("s1", 1),
        Err(SealRefusal::LabelMissing)
    );
    let refusal = ledger.terminal_finality("s1", 1).into_refusal().unwrap();
    assert_eq!(
        refusal.reason(),
        TerminalFinalityRefusalReason::TerminalReceiptMissing
    );
    assert_eq!(refusal.coverage(), Some(&coverage));
    ledger.require_text_recovery(&occurrence);
    assert_eq!(
        ledger
            .terminal_finality("s1", 1)
            .into_refusal()
            .unwrap()
            .reason(),
        TerminalFinalityRefusalReason::TextRecoveryPending
    );
}

#[test]
fn terminal_finality_requires_observed_silence_not_absent_evidence() {
    let mut ledger = AcousticLedger::new();
    assert!(ledger.has_no_capture_facts());
    assert!(ledger.terminal_finality("s1", 1).into_refusal().is_some());
    let silence = measured_speech("s1", 1, 16_000, vec![]);
    let coverage = ledger.assess_seal_coverage("s1", 1, &silence, 0);
    ledger.record_seal_coverage(coverage.clone());
    assert_eq!(
        ledger.terminal_finality("s1", 1),
        TerminalFinality::ObservedSilence(coverage)
    );
    assert!(!ledger.has_no_capture_facts());
    let unavailable = AcousticSpeechEvidence::unavailable(
        crate::audio::capture_receipt::CaptureEvidenceIdentity::new("s1", 1),
        "capture_energy",
        AcousticAvailability::NotObserved,
    );
    ledger.record_seal_coverage(ledger.assess_seal_coverage("s1", 1, &unavailable, 0));
    assert_eq!(
        ledger
            .terminal_finality("s1", 1)
            .into_refusal()
            .unwrap()
            .reason(),
        TerminalFinalityRefusalReason::CoverageRefused
    );
}

#[test]
fn text_recovery_debt_retains_apple_but_refuses_false_coverage_and_seals() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, occurrence.clone()),
        "partial",
    );
    let before = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 32_000);
    assert_eq!(before.coverage_ratio(), Some(1.0));
    assert!(ledger.record_seal_coverage(before));
    assert!(ledger.require_text_recovery(&occurrence));
    assert!(ledger.require_text_recovery(&occurrence));
    assert_eq!(
        ledger.pending_text_recoveries("s1", 1),
        vec![occurrence.clone()]
    );
    let after = ledger.assess_seal_coverage("s1", 1, &debt_speech(), 32_000);
    assert_eq!(after.status, SealCoverageStatus::Incomplete);
    assert_eq!(after.coverage_ratio(), Some(0.0));
    assert_eq!(after.max_uncovered_samples, 16_000);
    // Take 9608b50e: five committed windows, each owing text recovery.
    // covered=0 is the truthful receipt. The defect is that the stop path
    // never recovers those occurrences.
    let mut take = AcousticLedger::new();
    take.bind_capture_rate(48_000);
    let committed = [
        (972_288, 1_548_288, "bramki"),
        (1_548_288, 2_875_904, "brief"),
        (2_878_464, 3_454_464, "evidence"),
        (3_454_464, 3_926_016, "zero"),
        (4_217_856, 4_627_968, "wiadomosc"),
    ];
    let calibration = EnergyCalibration::new("take-9608", 1.0, 1);
    for (index, (start, end, label)) in committed.iter().copied().enumerate() {
        let occurrence = OccurrenceIdentity::new("9608b50e", 1, start, end);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(take.qualify(&evidence, &calibration).is_qualified());
        take.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(
            take.admit(
                &obs(ObservationProducer::Apple, index as u64, occurrence.clone()),
                label,
            )
            .grants_mutation()
        );
        assert!(take.require_text_recovery(&occurrence));
    }
    let speech = measured_speech(
        "9608b50e",
        1,
        5_363_200,
        vec![
            (972_288, 1_423_872),
            (2_549_760, 2_840_064),
            (2_878_464, 3_428_352),
            (3_495_936, 3_886_080),
            (3_912_192, 4_194_816),
            (4_217_856, 4_584_960),
        ]
        .into_iter()
        .map(|(sample_start, sample_end)| TailSampleRange {
            session: "9608b50e".into(),
            capture_epoch: 1,
            sample_start,
            sample_end,
        })
        .collect(),
    );
    let coverage = take.assess_seal_coverage("9608b50e", 1, &speech, 12_000);
    assert_eq!(
        take.pending_text_recoveries("9608b50e", 1).len(),
        5,
        "the fixture is five debt occurrences: {coverage:?}"
    );
    assert_eq!(coverage.status, SealCoverageStatus::Incomplete);
    assert_eq!(
        coverage.covered_samples, 0,
        "debt must not certify coverage: {coverage:?}"
    );
    assert_eq!(coverage.speech_samples, 2_331_648);
    assert_eq!(ledger.text_of(&occurrence), Some("partial"));
    ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occurrence.clone()),
        " ",
    );
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    assert!(ledger.text_recovery_pending(&occurrence));
    assert_eq!(
        ledger.seal(&occurrence),
        Err(SealRefusal::TextRecoveryPending)
    );
    assert_eq!(
        ledger.seal_terminal("s1", 1),
        Err(SealRefusal::TextRecoveryPending)
    );
}

// Root-only forensic controls. These measure fixture PCM through the
// production capture accumulator and feed the real ledger admission path;
// they are not microphone/model acceptance or production calibration.
fn forensic_recovery_measured_ledger()
-> (AcousticLedger, OccurrenceIdentity, AcousticSpeechEvidence) {
    use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
    let owner = occ(0, 16_000);
    let mut pcm = vec![0.0_f32; 16_000];
    pcm[2_000..8_000].fill(0.2);
    let energy = CaptureEnergyOwner::bind("s1", 1);
    let mut writer = CaptureLevelAccumulator::bound_to(&energy);
    for hop in pcm.chunks(1_000) {
        writer.push_samples(hop);
    }
    let speech = energy.session_active_speech_ranges("s1", 1, 16_000);
    assert_eq!(speech.ranges().len(), 1);
    assert_eq!(
        (
            speech.ranges()[0].sample_start,
            speech.ranges()[0].sample_end
        ),
        (2_000, 8_000)
    );
    let energy_integral = pcm
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>();
    let rms = (energy_integral / pcm.len() as f64).sqrt();
    let calibration = EnergyCalibration::new("forensic-recovery-fixture", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: owner.clone(),
        duration_ms: 1_000.0,
        energy_integral,
        mean_rms_dbfs: 20.0 * rms.log10(),
        peak_dbfs: 20.0 * 0.2_f64.log10(),
        vad_open_sample: Some(0),
        vad_close_sample: Some(16_000),
        evidence_calibration_version: calibration.version.clone(),
    };
    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(16_000);
    assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    ledger.record_speech_evidence(&speech);
    ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
    (ledger, owner, speech)
}

fn forensic_recovery_word_pins(label: &str, window: Option<(u64, u64)>) -> Vec<WordPin> {
    let words = label.split_whitespace().collect::<Vec<_>>();
    assert!(!words.is_empty() && words.len() <= 6);
    words
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let start = 2_000 + 6_000 * i as u64 / words.len() as u64;
            let end = 2_000 + 6_000 * (i + 1) as u64 / words.len() as u64;
            let pin = WordPin::new(start, end, *text);
            match window {
                Some((lo, hi)) => pin.with_decode_window(lo, hi),
                None => pin,
            }
        })
        .collect()
}

#[test]
fn recovery_diagnostics_separates_missing_pcm_from_committed_debt_without_settling_it() {
    let (mut ledger, owner, speech) = forensic_recovery_measured_ledger();
    let missing = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    let diagnostic = ledger.coverage_diagnostics(&missing);
    assert_eq!(
        diagnostic.missing_committed_speech,
        missing
            .uncovered_speech_ranges
            .iter()
            .map(OccurrenceIdentity::from)
            .collect::<Vec<_>>()
    );
    assert!(diagnostic.text_recovery.is_empty());

    assert!(
        ledger
            .admit(&obs(ObservationProducer::Apple, 0, owner.clone()), "Iwo")
            .grants_mutation()
    );
    assert!(ledger.require_text_recovery(&owner));
    let debt = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    assert_eq!(debt.covered_samples, 0);
    assert!(!debt.uncovered_speech_ranges.is_empty());
    let diagnostic = ledger.coverage_diagnostics(&debt);
    assert!(diagnostic.missing_committed_speech.is_empty());
    assert_eq!(diagnostic.text_recovery.len(), 1);
    assert_eq!(diagnostic.text_recovery[0].owner, owner);
    assert!(diagnostic.text_recovery[0].committed && diagnostic.text_recovery[0].pending);
    assert!(ledger.record_seal_coverage(debt));
    let before = ledger.layer_trail().to_vec();
    let truth = crate::pipeline::take_truth::TakeTruth::observe_ledger(&ledger, "s1", 1);
    let mut value = serde_json::to_value(&truth).unwrap();
    let restored: crate::pipeline::take_truth::TakeTruth =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        restored
            .ledger
            .as_ref()
            .unwrap()
            .coverage
            .as_ref()
            .unwrap()
            .diagnostics
            .as_ref(),
        Some(&diagnostic)
    );
    let serialized = serde_json::to_string(&diagnostic).unwrap();
    assert!(
        !serialized.contains("Iwo"),
        "diagnostics must not export candidate labels"
    );
    value["ledger"]["coverage"]
        .as_object_mut()
        .unwrap()
        .remove("diagnostics");
    let old: crate::pipeline::take_truth::TakeTruth = serde_json::from_value(value).unwrap();
    assert!(old.ledger.unwrap().coverage.unwrap().diagnostics.is_none());
    assert_eq!(ledger.layer_trail(), before);
    assert_eq!(ledger.text_of(&owner), Some("Iwo"));
    assert!(ledger.text_recovery_pending(&owner));
    assert!(ledger.seal(&owner).is_err());

    let returned = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    ledger.admit_word_slots(
        &returned,
        &forensic_recovery_word_pins("Iwo", Some((0, 16_000))),
    );
    assert!(!ledger.text_recovery_pending(&owner));
    let cleared = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    let diagnostic = ledger.coverage_diagnostics(&cleared);
    assert!(diagnostic.text_recovery.is_empty());
    assert!(diagnostic.missing_committed_speech.is_empty());
    assert_eq!(cleared.covered_samples, cleared.speech_samples);
}

#[test]
fn recovery_diagnostics_retains_exact_refused_pin_and_filters_foreign_capture() {
    let (mut ledger, owner, speech) = forensic_recovery_measured_ledger();
    ledger.admit(&obs(ObservationProducer::Apple, 0, owner.clone()), "held");
    let rejected = obs(ObservationProducer::Whisper, 3, owner.clone());
    let pin = WordPin::new(2_000, 8_000, "edge").with_decode_window(0, 9_000);
    ledger.retain_rejected_decode_word(&rejected, &pin);
    assert!(ledger.text_recovery_pending(&owner));
    let snapshot = ledger.recovery_diagnostic(&owner);
    assert_eq!(snapshot.rejected_pins.len(), 1);
    let actual = &snapshot.rejected_pins[0];
    assert_eq!(actual.observation, rejected);
    assert_eq!((actual.sample_start, actual.sample_end), (2_000, 8_000));
    assert_eq!(actual.decode, Some((0, 9_000)));
    assert!(!actual.accepted_observation);
    assert!(
        actual
            .refusal_reasons
            .iter()
            .any(|reason| reason == "decode_window_clipped")
    );
    let foreign = OccurrenceIdentity::new("foreign", 9, 0, 16_000);
    ledger.pending_text_recovery.insert(foreign.clone());
    let mut coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    coverage.uncovered_speech_ranges.push(TailSampleRange {
        session: foreign.session,
        capture_epoch: foreign.capture_epoch,
        sample_start: foreign.sample_start,
        sample_end: foreign.sample_end,
    });
    let diagnostic = ledger.coverage_diagnostics(&coverage);
    assert_eq!(diagnostic.text_recovery, vec![snapshot]);
    assert!(
        diagnostic
            .missing_committed_speech
            .iter()
            .all(|range| range.same_capture(&owner))
    );
    assert_eq!(ledger.text_of(&owner), Some("held"));
}

// Independent terminal-observer controls: fixture PCM is measured through
// CaptureEnergy, then valid fields are changed one at a time. These are
// ledger boundary probes, not a claim that live observers mint bad ranges.
fn forensic_coverage_observer_settled()
-> (AcousticLedger, OccurrenceIdentity, AcousticSpeechEvidence) {
    let (mut ledger, owner, speech) = forensic_recovery_measured_ledger();
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, 91, &owner);
    let receipt = ledger.admit_word_slots(
        &observation,
        &forensic_recovery_word_pins("Iwo", Some((0, 16_000))),
    );
    assert!(receipt.grants_mutation() || matches!(receipt, MutationReceipt::Preserve { .. }));
    assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
    ledger.seal(&owner).expect("actual measured scope returned");
    (ledger, owner, speech)
}

#[test]
fn forensic_coverage_observer_actual_pcm_positive_and_pending_controls() {
    use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
    use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;
    let (unheard, _, speech) = forensic_recovery_measured_ledger();
    let pending = unheard.assess_seal_coverage("s1", 1, &speech, 0);
    assert_eq!(pending.status, SealCoverageStatus::Incomplete);
    assert_eq!(pending.speech_samples, 6_000);
    assert_eq!(pending.covered_samples, 0);
    let (mut settled, owner, speech) = forensic_coverage_observer_settled();
    let original_slots = settled.slots_of(&owner).unwrap().to_vec();
    for evidence in [
        speech.clone(),
        AcousticSpeechEvidence::measured(
            speech.identity().clone(),
            SILERO_BOUNDARIES_PRODUCER,
            speech.availability(),
            speech.ranges().to_vec(),
        ),
    ] {
        let coverage = settled.assess_seal_coverage("s1", 1, &evidence, 0);
        assert_eq!(coverage.status, SealCoverageStatus::Complete);
        assert_eq!(
            (coverage.speech_samples, coverage.covered_samples),
            (6_000, 6_000)
        );
        assert_eq!(coverage.observed_samples, Some(16_000));
        assert!(settled.record_seal_coverage(coverage));
        settled
            .seal_terminal("s1", 1)
            .expect("valid acoustic observer");
        assert_eq!(settled.slots_of(&owner).unwrap(), original_slots);
    }
    let energy = CaptureEnergyOwner::bind("quiet-control", 1);
    let mut writer = CaptureLevelAccumulator::bound_to(&energy);
    writer.push_samples(&vec![0.0; 16_000]);
    let quiet = energy.session_active_speech_ranges("quiet-control", 1, 16_000);
    assert!(quiet.is_observed() && quiet.ranges().is_empty());
    let coverage = AcousticLedger::new().assess_seal_coverage("quiet-control", 1, &quiet, 0);
    assert_eq!(coverage.status, SealCoverageStatus::Complete);
    assert_eq!(coverage.observed_samples, Some(16_000));
}

#[test]
fn forensic_coverage_observer_malformed_ranges_cannot_certify_terminal() {
    let variants = [
        ("reversed", 8_000, 2_000),
        ("zero_length", 2_000, 2_000),
        ("reversed_past_extent", 20_000, 18_000),
    ];
    let mut results = Vec::new();
    for (name, start, end) in variants {
        let (mut ledger, owner, speech) = forensic_coverage_observer_settled();
        let held = ledger.slots_of(&owner).unwrap().to_vec();
        let mut ranges = speech.ranges().to_vec();
        ranges[0].sample_start = start;
        ranges[0].sample_end = end;
        let altered = AcousticSpeechEvidence::measured(
            speech.identity().clone(),
            speech.producer(),
            speech.availability(),
            ranges,
        );
        let verdict = ledger.assess_seal_coverage("s1", 1, &altered, 0);
        println!(
            "{name}: {:?}, ratio={:?}, speech_samples={}",
            verdict.status,
            verdict.coverage_ratio(),
            verdict.speech_samples
        );
        assert!(ledger.record_seal_coverage(verdict.clone()));
        let terminal = ledger.seal_terminal("s1", 1);
        println!("{name}: terminal_minted={}", terminal.is_ok());
        results.push((name, verdict, terminal));
        assert_eq!(ledger.slots_of(&owner).unwrap(), held);
    }
    for (name, receipt, terminal) in results {
        assert_eq!(terminal, Err(SealRefusal::CoverageIncomplete), "{name}");
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
            "{name}"
        );
        assert_eq!(receipt.coverage_ratio(), None, "{name}");
    }
}

#[test]
fn forensic_coverage_observer_non_acoustic_producer_cannot_certify_terminal() {
    let mut results = Vec::new();
    for producer in ["responses_formatter", "whisper", "text_producer"] {
        let (mut ledger, owner, speech) = forensic_coverage_observer_settled();
        let held = ledger.slots_of(&owner).unwrap().to_vec();
        let altered = AcousticSpeechEvidence::measured(
            speech.identity().clone(),
            producer,
            speech.availability(),
            speech.ranges().to_vec(),
        );
        let verdict = ledger.assess_seal_coverage("s1", 1, &altered, 0);
        println!(
            "{producer}: {:?}, ratio={:?}, speech_samples={}",
            verdict.status,
            verdict.coverage_ratio(),
            verdict.speech_samples
        );
        assert!(ledger.record_seal_coverage(verdict.clone()));
        let terminal = ledger.seal_terminal("s1", 1);
        println!("{producer}: terminal_minted={}", terminal.is_ok());
        results.push((producer, verdict, terminal));
        assert_eq!(ledger.slots_of(&owner).unwrap(), held);
    }
    for (producer, receipt, terminal) in results {
        assert_eq!(terminal, Err(SealRefusal::CoverageIncomplete), "{producer}");
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
            "{producer}"
        );
        assert_eq!(receipt.coverage_ratio(), None, "{producer}");
    }
}

#[test]
fn forensic_recovery_word_scope_distinguishes_missing_partial_and_full_decode() {
    for window in [None, Some((0, 9_000)), Some((0, 16_000))] {
        let (mut ledger, owner, speech) = forensic_recovery_measured_ledger();
        assert!(
            ledger
                .admit(
                    &obs(ObservationProducer::Apple, 0, owner.clone()),
                    "partial"
                )
                .grants_mutation()
        );
        assert!(ledger.require_text_recovery(&owner));
        let foreign = ObservationIdentity::new(
            ObservationProducer::Whisper,
            42,
            0,
            OccurrenceIdentity::new("other-capture", 2, 0, 16_000),
        );
        ledger.admit_word_slots(&foreign, &forensic_recovery_word_pins("Iwo", window));
        assert!(ledger.text_recovery_pending(&owner));
        let observation = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
        let decision =
            ledger.admit_word_slots(&observation, &forensic_recovery_word_pins("Iwo", window));
        let full = window == Some((0, 16_000));
        assert_eq!(
            ledger.text_recovery_pending(&owner),
            !full,
            "{window:?}: {decision:?}"
        );
        assert!(!ledger.is_sealed(&owner), "an open producer is not a seal");
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Whisper));
        if full {
            assert!(
                decision.grants_mutation() || matches!(decision, MutationReceipt::Preserve { .. })
            );
            assert_eq!(ledger.text_of(&owner), Some("Iwo"));
            let slots = ledger.slots_of(&owner).unwrap();
            assert_eq!(slots.len(), 1);
            assert_eq!((slots[0].sample_start, slots[0].sample_end), (2_000, 8_000));
            assert_eq!(slots[0].producer, ObservationProducer::Whisper);
            let coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
            assert_eq!(coverage.coverage_ratio(), Some(1.0));
            assert!(ledger.record_seal_coverage(coverage));
            ledger.seal(&owner).expect("completed measured owner");
            ledger
                .seal_terminal("s1", 1)
                .expect("completed measured take");
        } else {
            assert!(
                ledger.text_recovery_pending(&owner),
                "frontier return cannot complete missing decoder scope"
            );
            assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
            assert_eq!(
                ledger.seal_terminal("s1", 1),
                Err(SealRefusal::TextRecoveryPending)
            );
        }
        ledger.assert_slot_labels();
        assert_eq!(ledger.conservation().residue(), 0);
    }
}

#[test]
fn forensic_recovery_agreement_requires_fresh_word_scope_not_replay() {
    let (mut ledger, owner, _) = forensic_recovery_measured_ledger();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, owner.clone()),
        "same words",
    );
    assert!(ledger.require_text_recovery(&owner));
    let label = obs(ObservationProducer::Whisper, 0, owner.clone());
    assert!(matches!(
        ledger.admit(&label, "same words"),
        MutationReceipt::Preserve { .. }
    ));
    assert!(ledger.text_recovery_pending(&owner));
    let pins = forensic_recovery_word_pins("same words", Some((0, 16_000)));
    let word_observation = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    assert!(matches!(
        ledger.admit_word_slots(&word_observation, &pins),
        MutationReceipt::Preserve { .. }
    ));
    assert!(!ledger.text_recovery_pending(&owner));
    assert_eq!(ledger.text_of(&owner), Some("same words"));
    // Option A (Maciej, 2026-10-21): a range a complete window heard owes
    // nothing, so no new debt opens on it.
    assert!(!ledger.require_text_recovery(&owner));
    // Replaying the same observation still cannot mutate.
    assert!(matches!(
        ledger.admit_word_slots(&word_observation, &pins),
        MutationReceipt::Refuse { .. }
    ));
    assert!(!ledger.text_recovery_pending(&owner));
    let stale = obs(ObservationProducer::Whisper, 0, owner.clone());
    ledger.admit(&stale, "same words");
    assert_eq!(ledger.text_of(&owner), Some("same words"));
    let fresh = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    assert!(fresh.generation > word_observation.generation);
    assert!(matches!(
        ledger.admit_word_slots(&fresh, &pins),
        MutationReceipt::Preserve { .. }
    ));
    assert!(!ledger.text_recovery_pending(&owner));
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn forensic_recovery_label_generation_and_explicit_human_authority() {
    let (mut ledger, owner, _) = forensic_recovery_measured_ledger();
    ledger.admit(
        &obs(ObservationProducer::Whisper, 4, owner.clone()),
        "heard words",
    );
    assert!(ledger.require_text_recovery(&owner));
    ledger.admit(
        &obs(ObservationProducer::Whisper, 3, owner.clone()),
        "heard words",
    );
    assert!(ledger.text_recovery_pending(&owner));
    ledger.admit(
        &obs(ObservationProducer::Whisper, 5, owner.clone()),
        "heard words",
    );
    assert!(
        ledger.text_recovery_pending(&owner),
        "fresh labels are not decoder-work receipts"
    );
    let fresh = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
    ledger.admit_word_slots(
        &fresh,
        &forensic_recovery_word_pins("heard words", Some((0, 16_000))),
    );
    assert!(!ledger.text_recovery_pending(&owner));
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    // Option A: the complete window heard the owner, so it owes nothing.
    assert!(!ledger.require_text_recovery(&owner));
    ledger.admit(
        &obs(ObservationProducer::Formatter, 6, owner.clone()),
        "invented words",
    );
    assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
    assert!(!ledger.text_recovery_pending(&owner));
    ledger.admit(&obs(ObservationProducer::ManualHuman, 7, owner.clone()), "");
    assert!(!ledger.text_recovery_pending(&owner));
    // The UI's whole-document edit enters the document-receipt corridor,
    // not untargeted raw-label admission over already pinned words.
    let revision = ledger
        .record_manual_document_revision(
            "s1",
            0,
            1,
            "human words",
            std::slice::from_ref(&owner),
            DocumentRevisionProvenance::UserEdit,
        )
        .expect("explicit document edit over committed sources");
    assert!(ledger.authenticates_manual_document_revision(&revision));
    assert_eq!(revision.rendered_text, "human words");
    assert_eq!(revision.source_occurrences, vec![owner.clone()]);
    assert!(revision.source_seal_receipts.is_empty());
    assert_eq!(ledger.slots_of(&owner).unwrap(), before.as_slice());
    assert_eq!(ledger.text_of(&owner), Some("heard words"));
    assert!(
        !ledger.text_recovery_pending(&owner),
        "a human document edit reopens no debt the complete window paid"
    );
    assert!(!ledger.is_sealed(&owner), "a document edit is not a seal");
    assert_eq!(ledger.conservation().residue(), 0);

    // An explicit human decision on an unpinned provisional label keeps
    // the separate whole-owner recovery permission exercised by the old
    // agreement fixture. No acoustic word geometry is manufactured.
    let (mut unpinned, owner, _) = forensic_recovery_measured_ledger();
    unpinned.admit(
        &obs(ObservationProducer::Apple, 0, owner.clone()),
        "same words",
    );
    assert!(unpinned.require_text_recovery(&owner));
    unpinned.admit(&obs(ObservationProducer::ManualHuman, 3, owner.clone()), "");
    assert!(unpinned.text_recovery_pending(&owner));
    unpinned.admit(
        &obs(ObservationProducer::ManualHuman, 4, owner.clone()),
        "human words",
    );
    assert!(!unpinned.text_recovery_pending(&owner));
    assert_eq!(unpinned.text_of(&owner), Some("human words"));
    assert_eq!(unpinned.conservation().residue(), 0);
}

#[test]
fn text_recovery_debt_resolves_only_after_exact_authorized_observation() {
    let (mut ledger, occurrence, speech) = forensic_recovery_measured_ledger();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, occurrence.clone()),
        "partial",
    );
    assert!(ledger.require_text_recovery(&occurrence));
    let foreign = OccurrenceIdentity::new("foreign", 1, 0, 16_000);
    ledger.admit(
        &obs(ObservationProducer::Whisper, 0, foreign),
        "foreign words",
    );
    assert!(ledger.text_recovery_pending(&occurrence));
    ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occ(0, 8_000)),
        "tail words",
    );
    assert!(ledger.text_recovery_pending(&occurrence));
    let repaired = ledger.admit(
        &obs(ObservationProducer::Whisper, 1, occurrence.clone()),
        "entire recovered utterance",
    );
    // D5 (ledger speech truth brief): absence is not evidence. A whole
    // label without word geometry cannot show that the held wording is
    // covered, so it preserves the held label (03beb77a).
    assert!(
        matches!(repaired, MutationReceipt::Preserve { .. }),
        "{repaired:?}"
    );
    assert!(
        ledger.text_recovery_pending(&occurrence),
        "an exact label is not returned decoder work"
    );
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, 7, &occurrence);
    let words = ledger.admit_word_slots(
        &observation,
        &forensic_recovery_word_pins("entire recovered utterance", Some((0, 16_000))),
    );
    assert!(words.grants_mutation() || matches!(words, MutationReceipt::Preserve { .. }));
    assert!(!ledger.text_recovery_pending(&occurrence));
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    let coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    assert_eq!(coverage.coverage_ratio(), Some(1.0));
    assert!(ledger.record_seal_coverage(coverage));
    ledger
        .seal(&occurrence)
        .expect("returned measured occurrence");
    assert!(ledger.seal_terminal("s1", 1).is_ok());
    assert!(!ledger.require_text_recovery(&occurrence));
    assert!(!ledger.require_text_recovery(&occ(32_000, 48_000)));
}

#[test]
fn text_recovery_debt_accepts_agreement_but_not_replayed_agreement() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, occurrence.clone()),
        "same words",
    );
    assert!(ledger.require_text_recovery(&occurrence));
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    assert!(matches!(
        ledger.admit(&whisper, "same words"),
        MutationReceipt::Preserve { .. }
    ));
    assert!(
        ledger.text_recovery_pending(&occurrence),
        "label agreement alone does not settle audio"
    );
    assert!(ledger.require_text_recovery(&occurrence));
    assert!(matches!(
        ledger.admit(&whisper, "same words"),
        MutationReceipt::Refuse { .. }
    ));
    assert!(ledger.text_recovery_pending(&occurrence));
    assert!(matches!(
        ledger.admit(
            &obs(ObservationProducer::Formatter, 1, occurrence.clone()),
            "polished words",
        ),
        MutationReceipt::Refuse {
            reason: RefuseReason::AuthorityConflict,
            ..
        }
    ));
    assert_eq!(ledger.text_of(&occurrence), Some("same words"));
    assert!(matches!(
        ledger.admit(
            &obs(ObservationProducer::Whisper, 2, occurrence.clone()),
            "same words"
        ),
        MutationReceipt::Preserve { .. }
    ));
    assert!(ledger.text_recovery_pending(&occurrence));
    ledger.admit(
        &obs(ObservationProducer::ManualHuman, 3, occurrence.clone()),
        "",
    );
    assert!(ledger.text_recovery_pending(&occurrence));
    ledger.admit(
        &obs(ObservationProducer::ManualHuman, 4, occurrence.clone()),
        "human words",
    );
    assert!(!ledger.text_recovery_pending(&occurrence));
    forensic_recovery_agreement_requires_fresh_word_scope_not_replay();
}

#[test]
fn text_recovery_debt_cannot_be_covered_by_an_overlapping_label() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    assert!(ledger.require_text_recovery(&occurrence));
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::Whisper, 0, occ(0, 32_000)),
                "words spanning both ranges",
            )
            .grants_mutation()
    );
    let speech = measured_speech(
        "s1",
        1,
        32_000,
        vec![TailSampleRange {
            session: "s1".to_string(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 32_000,
        }],
    );
    let coverage = ledger.assess_seal_coverage("s1", 1, &speech, 0);
    assert_eq!(coverage.coverage_ratio(), Some(0.5));
    assert_eq!(coverage.max_uncovered_samples, 16_000);
    assert_eq!(coverage.uncovered_speech_ranges[0].sample_start, 0);
    assert_eq!(coverage.uncovered_speech_ranges[0].sample_end, 16_000);
    assert!(ledger.text_recovery_pending(&occurrence));
}

#[test]
fn text_recovery_debt_requires_new_same_lane_generation() {
    let (mut ledger, occurrence, _) = forensic_recovery_measured_ledger();
    ledger.admit(
        &obs(ObservationProducer::Whisper, 4, occurrence.clone()),
        "heard words",
    );
    assert!(ledger.require_text_recovery(&occurrence));
    let stale = ObservationIdentity::new(ObservationProducer::Whisper, 8, 3, occurrence.clone());
    assert!(matches!(
        ledger.admit(&stale, "heard words"),
        MutationReceipt::Preserve { .. }
    ));
    assert!(ledger.text_recovery_pending(&occurrence));
    assert!(matches!(
        ledger.admit(
            &obs(ObservationProducer::Whisper, 5, occurrence.clone()),
            "heard words",
        ),
        MutationReceipt::Preserve { .. }
    ));
    assert!(
        ledger.text_recovery_pending(&occurrence),
        "new label generations still owe decoder scope"
    );
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, 7, &occurrence);
    assert!(observation.generation > 5);
    let returned = ledger.admit_word_slots(
        &observation,
        &forensic_recovery_word_pins("heard words", Some((0, 16_000))),
    );
    assert!(returned.grants_mutation() || matches!(returned, MutationReceipt::Preserve { .. }));
    assert!(!ledger.text_recovery_pending(&occurrence));
}

#[test]
fn empty_whisper_then_real_apple_preserves_returns_and_receipt_history() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let serial = ledger.serial_of(&occurrence).unwrap().clone();
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    assert_eq!(
        ledger.admit(&whisper, " "),
        MutationReceipt::Refuse {
            occurrence: occurrence.clone(),
            reason: RefuseReason::EmptyLabel,
        }
    );
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::LabelMissing));
    assert_eq!(
        ledger.seal_terminal("s1", 1),
        Err(SealRefusal::LabelMissing)
    );
    let prior = ledger.frontier_of(&occurrence).unwrap().clone();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(ledger.schedule_late_apple_label(&apple, "real words"));
    assert!(
        prior
            .returned
            .is_subset(&ledger.frontier_of(&occurrence).unwrap().returned)
    );
    assert_eq!(
        ledger.admit(&apple, "real words"),
        MutationReceipt::Insert {
            occurrence: occurrence.clone(),
        }
    );
    assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    let lexicon = obs(ObservationProducer::Lexicon, 0, occurrence.clone());
    assert!(matches!(
        ledger.admit(&lexicon, "real words"),
        MutationReceipt::Preserve { .. }
    ));
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
    let seal = ledger.seal(&occurrence).unwrap().clone();
    assert_eq!(seal.layer_trail_ordinals, vec![0, 1, 2]);
    assert_eq!(seal.serials, vec![serial]);
    assert_eq!(seal.coverage, occurrence);
    assert_eq!(ledger.text_of(&occurrence), Some("real words"));
    assert_eq!(ledger.len(), 1);
}

#[test]
fn duplicate_empty_returns_do_not_seal_or_erase_evidence() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    assert!(matches!(
        ledger.admit(&whisper, ""),
        MutationReceipt::Refuse {
            reason: RefuseReason::EmptyLabel,
            ..
        }
    ));
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    let frontier = ledger.frontier_of(&occurrence).unwrap().clone();
    assert!(matches!(
        ledger.admit(&whisper, ""),
        MutationReceipt::Refuse {
            reason: RefuseReason::BatchDuplicate,
            ..
        }
    ));
    assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    assert_eq!(ledger.frontier_of(&occurrence), Some(&frontier));
    assert_eq!(ledger.layer_trail().len(), 2);
    assert!(ledger.is_empty());
    assert_eq!(ledger.seal(&occurrence), Err(SealRefusal::LabelMissing));
}

#[test]
fn late_apple_requires_exact_qualification_and_never_relaunches_observers() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    let frontier = ledger.frontier_of(&occurrence).unwrap().clone();
    for foreign in [
        OccurrenceIdentity::new("other", 1, 0, 16_000),
        OccurrenceIdentity::new("s1", 2, 0, 16_000),
        occ(1, 16_000),
        occ(0, 16_001),
    ] {
        assert!(
            !ledger
                .schedule_late_apple_label(&obs(ObservationProducer::Apple, 0, foreign), "words")
        );
    }
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(!ledger.schedule_late_apple_label(&apple, " "));
    assert!(!ledger.schedule_late_apple_label(
        &obs(ObservationProducer::Whisper, 1, occurrence.clone()),
        "words"
    ));
    assert_eq!(ledger.frontier_of(&occurrence), Some(&frontier));
    assert!(ledger.schedule_late_apple_label(&apple, "words"));
    assert!(!ledger.schedule_late_apple_label(&apple, "words"));
    assert!(!ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));
    assert!(
        ledger
            .frontier_of(&occurrence)
            .unwrap()
            .returned
            .contains(&ObservationProducer::Whisper)
    );
}

#[test]
fn successful_label_seal_refuses_late_machine_but_preserves_human_provenance() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    assert!(ledger.admit(&whisper, "successful words").grants_mutation());
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    let seal = ledger.seal(&occurrence).unwrap().clone();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(!ledger.schedule_late_apple_label(&apple, "late words"));
    assert!(matches!(
        ledger.admit(&apple, "late words"),
        MutationReceipt::Refuse {
            reason: RefuseReason::SealedReplay,
            ..
        }
    ));
    assert_eq!(ledger.text_of(&occurrence), Some("successful words"));
    let human = obs(ObservationProducer::ManualHuman, 1, occurrence.clone());
    assert!(ledger.admit(&human, "human correction").grants_mutation());
    assert_eq!(ledger.manual_edits().len(), 1);
    assert_eq!(ledger.manual_edits()[0].supersedes_seal, seal.receipt_id);
    assert_eq!(ledger.seal(&occurrence), Ok(&seal));
    assert_eq!(ledger.post_seal_decisions(&occurrence).len(), 2);
}

/// Only concrete observer launches extend a frontier. A formatter setting
/// without a launched proposal job is absent, and a repeated return cannot
/// report a second close transition.
#[test]
fn frontier_contains_launched_observers_and_closes_once() {
    let occurrence = occ(0, 16_000);
    let mut ledger = AcousticLedger::new();
    ledger.schedule_frontier(
        occurrence.clone(),
        [ObservationProducer::Apple, ObservationProducer::Lexicon],
    );
    assert!(ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));

    let frontier = ledger.frontier_of(&occurrence).expect("frontier");
    assert_eq!(
        frontier.open_producers(),
        vec![
            ObservationProducer::Apple,
            ObservationProducer::Whisper,
            ObservationProducer::Lexicon,
        ]
    );
    assert!(
        !frontier
            .open_producers()
            .contains(&ObservationProducer::Formatter),
        "configured but unlaunched Formatter is not scheduled"
    );

    assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Lexicon));
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    assert!(
        !ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper),
        "repeated completion is not a second close transition"
    );
    assert!(
        !ledger.schedule_observer(occurrence, ObservationProducer::Formatter),
        "a closed frontier cannot be reopened by a late configured observer"
    );
}

#[test]
fn duplicate_schedule_observer_is_not_a_new_launch() {
    let occurrence = occ(0, 16_000);
    let mut ledger = AcousticLedger::new();
    ledger.schedule_frontier(
        occurrence.clone(),
        [ObservationProducer::Apple, ObservationProducer::Lexicon],
    );

    assert!(ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper));
    assert!(
        !ledger.schedule_observer(occurrence.clone(), ObservationProducer::Whisper),
        "a duplicate before return is not a concrete new launch"
    );
    assert!(
        !ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper),
        "Apple and Lexicon still keep the frontier open"
    );
    assert!(
        !ledger.schedule_observer(occurrence, ObservationProducer::Whisper),
        "a returned producer cannot be relaunched inside the same frontier"
    );
}

/// The conservation law itself, on the shape that motivated it: five
/// distinct acoustic occurrences of one name, byte-identical text.
#[test]
fn five_acoustic_occurrences_of_one_word_yield_five_inserts() {
    let mut ledger = AcousticLedger::new();
    let items: Vec<(ObservationIdentity, String)> = (0..5)
        .map(|i| {
            let start = i * 16_000;
            (
                obs(ObservationProducer::Whisper, i, occ(start, start + 8_000)),
                "Iwo".to_string(),
            )
        })
        .collect();
    let (receipts, tally) = ledger.admit_batch(&items);
    assert_eq!(receipts.iter().filter(|r| r.is_insert()).count(), 5);
    assert_eq!(tally.observations_in, tally.receipts_out);
    assert_eq!(tally.occurrences_held, 5);
}

/// A cumulative producer restating the same five occurrences must not grow
/// the count. Conservation is counted over occurrences, not hypotheses.
#[test]
fn restating_the_same_occurrences_does_not_grow_the_count() {
    let mut ledger = AcousticLedger::new();
    let first: Vec<(ObservationIdentity, String)> = (0..5)
        .map(|i| {
            let start = i * 16_000;
            (
                obs(ObservationProducer::Apple, i, occ(start, start + 8_000)),
                "Iwo".to_string(),
            )
        })
        .collect();
    ledger.admit_batch(&first);
    let (_, tally) = ledger.admit_batch(&first);
    assert_eq!(tally.occurrences_held, 5, "a replay is not a sixth event");
}

/// D1 — Apple hears "Ivo", Whisper hears "Iwo" on the SAME range. That is a
/// correction of one occurrence, not a replay to be refused: the higher
/// authority rewrites the text in place and the count is unchanged.
#[test]
fn d1_higher_authority_on_the_same_range_corrects_rather_than_refuses() {
    let mut ledger = AcousticLedger::new();
    let range = occ(0, 16_000);
    let apple = ledger.admit(&obs(ObservationProducer::Apple, 0, range.clone()), "Ivo");
    assert!(apple.is_insert());

    let whisper = ledger.admit(&obs(ObservationProducer::Whisper, 0, range.clone()), "Iwo");
    assert_eq!(
        whisper,
        MutationReceipt::Correct {
            occurrence: range.clone(),
            from: ObservationProducer::Apple,
            to: ObservationProducer::Whisper,
        }
    );
    assert_eq!(ledger.text_of(&range), Some("Iwo"));
    assert_eq!(ledger.len(), 1, "a correction may not mint a second event");
}

/// D1b — human evidence outranks every model prior, so `Iwo` is never
/// normalised back to `Ivo` by a lower layer.
#[test]
fn d1b_manual_evidence_is_not_overridden_by_a_model_prior() {
    let mut ledger = AcousticLedger::new();
    let range = occ(0, 16_000);
    ledger.admit(
        &obs(ObservationProducer::ManualHuman, 0, range.clone()),
        "Iwo",
    );
    // Human ownership is protected without inventing an acoustic seal.
    let downgrade = ledger.admit(&obs(ObservationProducer::Whisper, 9, range.clone()), "Ivo");
    assert_eq!(
        downgrade,
        MutationReceipt::Refuse {
            occurrence: range.clone(),
            reason: RefuseReason::AuthorityConflict,
        }
    );
    assert_eq!(ledger.text_of(&range), Some("Iwo"));
}

/// D2 — a range that shares audio with a committed one but carries no word
/// pins may not clip text. It stays visible and gets no mutation right.
#[test]
fn d2_overlap_without_word_pins_does_not_clip_text() {
    let mut ledger = AcousticLedger::new();
    ledger.admit(&obs(ObservationProducer::Apple, 0, occ(0, 16_000)), "Iwo");
    let overlapping = ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occ(8_000, 24_000)),
        "Iwo later",
    );
    assert_eq!(
        overlapping,
        MutationReceipt::KeepVisibleUnanchored {
            occurrence: occ(8_000, 24_000),
            label: "Iwo later".to_string(),
            reason: NoAuthorityReason::OverlapWithoutWordPins,
        },
        "clipping a range without pins invents a sub-range the payload does not carry"
    );
    assert!(!overlapping.grants_mutation());
    assert_eq!(ledger.len(), 1);
}

/// D3 — one zero-width prior must not poison the ledger. It never enters
/// the map, so a later well-formed disjoint span still inserts cleanly.
#[test]
fn d3_a_zero_width_prior_does_not_poison_the_ledger() {
    let mut ledger = AcousticLedger::new();
    let degenerate = ledger.admit(
        &obs(ObservationProducer::Apple, 0, occ(12_000, 12_000)),
        "hm",
    );
    assert_eq!(
        degenerate,
        MutationReceipt::KeepVisibleUnanchored {
            occurrence: occ(12_000, 12_000),
            label: "hm".to_string(),
            reason: NoAuthorityReason::ZeroWidth,
        }
    );
    assert!(ledger.is_empty(), "an unanchored prior holds no occurrence");

    let good = ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occ(32_000, 48_000)),
        "Iwo",
    );
    assert!(
        good.is_insert(),
        "a good disjoint span must not inherit the degenerate prior's verdict"
    );
}

/// D4 — capture epochs are partitioned. A new epoch restarts the sample
/// clock, so its ranges are not comparable to the old epoch's and must not
/// be refused wholesale.
#[test]
fn d4_a_new_capture_epoch_is_partitioned_not_refused() {
    let mut ledger = AcousticLedger::new();
    ledger.admit(
        &obs(
            ObservationProducer::Apple,
            0,
            OccurrenceIdentity::new("s1", 1, 0, 16_000),
        ),
        "Iwo",
    );
    let next_epoch = ledger.admit(
        &obs(
            ObservationProducer::Apple,
            0,
            OccurrenceIdentity::new("s1", 2, 0, 16_000),
        ),
        "Iwo",
    );
    assert!(
        next_epoch.is_insert(),
        "same sample numbers in a new epoch are a different moment in the world"
    );
    assert_eq!(ledger.len(), 2);

    let other_session = ledger.admit(
        &obs(
            ObservationProducer::Apple,
            0,
            OccurrenceIdentity::new("s2", 1, 0, 16_000),
        ),
        "Iwo",
    );
    assert!(other_session.is_insert());
    assert_eq!(ledger.len(), 3);
}

/// D5 — text with no anchor stays visible. It is not deleted and it is not
/// granted the right to delete.
#[test]
fn d5_unanchored_text_stays_visible_without_mutation_rights() {
    let mut ledger = AcousticLedger::new();
    ledger.admit(&obs(ObservationProducer::Whisper, 0, occ(0, 16_000)), "Iwo");
    let floating = ledger.admit(
        &obs(ObservationProducer::Apple, 0, occ(99_000, 99_000)),
        "coś jeszcze",
    );
    assert_eq!(
        floating,
        MutationReceipt::KeepVisibleUnanchored {
            occurrence: occ(99_000, 99_000),
            label: "coś jeszcze".to_string(),
            reason: NoAuthorityReason::ZeroWidth,
        }
    );
    assert!(!floating.grants_mutation());
    assert_eq!(
        ledger.text_of(&occ(0, 16_000)),
        Some("Iwo"),
        "unanchored text may not overwrite an anchored occurrence"
    );
}

/// D6 — the same observation twice in one batch is answered twice, but only
/// counted once. The receipt count still equals the observation count.
#[test]
fn d6_a_duplicate_observation_in_one_batch_is_answered_but_not_double_counted() {
    let mut ledger = AcousticLedger::new();
    let observation = obs(ObservationProducer::Whisper, 0, occ(0, 16_000));
    let items = vec![
        (observation.clone(), "Iwo".to_string()),
        (observation.clone(), "Iwo".to_string()),
    ];
    let (receipts, tally) = ledger.admit_batch(&items);
    assert_eq!(receipts.len(), 2, "one receipt per observation, always");
    assert!(receipts[0].is_insert());
    assert_eq!(
        receipts[1],
        MutationReceipt::Refuse {
            occurrence: occ(0, 16_000),
            reason: RefuseReason::BatchDuplicate,
        }
    );
    assert_eq!(tally.occurrences_held, 1);
    assert_eq!(tally.observations_in, tally.receipts_out);
}

/// D7 — utterance-grain evidence is all-or-nothing. The ledger never
/// answers with a sub-range the observation did not declare.
#[test]
fn d7_utterance_grain_is_never_clipped_into_an_invented_sub_range() {
    let mut ledger = AcousticLedger::new();
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, occ(0, 40_000)),
        "zdanie",
    );
    let straddling = ledger.admit(
        &obs(ObservationProducer::Whisper, 0, occ(20_000, 60_000)),
        "zdanie dalej",
    );
    match &straddling {
        MutationReceipt::KeepVisibleUnanchored {
            reason,
            occurrence,
            label,
        } => {
            assert_eq!(*reason, NoAuthorityReason::OverlapWithoutWordPins);
            assert_eq!(occurrence, &occ(20_000, 60_000));
            assert_eq!(label, "zdanie dalej");
        }
        other => panic!("utterance grain must not be clipped, got {other:?}"),
    }
    let held: Vec<&OccurrenceIdentity> = ledger.occurrences().collect();
    assert_eq!(held, vec![&occ(0, 40_000)], "no invented sub-range entered");
}

/// A covered overlap is refused by range. The later string is not consulted,
/// and the admitted word stays the only token.
#[test]
fn covered_overlap_is_replayed_range_identity_without_reading_the_string() {
    let mut ledger = AcousticLedger::new();
    let admitted = occ(24_000, 48_000);
    ledger.admit(
        &obs(ObservationProducer::Apple, 0, admitted.clone()),
        "beta",
    );
    let covered = occ(32_000, 48_000);
    assert_eq!(
        ledger.classify_overlap_pin(
            &covered,
            48_000,
            72_000,
            std::slice::from_ref(&admitted),
            false,
        ),
        OverlapPinClass::Replay
    );
    let different = ledger.refuse_replayed_range(
        &obs(ObservationProducer::Whisper, 2, covered.clone()),
        "powtorka",
    );
    assert_eq!(
        different,
        MutationReceipt::Refuse {
            occurrence: covered,
            reason: RefuseReason::ReplayedRangeIdentity,
        }
    );
    assert!(!different.grants_mutation());
    assert_eq!(ledger.text_of(&admitted), Some("beta"));
    assert_eq!(ledger.len(), 1);
    let tally = ledger.conservation();
    assert_eq!(tally.observations_in, tally.receipts_out);
    assert_eq!(tally.occurrences_held, 1);
}

#[test]
fn word_midpoint_owns_seam_without_apple_range_replay() {
    let mut ledger = AcousticLedger::new();
    let member = occ(0, 96_000);
    ledger.admit(&obs(ObservationProducer::Apple, 0, member.clone()), "apple");
    let seam_word = occ(44_000, 51_000);
    assert_eq!(
        ledger.classify_overlap_pin(&seam_word, 0, 48_000, std::slice::from_ref(&member), true,),
        OverlapPinClass::ExclusiveTail { member_index: 0 },
    );
    assert_eq!(
        ledger.classify_overlap_pin(
            &seam_word,
            48_000,
            96_000,
            std::slice::from_ref(&member),
            true,
        ),
        OverlapPinClass::ExclusiveTail { member_index: 0 },
    );
    let apple_held_word = occ(52_000, 60_000);
    assert_eq!(
        ledger.classify_overlap_pin(
            &apple_held_word,
            48_000,
            96_000,
            std::slice::from_ref(&member),
            true,
        ),
        OverlapPinClass::ExclusiveTail { member_index: 0 },
    );
}

#[test]
fn word_midpoint_owns_a_pin_straddling_either_member_edge() {
    let ledger = AcousticLedger::new();
    let member = occ(10_000, 30_000);
    for pin in [occ(8_000, 14_000), occ(26_000, 32_000)] {
        assert_eq!(
            ledger.classify_overlap_pin(&pin, 0, 40_000, std::slice::from_ref(&member), true),
            OverlapPinClass::ExclusiveTail { member_index: 0 },
        );
        assert_eq!(
            ledger.classify_overlap_pin(&pin, 0, 40_000, std::slice::from_ref(&member), false),
            OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins),
        );
    }
}

#[test]
fn word_midpoint_in_member_gap_remains_unanchored() {
    let ledger = AcousticLedger::new();
    assert_eq!(
        ledger.classify_overlap_pin(&occ(8_000, 12_000), 0, 40_000, &[occ(12_000, 30_000)], true),
        OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins),
    );
}

#[test]
fn adjacent_members_give_straddling_word_to_its_midpoint_owner() {
    let ledger = AcousticLedger::new();
    let members = [occ(0, 16_000), occ(16_000, 32_000)];
    assert_eq!(
        ledger.classify_overlap_pin(&occ(14_000, 22_000), 0, 32_000, &members, true),
        OverlapPinClass::ExclusiveTail { member_index: 1 },
    );
    assert_eq!(
        ledger.classify_overlap_pin(&occ(14_000, 22_000), 0, 32_000, &members, false),
        OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins),
    );
    assert_eq!(
        ledger.classify_overlap_pin(
            &occ(14_000, 22_000),
            0,
            32_000,
            &[occ(0, 20_000), occ(16_000, 32_000)],
            true
        ),
        OverlapPinClass::Unanchored(NoAuthorityReason::OverlapWithoutWordPins),
    );
}

#[test]
fn partial_whisper_coverage_replaces_only_heard_apple_slots() {
    let mut ledger = AcousticLedger::new();
    let owner = occ(0, 240_000);
    let apple = obs(ObservationProducer::Apple, 0, owner.clone());
    assert!(
        ledger
            .admit_pinned_label(
                &apple,
                "a b c d e",
                &[
                    crate::pipeline::acoustic_ledger::WordPin::new(0, 48_000, "a"),
                    crate::pipeline::acoustic_ledger::WordPin::new(48_000, 96_000, "b"),
                    crate::pipeline::acoustic_ledger::WordPin::new(96_000, 144_000, "c"),
                    crate::pipeline::acoustic_ledger::WordPin::new(144_000, 192_000, "d"),
                    crate::pipeline::acoustic_ledger::WordPin::new(192_000, 240_000, "e"),
                ]
            )
            .is_insert()
    );
    let whisper = obs(ObservationProducer::Whisper, 1, owner.clone());
    assert!(
        ledger
            .admit_word_slots(
                &whisper,
                &[
                    crate::pipeline::acoustic_ledger::WordPin::new(0, 48_000, "A")
                        .with_decode_window(0, 240_000),
                    crate::pipeline::acoustic_ledger::WordPin::new(48_000, 96_000, "B")
                        .with_decode_window(0, 240_000),
                    crate::pipeline::acoustic_ledger::WordPin::new(96_000, 144_000, "C")
                        .with_decode_window(0, 240_000),
                    crate::pipeline::acoustic_ledger::WordPin::new(144_000, 192_000, "D")
                        .with_decode_window(0, 240_000),
                ]
            )
            .grants_mutation()
    );
    assert_eq!(ledger.text_of(&owner), Some("A B C D e"));
    assert_eq!(
        ledger
            .layer_trail()
            .iter()
            .filter(|r| matches!(
                r.decision,
                MutationReceipt::Refuse {
                    reason: RefuseReason::ReplacedByWhisper,
                    ..
                }
            ) && r.observation == apple)
            .count(),
        4
    );
    assert_eq!(ledger.conservation().residue(), 0);
    ledger.assert_slot_labels();
    // D3 (ledger speech truth brief): a second engine on an owned range is
    // a witness, not silence. Whisper's four words land over Apple's on the
    // same ranges and agree with them, so they are Confirmed; Apple's `e`,
    // which no Whisper word reached, keeps no verdict.
    let witnesses = ledger
        .slots_of(&owner)
        .unwrap()
        .iter()
        .map(|slot| slot.witness.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        witnesses,
        [
            SlotWitness::Confirmed,
            SlotWitness::Confirmed,
            SlotWitness::Confirmed,
            SlotWitness::Confirmed,
            SlotWitness::Unwitnessed,
        ]
    );
}

/// Same-lane revision: Apple correcting its own final on its own range at a
/// newer generation is a correction, not a replay.
#[test]
fn same_producer_at_a_newer_generation_may_revise_its_own_span() {
    let mut ledger = AcousticLedger::new();
    let range = occ(0, 16_000);
    ledger.admit(&obs(ObservationProducer::Apple, 0, range.clone()), "szuty");
    let revised = ledger.admit(&obs(ObservationProducer::Apple, 1, range.clone()), "skróty");
    assert!(revised.is_correct());
    assert_eq!(ledger.text_of(&range), Some("skróty"));
    assert_eq!(ledger.len(), 1);
}

/// Arriving later does not make a hypothesis a new physical event: `order`
/// lives on the observation, not on the occurrence.
#[test]
fn order_lives_on_the_observation_not_on_the_occurrence() {
    let early = obs(ObservationProducer::Whisper, 0, occ(0, 16_000));
    let late = obs(ObservationProducer::Whisper, 42, occ(0, 16_000));
    assert_eq!(
        early.occurrence, late.occurrence,
        "generation must not be part of physical identity"
    );
    assert_ne!(
        early, late,
        "generation must be part of observation identity"
    );
}

/// A cumulative final whose window is disjoint from every committed
/// occurrence is new audio; the text matcher is not consulted.
#[test]
fn a_disjoint_cumulative_window_is_wholly_novel() {
    let committed = vec![(occ(0, 16_000), 4)];
    let decision = CumulativeFinalAdmission::decide(&occ(32_000, 48_000), &committed);
    assert_eq!(decision, CumulativeFinalAdmission::WhollyNovel);
    assert_eq!(
        decision.clamp_known_prefix(5),
        0,
        "no committed occurrence overlaps, so nothing is already known"
    );
}

/// The clamp is the conservation guard: a restatement may not claim more
/// occurrences than the overlapped canvas actually holds.
#[test]
fn alignment_may_not_claim_more_words_than_the_canvas_holds() {
    let committed = vec![(occ(0, 64_000), 4)];
    let decision = CumulativeFinalAdmission::decide(&occ(48_000, 80_000), &committed);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::AlignInside {
            canvas_words_under_authority: 4,
        }
    );
    assert_eq!(
        decision.clamp_known_prefix(5),
        4,
        "the fifth occurrence has no committed counterpart and must survive"
    );
    assert_eq!(
        decision.clamp_known_prefix(3),
        3,
        "shorter matches pass through"
    );
}

/// With nothing anchored to compare against, the legacy text lane keeps its
/// behaviour — it is demoted, not replaced by a fabricated verdict.
#[test]
fn without_an_anchor_the_text_matcher_keeps_its_legacy_answer() {
    let decision = CumulativeFinalAdmission::decide(&occ(0, 16_000), &[]);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::NoAnchor {
            reason: NoAuthorityReason::NoRange,
        }
    );
    assert_eq!(decision.clamp_known_prefix(5), 5);
}

/// A zero-width window carries no authority in either direction.
#[test]
fn a_zero_width_cumulative_window_has_no_authority() {
    let committed = vec![(occ(0, 16_000), 4)];
    let decision = CumulativeFinalAdmission::decide(&occ(16_000, 16_000), &committed);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::NoAnchor {
            reason: NoAuthorityReason::ZeroWidth,
        }
    );
}

/// Unanchored committed spans are absent evidence, never objections.
#[test]
fn unanchored_committed_spans_do_not_veto_a_window() {
    let committed = vec![(occ(5_000, 5_000), 3), (occ(32_000, 48_000), 2)];
    let decision = CumulativeFinalAdmission::decide(&occ(40_000, 56_000), &committed);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::AlignInside {
            canvas_words_under_authority: 2,
        },
        "only the anchored overlap contributes authority"
    );
}

/// A cumulative producer under-declares its window, so the finality bar —
/// not the window — bounds how much it may claim to restate.
#[test]
fn a_cumulative_restatement_is_bounded_by_the_open_occurrences() {
    let open = vec![
        (occ(0, 16_000), 1),
        (occ(16_000, 32_000), 1),
        (occ(32_000, 48_000), 1),
        (occ(48_000, 64_000), 1),
    ];
    let decision = CumulativeFinalAdmission::for_cumulative_restatement(&open);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::AlignInside {
            canvas_words_under_authority: 4,
        }
    );
    assert_eq!(
        decision.clamp_known_prefix(5),
        4,
        "four open occurrences cannot account for a fifth token"
    );
}

/// With nothing open, a cumulative final restates nothing and the legacy
/// text lane is left exactly as it was.
#[test]
fn a_cumulative_restatement_with_nothing_open_has_no_anchor() {
    let decision = CumulativeFinalAdmission::for_cumulative_restatement(&[]);
    assert_eq!(
        decision,
        CumulativeFinalAdmission::NoAnchor {
            reason: NoAuthorityReason::NoRange,
        }
    );
    assert_eq!(decision.clamp_known_prefix(3), 3);
}

/// The authorised slice is what keeps a match in an unrelated part of the
/// transcript out of the matcher's reach.
#[test]
fn the_authorized_canvas_hides_text_outside_the_authorized_span() {
    let canvas = vec![
        "zupelnie", "co", "innego", "iwo", "iwo", "iwo", "iwo", "iwo", "dalszy", "ciag",
    ];
    let decision = CumulativeFinalAdmission::AlignInside {
        canvas_words_under_authority: 2,
    };
    assert_eq!(decision.authorized_canvas(&canvas), &["dalszy", "ciag"]);

    assert!(
        CumulativeFinalAdmission::WhollyNovel
            .authorized_canvas(&canvas)
            .is_empty(),
        "new audio has no canvas to align against"
    );
    assert_eq!(
        CumulativeFinalAdmission::NoAnchor {
            reason: NoAuthorityReason::NoRange
        }
        .authorized_canvas(&canvas),
        canvas.as_slice(),
        "with no anchor the legacy matcher keeps the whole canvas"
    );
}

/// A `TailSampleRange` and the occurrence built from it are the same key.
#[test]
fn tail_sample_range_maps_onto_occurrence_identity() {
    let range = TailSampleRange {
        session: "s1".to_string(),
        capture_epoch: 1,
        sample_start: 16_000,
        sample_end: 32_000,
    };
    assert_eq!(OccurrenceIdentity::from(&range), occ(16_000, 32_000));
}

/// Recorded W5-A fixture from session b2b3b95e. The replay harness cannot
/// inject historical Apple callbacks, so this pins their exact admitted
/// occurrence receipts and proves gap recovery at the ledger boundary.
#[test]
fn recorded_b2b3b95e_gaps_refuse_terminal_truth_until_pcm_occurrences_land() {
    const SESSION: &str = "b2b3b95e-4ddc-4845-a5ce-149b21eec166";
    const EPOCH: u64 = 1;
    const RATE: u64 = 88_200;
    const THRESHOLD: u64 = RATE * 250 / 1_000;

    fn admit_receipted(
        ledger: &mut AcousticLedger,
        range: OccurrenceIdentity,
        producer: ObservationProducer,
        request: u64,
        label: &str,
    ) {
        let calibration = EnergyCalibration::new("recorded-calibration", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: range.clone(),
            duration_ms: range.sample_len() as f64 * 1_000.0 / RATE as f64,
            energy_integral: range.sample_len() as f64,
            mean_rms_dbfs: -35.0,
            peak_dbfs: -19.0,
            vad_open_sample: Some(range.sample_start),
            vad_close_sample: Some(range.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(range.clone(), [producer]);
        let observation = ObservationIdentity::new(producer, request, 0, range.clone());
        assert!(ledger.admit(&observation, label).grants_mutation());
        assert!(ledger.note_frontier_return(&range, producer));
        ledger.seal(&range).expect("recorded occurrence seals");
    }

    let mut ledger = AcousticLedger::new();
    admit_receipted(
        &mut ledger,
        OccurrenceIdentity::new(SESSION, EPOCH, 304_819, 869_376),
        ObservationProducer::Apple,
        1,
        "Kurde powiem ci że dość fajne mechanizmy nam",
    );
    admit_receipted(
        &mut ledger,
        OccurrenceIdentity::new(SESSION, EPOCH, 1_196_697, 1_558_528),
        ObservationProducer::Apple,
        2,
        "Dość śmiesznym i ciekawym linie",
    );
    let speech = measured_speech(
        SESSION,
        EPOCH,
        1_602_560,
        vec![TailSampleRange {
            session: SESSION.to_string(),
            capture_epoch: EPOCH,
            sample_start: 304_819,
            sample_end: 1_602_560,
        }],
    );

    let incomplete = ledger.assess_seal_coverage(SESSION, EPOCH, &speech, THRESHOLD);
    assert_eq!(incomplete.status, SealCoverageStatus::Incomplete);
    assert_eq!(incomplete.speech_samples, 1_297_741);
    assert_eq!(incomplete.covered_samples, 926_388);
    assert_eq!(incomplete.max_uncovered_samples, 327_321);
    assert_eq!(
        incomplete
            .uncovered_speech_ranges
            .iter()
            .map(|range| (range.sample_start, range.sample_end))
            .collect::<Vec<_>>(),
        vec![(869_376, 1_196_697), (1_558_528, 1_602_560)]
    );
    assert!(ledger.record_seal_coverage(incomplete.clone()));
    assert_eq!(
        ledger.seal_terminal(SESSION, EPOCH),
        Err(SealRefusal::CoverageIncomplete),
        "incomplete measured speech must not become terminal truth"
    );

    admit_receipted(
        &mut ledger,
        OccurrenceIdentity::new(SESSION, EPOCH, 869_376, 1_196_697),
        ObservationProducer::Whisper,
        3,
        "wychodzą spontanicznie w tym",
    );
    admit_receipted(
        &mut ledger,
        OccurrenceIdentity::new(SESSION, EPOCH, 1_558_528, 1_602_560),
        ObservationProducer::Whisper,
        4,
        "pipeline'ie. Nie sądzisz?",
    );

    let complete = ledger.assess_seal_coverage(SESSION, EPOCH, &speech, THRESHOLD);
    assert_eq!(complete.status, SealCoverageStatus::Complete);
    assert_eq!(complete.covered_samples, complete.speech_samples);
    assert!(complete.uncovered_speech_ranges.is_empty());
    let rendered = ledger.rendered_text();
    assert!(rendered.contains("wychodzą spontanicznie w tym"));
    assert!(rendered.ends_with("Nie sądzisz?"));
    assert_eq!(complete.coverage_ratio(), Some(1.0));
    assert_eq!(complete.observed_samples, Some(1_602_560));
    assert!(ledger.record_seal_coverage(complete));
    ledger
        .seal_terminal(SESSION, EPOCH)
        .expect("complete recorded coverage may become terminal truth");
}

/// Evidence the two acoustic observers mint. Tests use it directly so the
/// ledger's own adjudication is exercised, not a bare range vector.
fn measured_speech(
    session: &str,
    capture_epoch: u64,
    observed_samples: u64,
    ranges: Vec<TailSampleRange>,
) -> AcousticSpeechEvidence {
    AcousticSpeechEvidence::measured(
        CaptureEvidenceIdentity::new(session, capture_epoch),
        crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
        AcousticAvailability::Observed { observed_samples },
        ranges,
    )
}

/// Measured silence is a verdict. Absence of measurement is not, and the
/// two must not collapse into the same trivially complete receipt.
#[test]
fn measured_silence_seals_and_absent_measurement_refuses() {
    let ledger = AcousticLedger::new();
    let silence = ledger.assess_seal_coverage(
        "quiet",
        1,
        &measured_speech("quiet", 1, 16_000, Vec::new()),
        4_000,
    );
    assert_eq!(silence.status, SealCoverageStatus::Complete);
    assert_eq!(silence.speech_samples, 0);
    assert_eq!(silence.coverage_ratio(), Some(1.0));
    assert_eq!(silence.observed_samples, Some(16_000));

    for availability in [
        AcousticAvailability::NotObserved,
        AcousticAvailability::InvalidMeasurement { valid_samples: 0 },
        AcousticAvailability::InvalidMeasurement {
            valid_samples: 8_000,
        },
        AcousticAvailability::Discontinuous {
            observed_samples: 8_000,
        },
    ] {
        let receipt = ledger.assess_seal_coverage(
            "quiet",
            1,
            &AcousticSpeechEvidence::unavailable(
                CaptureEvidenceIdentity::new("quiet", 1),
                "test_observer",
                availability,
            ),
            4_000,
        );
        assert!(
            receipt.status.unavailable_reason().is_some(),
            "{availability:?} cannot certify a take"
        );
        assert!(!receipt.status.is_complete());
        assert_eq!(
            receipt.coverage_ratio(),
            None,
            "an unknown extent has no ratio; 1.0 would read as a perfect take"
        );
        assert_eq!(receipt.observed_samples, None);
        assert_eq!(receipt.status.as_str(), "unavailable");
    }
}

/// A discontinuous observer names `PartialObservation`, and so does an
/// observer whose measured extent stops short of committed speech.
#[test]
fn unobserved_tails_and_foreign_evidence_refuse_terminal_truth() {
    let mut ledger = AcousticLedger::new();
    let discontinuous = ledger.assess_seal_coverage(
        "partial",
        1,
        &AcousticSpeechEvidence::unavailable(
            CaptureEvidenceIdentity::new("partial", 1),
            "test_observer",
            AcousticAvailability::Discontinuous {
                observed_samples: 8_000,
            },
        ),
        4_000,
    );
    assert_eq!(
        discontinuous.status,
        SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
    );

    // A speech span past the observed extent is an unobserved tail.
    let beyond = ledger.assess_seal_coverage(
        "partial",
        1,
        &measured_speech(
            "partial",
            1,
            8_000,
            vec![TailSampleRange {
                session: "partial".into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 16_000,
            }],
        ),
        4_000,
    );
    assert_eq!(
        beyond.status,
        SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
    );

    // Evidence bound to another take cannot be authenticated by asking
    // about this one, and a foreign range inside otherwise-valid evidence
    // survives adjudication instead of being filtered into silence.
    let foreign_owner = ledger.assess_seal_coverage(
        "partial",
        1,
        &measured_speech("successor", 1, 16_000, Vec::new()),
        4_000,
    );
    assert_eq!(
        foreign_owner.status,
        SealCoverageStatus::Unavailable(AcousticEvidenceGap::IdentityMismatch)
    );
    let foreign_range = ledger.assess_seal_coverage(
        "partial",
        1,
        &measured_speech(
            "partial",
            1,
            16_000,
            vec![TailSampleRange {
                session: "successor".into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 16_000,
            }],
        ),
        4_000,
    );
    assert_eq!(
        foreign_range.status,
        SealCoverageStatus::Unavailable(AcousticEvidenceGap::IdentityMismatch)
    );

    assert!(ledger.record_seal_coverage(discontinuous));
    assert_eq!(
        ledger.seal_terminal("partial", 1),
        Err(SealRefusal::CoverageIncomplete),
        "a non-complete verdict blocks the terminal seal whatever its reason"
    );
}

/// A sealed occurrence, its exact committed label, and the left context the
/// casing saw. That is the whole claim an incremental shaping makes — and
/// the receipt states each part explicitly instead of implying it.
fn sealed_for_shaping() -> (AcousticLedger, OccurrenceIdentity) {
    let occurrence = occ(0, 16_000);
    let mut ledger = AcousticLedger::new();
    let calibration = EnergyCalibration::new("incremental-shaping", 1.0, 1);
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
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
    let observation = obs(ObservationProducer::Apple, 0, occurrence.clone());
    assert!(ledger.admit(&observation, "jakieś słowa").grants_mutation());
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    ledger.seal(&occurrence).expect("closed occurrence seals");
    (ledger, occurrence)
}

#[test]
fn an_incremental_shaping_binds_one_sealed_occurrence_and_its_seal() {
    let (mut ledger, occurrence) = sealed_for_shaping();
    let seal_receipt = ledger
        .seal_of(&occurrence)
        .expect("sealed")
        .receipt_id
        .clone();

    let receipt = ledger
        .record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "Poprzednie zdanie.",
            shaped_text: "Jakieś słowa",
            sentence_break_before: false,
        })
        .expect("a sealed, committed, unchanged label shapes");

    assert_eq!(receipt.provenance, "light-plus");
    assert!(receipt.receipt_id.starts_with("light-plus-incremental-"));
    assert_eq!(receipt.occurrence, occurrence);
    assert_eq!(
        receipt.source_seal_receipt.as_deref(),
        Some(seal_receipt.as_str())
    );
    assert_eq!(receipt.source_label, "jakieś słowa");
    assert_eq!(receipt.shaped_text, "Jakieś słowa");
    assert_eq!(receipt.source_revision, 4);
    assert_eq!(receipt.revision, 5);
    assert_eq!(
        receipt.left_context_sha256,
        hex::encode(Sha256::digest("Poprzednie zdanie.".as_bytes())),
        "the left neighbourhood is pinned so the shape can be reproduced"
    );
    assert_eq!(ledger.incremental_shapings().len(), 1);
    assert_eq!(ledger.incremental_shapings()[0], receipt);

    // The acoustic label itself is untouched — presentation, not words.
    assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
    assert!(ledger.manual_document_revisions().is_empty());
    assert!(ledger.manual_edits().is_empty());
}

#[test]
fn an_identical_incremental_shaping_is_refused_instead_of_restated() {
    let (mut ledger, occurrence) = sealed_for_shaping();
    assert!(
        ledger
            .record_incremental_shaping(IncrementalShapingInput {
                session_id: "s1",
                source_revision: 4,
                revision: 5,
                occurrence: &occurrence,
                source_label: "jakieś słowa",
                left_context: "",
                shaped_text: "Jakieś słowa",
                sentence_break_before: false,
            })
            .is_ok()
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 5,
            revision: 6,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "Jakieś słowa",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_unchanged"),
        "a replayed seal mints no second receipt"
    );
    assert_eq!(ledger.incremental_shapings().len(), 1);
}

#[test]
fn incremental_shaping_refuses_unsealed_foreign_stale_and_empty_sources() {
    let (mut ledger, occurrence) = sealed_for_shaping();

    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "X.",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_session_missing")
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "   \n ",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_text_empty"),
        "an empty shape must never be allowed to erase words"
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 9,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "Jakieś słowa",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_revision_nonconsecutive")
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "obca-sesja",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "Jakieś słowa",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_session_mismatch")
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "zupełnie inne słowa",
            left_context: "",
            shaped_text: "Zupełnie inne słowa",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_source_label_stale"),
        "a shape must be derived from the label the ledger actually holds"
    );

    let unsealed = occ(16_000, 32_000);
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &unsealed,
            source_label: "cokolwiek",
            left_context: "",
            shaped_text: "Cokolwiek.",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_occurrence_not_committed"),
        "an open occurrence is never a sealed source"
    );
    assert!(ledger.incremental_shapings().is_empty());
}

/// The grouped input is a container, never a permission. Restating one
/// honest claim with exactly one fact replaced must still meet the
/// ledger's own checks, in the ledger's own order — and a refused claim
/// must leave the ledger able to accept the untouched original afterwards.
#[test]
fn incremental_shaping_refuses_a_grouped_foreign_session_and_a_grouped_stale_label() {
    let (mut ledger, occurrence) = sealed_for_shaping();
    let honest = IncrementalShapingInput {
        session_id: "s1",
        source_revision: 11,
        revision: 12,
        occurrence: &occurrence,
        source_label: "jakieś słowa",
        left_context: "Zdanie wcześniej.",
        shaped_text: "Jakieś słowa",
        sentence_break_before: false,
    };

    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "obca-sesja",
            ..honest
        }),
        Err("incremental_shaping_session_mismatch"),
        "one field of the group may not carry a foreign session past the check"
    );
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            source_label: "zupełnie inne słowa",
            shaped_text: "Zupełnie inne słowa",
            ..honest
        }),
        Err("incremental_shaping_source_label_stale"),
        "a shape grouped with a label the ledger does not hold is still stale"
    );
    assert!(
        ledger.incremental_shapings().is_empty(),
        "a refused grouped input mints nothing"
    );

    // Those were verdicts on the claim, not damage to the ledger: the
    // untouched original must still be admissible after both refusals.
    assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
    let receipt = ledger
        .record_incremental_shaping(honest)
        .expect("the untouched claim is still admissible after two refusals");
    assert_eq!(receipt.source_revision, 11);
    assert_eq!(receipt.revision, 12);
    assert_eq!(ledger.incremental_shapings().len(), 1);
}

/// Grouping the facts must not move a single byte of the receipt. The
/// identifier encodes provenance, session, sample geometry, both revisions
/// and the ordinal, so asserting it literally catches any silent reshuffle
/// of the facts on their way into the ledger.
#[test]
fn incremental_shaping_from_a_grouped_input_mints_the_same_receipt_facts() {
    let (mut ledger, occurrence) = sealed_for_shaping();
    let left_context = "Zdanie wcześniej.";
    let shaped = crate::pipeline::light_plus::apply_live_span(left_context, "jakieś słowa", false);
    assert_eq!(
        shaped, "Jakieś słowa",
        "the deterministic shaper still owns the bytes the ledger re-derives"
    );
    let seal_receipt = ledger
        .seal_of(&occurrence)
        .expect("sealed")
        .receipt_id
        .clone();

    let receipt = ledger
        .record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context,
            shaped_text: &shaped,
            sentence_break_before: false,
        })
        .expect("a grouped, sealed, deterministic claim shapes");

    assert_eq!(
        receipt.receipt_id, "light-plus-incremental-s1-0-16000-4-5-0",
        "provenance, session, sample span, both revisions and the ordinal              keep their exact places in the identifier"
    );
    assert_eq!(receipt.provenance, "light-plus");
    assert_eq!(receipt.session_id, "s1");
    assert_eq!(receipt.source_revision, 4);
    assert_eq!(receipt.revision, 5);
    assert_eq!(receipt.occurrence, occurrence);
    assert_eq!(
        receipt.source_seal_receipt.as_deref(),
        Some(seal_receipt.as_str())
    );
    assert_eq!(receipt.source_label, "jakieś słowa");
    assert_eq!(receipt.left_context, left_context);
    assert_eq!(
        receipt.left_context_sha256,
        hex::encode(Sha256::digest(left_context.as_bytes()))
    );
    assert_eq!(receipt.shaped_text, "Jakieś słowa");

    // The acoustic label stays untouched: this is presentation, not words.
    assert_eq!(ledger.text_of(&occurrence), Some("jakieś słowa"));
    assert_eq!(ledger.incremental_shapings().len(), 1);
    assert_eq!(ledger.incremental_shapings()[0], receipt);
}

#[test]
fn occurrence_and_single_terminal_have_distinct_authenticated_scope() {
    let (mut ledger, occurrence) = sealed_for_shaping();
    let ordinary = ledger.seal_of(&occurrence).unwrap().clone();
    let terminal = ledger.seal_terminal("s1", 1).unwrap();
    assert!(ordinary.is_occurrence_seal());
    assert!(!terminal.is_occurrence_seal());
    assert_eq!(terminal.sealed_occurrences, vec![occurrence]);
    assert_ne!(ordinary.receipt_id, terminal.receipt_id);
    assert!(ledger.authenticates_seal(&ordinary));
    assert!(ledger.authenticates_seal(&terminal));
    assert_eq!(ledger.seal_terminal("s1", 1).unwrap(), terminal);
    assert_eq!(ledger.terminal_seals.len(), 1);
    let mut forged = ordinary;
    forged.scope = LedgerSealScope::Terminal;
    assert!(!ledger.authenticates_seal(&forged));
}

#[test]
fn qualified_committed_open_source_shapes_without_claiming_a_seal() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let observation = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    assert!(ledger.admit(&observation, "jakieś słowa").grants_mutation());
    assert!(ledger.is_qualified(&occurrence));
    assert!(ledger.text_of(&occurrence).is_some());
    assert!(!ledger.is_sealed(&occurrence));
    let open_shape = ledger
        .record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "Jakieś słowa",
            sentence_break_before: false,
        })
        .expect("an admitted open label is presentable");
    assert!(open_shape.source_seal_receipt.is_none());
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    ledger.seal(&occurrence).unwrap();
    assert_eq!(
        ledger.record_incremental_shaping(IncrementalShapingInput {
            session_id: "s1",
            source_revision: 4,
            revision: 5,
            occurrence: &occurrence,
            source_label: "jakieś słowa",
            left_context: "",
            shaped_text: "Unrelated bytes.",
            sentence_break_before: false,
        }),
        Err("incremental_shaping_not_deterministic")
    );
    assert_eq!(ledger.incremental_shapings().len(), 1);
}

#[test]
fn clock_lie_span_keeps_its_text_and_cannot_replace_a_neighbour() {
    use crate::quality::supervisor::{
        QualityIssueKind, TakeQualityEvidence, classify_take_findings,
    };

    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(16_000);
    let lie = occ(0, 1_600);
    let lie_text = "x".repeat(41);
    let admitted = ledger.admit(&obs(ObservationProducer::Apple, 0, lie.clone()), &lie_text);
    assert!(admitted.is_insert(), "clock-lie keeps the span's own text");
    assert_eq!(ledger.text_of(&lie), Some(lie_text.as_str()));
    assert!(ledger.is_clock_lie_span(&lie));
    assert!(ledger.latest_receipt_names_clock_lie());
    assert_eq!(ledger.conservation().receipts_out, 1);
    assert_eq!(ledger.clock_lie_count(), 1);

    let neighbour = occ(1_600, 16_000);
    let neighbour_text = "good neighbour";
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::Apple, 0, neighbour.clone()),
                neighbour_text
            )
            .is_insert()
    );
    let neighbour_word = WordSlot {
        sample_start: neighbour.sample_start,
        sample_end: neighbour.sample_end,
        text: neighbour_text.to_string(),
        producer: ObservationProducer::Apple,
        observation: obs(ObservationProducer::Apple, 0, neighbour.clone()),
        witness: SlotWitness::Unwitnessed,
        confidence: None,
        surface_rewritten: false,
    };
    assert_eq!(
        ledger.replace_neighbour(&lie, std::slice::from_ref(&neighbour_word)),
        Some(lie.clone()),
        "the flagged span cannot authorize replacing its neighbour"
    );
    assert_eq!(
        ledger.replace_neighbour(&lie, &[]),
        None,
        "words over audio that holds no word are always insertable"
    );
    assert_eq!(ledger.text_of(&neighbour), Some(neighbour_text));
    assert_eq!(ledger.clock_lie_count(), 1);
    assert_eq!(ledger.conservation().residue(), 0);

    let mut evidence = TakeQualityEvidence::default();
    evidence.observe_clock_lie_count(ledger.clock_lie_count());
    evidence.daily_text = lie_text;
    let report = classify_take_findings(&evidence);
    assert_eq!(evidence.clock_lie_count, 1);
    assert!(
        report
            .findings
            .iter()
            .any(|row| row.kind == QualityIssueKind::ClockLie)
    );
}

#[test]
fn cloud_live_authority_rank_matches_enum_order() {
    let ordered = [
        ObservationProducer::Apple,
        ObservationProducer::CloudLive,
        ObservationProducer::Whisper,
        ObservationProducer::Lexicon,
        ObservationProducer::Formatter,
        ObservationProducer::ManualHuman,
    ];
    for pair in ordered.windows(2) {
        assert!(pair[0] < pair[1]);
        assert!(pair[0].authority_rank() < pair[1].authority_rank());
    }
    assert_eq!(ordered[1].as_str(), "cloud_live");
    assert_eq!(ordered[1].layer_label(), "cloud_live");
    assert_eq!(
        NoAuthorityReason::LateCloudLiveWordSealedOwner.as_str(),
        "late_cloud_live_word_sealed_owner"
    );
}

fn heard_slot(
    ledger: &mut AcousticLedger,
    producer: ObservationProducer,
    occurrence: &OccurrenceIdentity,
    start: u64,
    end: u64,
    text: &str,
) -> MutationReceipt {
    let observation = obs(
        producer,
        start
            .saturating_add(end)
            .saturating_add(text.len() as u64)
            .saturating_add(u64::from(producer.authority_rank())),
        occurrence.clone(),
    );
    ledger.admit_word_slots(
        &observation,
        &[
            crate::pipeline::acoustic_ledger::WordPin::new(start, end, text.to_string())
                .with_decode_window(occurrence.sample_start, occurrence.sample_end),
        ],
    )
}

#[test]
fn cloud_live_replaces_apple_and_yields_to_whisper() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    assert!(
        heard_slot(
            &mut ledger,
            ObservationProducer::Apple,
            &occurrence,
            1_000,
            5_000,
            "apple"
        )
        .is_insert()
    );
    assert!(
        heard_slot(
            &mut ledger,
            ObservationProducer::CloudLive,
            &occurrence,
            1_000,
            5_000,
            "cloud"
        )
        .grants_mutation()
    );
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].producer, ObservationProducer::CloudLive);
    assert_eq!(slots[0].text, "cloud");
    assert!(ledger.layer_trail().iter().any(|entry| {
        matches!(
            entry.decision,
            MutationReceipt::Refuse {
                reason: RefuseReason::ReplacedByCloudLive,
                ..
            }
        ) && entry.observation.producer == ObservationProducer::Apple
    }));

    assert!(
        heard_slot(
            &mut ledger,
            ObservationProducer::Apple,
            &occurrence,
            1_000,
            5_000,
            "again"
        )
        .grants_mutation()
            || !ledger.slots_of(&occurrence).unwrap().iter().any(|slot| {
                slot.producer == ObservationProducer::Apple && slot.text == "again"
            })
    );
    assert!(
        !ledger
            .slots_of(&occurrence)
            .unwrap()
            .iter()
            .any(|slot| slot.producer == ObservationProducer::Apple)
    );

    // Producer rank alone cannot authorize this edge-of-window change.
    assert!(
        !heard_slot(
            &mut ledger,
            ObservationProducer::Whisper,
            &occurrence,
            1_000,
            5_000,
            "whisper"
        )
        .grants_mutation()
    );
    assert_eq!(
        ledger.slots_of(&occurrence).unwrap()[0].producer,
        ObservationProducer::CloudLive
    );
    let contextual_whisper =
        ledger.next_word_observation(ObservationProducer::Whisper, 10_000, &occurrence);
    assert!(
        ledger
            .admit_word_slots(
                &contextual_whisper,
                &[WordPin::new(1_000, 5_000, "whisper").with_decode_window(0, 6_000)],
            )
            .grants_mutation()
    );
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].producer, ObservationProducer::Whisper);

    let before = ledger.slots_of(&occurrence).unwrap().to_vec();
    let _ = heard_slot(
        &mut ledger,
        ObservationProducer::CloudLive,
        &occurrence,
        1_000,
        5_000,
        "late-cloud",
    );
    assert_eq!(ledger.slots_of(&occurrence).unwrap(), &before);
}

#[test]
fn cloud_live_word_final_keeps_one_slot_per_word() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let observation = obs(ObservationProducer::CloudLive, 1, occurrence.clone());
    assert!(
        ledger
            .admit_word_slots(
                &observation,
                &[
                    crate::pipeline::acoustic_ledger::WordPin::new(1_000, 4_000, "dwa"),
                    crate::pipeline::acoustic_ledger::WordPin::new(8_000, 12_000, "slowa")
                ],
            )
            .is_insert()
    );
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots.len(), 2);
    assert_eq!(
        (
            slots[0].sample_start,
            slots[0].sample_end,
            slots[0].text.as_str()
        ),
        (1_000, 4_000, "dwa")
    );
    assert_eq!(
        (
            slots[1].sample_start,
            slots[1].sample_end,
            slots[1].text.as_str()
        ),
        (8_000, 12_000, "slowa")
    );
    assert!(
        slots
            .iter()
            .all(|slot| slot.sample_end - slot.sample_start < occurrence.sample_len())
    );
}

/// A6: pin confidence and the lexicon-rewrite mark travel from admission
/// through the slot into the signed composition receipts (d5).
#[test]
fn word_confidence_travels_from_pin_to_composed_receipt() {
    use crate::pipeline::word_confidence::{WordConfidence, WordConfidenceSource};
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let observation = obs(ObservationProducer::CloudLive, 1, occurrence.clone());
    let confidence = WordConfidence::new(WordConfidenceSource::VendorWordProbability, 0.42, 1);
    assert!(
        ledger
            .admit_word_slots(
                &observation,
                &[
                    crate::pipeline::acoustic_ledger::WordPin::new(1_000, 4_000, "dwa")
                        .with_confidence(confidence)
                        .surface_rewritten(),
                    crate::pipeline::acoustic_ledger::WordPin::new(8_000, 12_000, "slowa"),
                ],
            )
            .is_insert()
    );
    let slots = ledger.slots_of(&occurrence).unwrap();
    assert_eq!(slots[0].confidence, Some(confidence));
    assert!(slots[0].surface_rewritten);
    assert_eq!(slots[1].confidence, None);
    assert!(!slots[1].surface_rewritten);

    let composed = ledger.compose(&occurrence).expect("qualified words");
    assert_eq!(composed.tokens.len(), 2);
    assert_eq!(composed.tokens[0].confidence, Some(confidence));
    assert!(composed.tokens[0].surface_rewritten);
    assert_eq!(composed.tokens[0].token_sample_start, Some(1_000));
    assert_eq!(composed.tokens[1].confidence, None);
    assert!(!composed.tokens[1].surface_rewritten);
}

#[test]
fn one_pcm_slot_replaces_variant_chain_and_numeric_double() {
    for (first, second, final_word) in [("Vite", "Vita", "Vitae"), ("21.", "21.", "21.")] {
        let occurrence = occ(0, 16_000);
        let mut pcm = vec![0.0_f32; 24_000];
        pcm[1_000..4_000].fill(0.2);
        let mut ledger = word_adjudication_tests::measured_ledger(&occurrence, &pcm);
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
        let apple_first = obs(ObservationProducer::Apple, 0, occurrence.clone());
        ledger.admit_word_slots(&apple_first, &[WordPin::new(1_000, 4_000, first)]);
        let apple_second = obs(ObservationProducer::Apple, 1, occurrence.clone());
        ledger.admit_word_slots(&apple_second, &[WordPin::new(1_000, 4_000, second)]);
        let mut whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
        word_adjudication_tests::corroborate_candidate(
            &mut ledger,
            &mut whisper,
            &[WordPin::new(1_000, 4_000, final_word).with_decode_window(0, 16_000)],
            (0, 20_000),
        );

        assert_eq!(ledger.text_of(&occurrence), Some(final_word));
        let slots = ledger.slots_of(&occurrence).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].producer, ObservationProducer::Whisper);
        assert!(
            ledger
                .layer_trail()
                .iter()
                .any(|entry| { entry.observation.producer == ObservationProducer::Apple })
        );
        ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
        ledger.seal(&occurrence).unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some(final_word));
        ledger.assert_slot_labels();
    }
}

#[test]
fn word_interleave_fills_pcm_gaps_without_moving_committed_slots() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    ledger.admit_word_slots(
        &apple,
        &[
            crate::pipeline::acoustic_ledger::WordPin::new(1_000, 2_000, "po"),
            crate::pipeline::acoustic_ledger::WordPin::new(5_000, 6_000, "w"),
        ],
    );
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    ledger.admit_word_slots(
        &whisper,
        &[
            crate::pipeline::acoustic_ledger::WordPin::new(3_000, 4_000, "poproszę"),
            crate::pipeline::acoustic_ledger::WordPin::new(7_000, 8_000, "prostu"),
            crate::pipeline::acoustic_ledger::WordPin::new(9_000, 10_000, "sumie"),
        ],
    );
    assert_eq!(
        ledger.text_of(&occurrence),
        Some("po poproszę w prostu sumie")
    );
    let held = ledger.slots_of(&occurrence).unwrap().to_vec();
    let late = obs(ObservationProducer::Whisper, 1, occurrence.clone());
    ledger.admit_word_slots(
        &late,
        &[crate::pipeline::acoustic_ledger::WordPin::new(
            7_000, 8_000, "prostu",
        )],
    );
    assert_eq!(ledger.slots_of(&occurrence).unwrap().len(), held.len());
    assert_eq!(
        ledger.text_of(&occurrence),
        Some("po poproszę w prostu sumie")
    );
    ledger.assert_slot_labels();
}

#[test]
fn adjacent_swap_inserts_new_word_without_reordering_neighbours() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    ledger.admit_word_slots(
        &apple,
        &[
            crate::pipeline::acoustic_ledger::WordPin::new(1_000, 2_000, "parę"),
            crate::pipeline::acoustic_ledger::WordPin::new(7_000, 8_000, "słów"),
        ],
    );
    let whisper = obs(ObservationProducer::Whisper, 0, occurrence.clone());
    ledger.admit_word_slots(
        &whisper,
        &[crate::pipeline::acoustic_ledger::WordPin::new(
            4_000,
            5_000,
            "korzystając",
        )],
    );
    assert_eq!(ledger.text_of(&occurrence), Some("parę korzystając słów"));
    assert_eq!(ledger.slots_of(&occurrence).unwrap().len(), 3);
    ledger.assert_slot_labels();
}

#[test]
fn cloud_live_word_on_a_sealed_owner_stays_visible() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::Apple, 0, occurrence.clone()),
                "zostaje"
            )
            .is_insert()
    );
    ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
    ledger.seal(&occurrence).unwrap();
    let observation = obs(ObservationProducer::CloudLive, 2, occurrence.clone());
    let receipt = ledger.admit_word_slots(
        &observation,
        &[crate::pipeline::acoustic_ledger::WordPin::new(
            1_000, 4_000, "pozno",
        )],
    );
    assert!(matches!(
        receipt,
        MutationReceipt::KeepVisibleUnanchored {
            reason: NoAuthorityReason::LateCloudLiveWordSealedOwner,
            ref label,
            ..
        } if label.contains("pozno")
    ));
    assert_eq!(ledger.text_of(&occurrence), Some("zostaje"));
}
#[test]
fn w0_falsifier_whisper_slot_replacement_accounts_for_missing_word() {
    let (mut ledger, occurrence) = whisper_only_qualified_ledger();
    let apple = obs(ObservationProducer::Apple, 0, occurrence.clone());
    ledger.admit_pinned_label(
        &apple,
        "Iwo plan wraca",
        &[
            WordPin::new(1_000, 4_000, "Iwo"),
            WordPin::new(5_000, 8_000, "plan"),
            WordPin::new(9_000, 14_000, "wraca"),
        ],
    );
    assert_eq!(ledger.slots_of(&occurrence).unwrap().len(), 3);
    let whisper = obs(ObservationProducer::Whisper, 1, occurrence.clone());
    let mut replacement = ledger.slots_of(&occurrence).unwrap().to_vec();
    replacement.remove(1);
    for slot in &mut replacement {
        slot.producer = ObservationProducer::Whisper;
        slot.observation = whisper.clone();
    }
    ledger.admit_with_slots(&whisper, "Iwo wraca", Some(replacement), true);
    assert!(
        ledger
            .slots_of(&occurrence)
            .unwrap()
            .iter()
            .any(|slot| slot.text == "plan"),
        "omitted pin vanished without a no-speech receipt"
    );
}

/// The fdda978f phantom loop: decoder outros on measured silence. An
/// automatic whole-label claim with no measured-speech overlap is kept
/// visible as evidence and never enters the document; the same word on
/// measured speech inserts; a human on silence is never gated.
#[test]
fn automatic_label_on_measured_silence_is_kept_visible_never_inserted() {
    use crate::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;
    use crate::stt::tail_provider::TailSampleRange;

    let mut ledger = AcousticLedger::new();
    ledger.bind_capture_rate(48_000);
    ledger.record_speech_evidence(&AcousticSpeechEvidence::measured(
        CaptureEvidenceIdentity::new("phantom-session", 1),
        SILERO_BOUNDARIES_PRODUCER,
        AcousticAvailability::Observed {
            observed_samples: 960_000,
        },
        vec![TailSampleRange {
            session: "phantom-session".to_string(),
            capture_epoch: 1,
            sample_start: 48_000,
            sample_end: 96_000,
        }],
    ));
    let phantom = OccurrenceIdentity::new("phantom-session", 1, 480_000, 528_000);
    let receipt = ledger.admit(
        &obs(ObservationProducer::Whisper, 0, phantom.clone()),
        "uwagę.",
    );
    assert!(matches!(
        receipt,
        MutationReceipt::KeepVisibleUnanchored {
            reason: NoAuthorityReason::NoMeasuredSpeech,
            ..
        }
    ));
    assert_eq!(ledger.text_of(&phantom), None);
    let real = OccurrenceIdentity::new("phantom-session", 1, 48_000, 96_000);
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::Whisper, 1, real.clone()),
                "uwagę."
            )
            .is_insert()
    );
    let human = OccurrenceIdentity::new("phantom-session", 1, 624_000, 672_000);
    assert!(
        ledger
            .admit(
                &obs(ObservationProducer::ManualHuman, 2, human.clone()),
                "uwagę."
            )
            .is_insert()
    );
}
