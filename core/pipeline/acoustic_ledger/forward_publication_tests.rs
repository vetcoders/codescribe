//! Integrator regressions from native coarse-label and good-to-bad mutations.
//! Synthetic labels and PCM test authority/conservation, not recognition WER.
use super::word_adjudication_tests::measured_ledger_at_rate;
use super::*;

fn fixture(start: u64, end: u64, capture_end: u64) -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new("forward-publication", 1, start, end);
    let mut pcm = vec![0.0; capture_end as usize];
    pcm[start as usize..end as usize].fill(0.2);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
    assert!(ledger.admit(&apple, "prowizoryczna").grants_mutation());
    (ledger, owner)
}

fn fixture_words(
    start: u64,
    end: u64,
    capture_end: u64,
    ranges: &[(u64, u64)],
) -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new("forward-publication", 1, start, end);
    let mut pcm = vec![0.00001; capture_end as usize];
    for &(a, b) in ranges {
        pcm[a as usize..b as usize].fill(0.2);
    }
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
    assert!(ledger.admit(&apple, "prowizoryczna").grants_mutation());
    (ledger, owner)
}

fn pins(ranges: &[(u64, u64)], decode: (u64, u64)) -> Vec<WordPin> {
    ranges
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| {
            WordPin::new(start, end, format!("słowo{i}")).with_decode_window(decode.0, decode.1)
        })
        .collect()
}

fn publish(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    request: u64,
    band: (u64, u64),
    selected: &[WordPin],
    full: &[WordPin],
) -> MutationReceipt {
    let observation = ledger.next_word_observation(ObservationProducer::Whisper, request, owner);
    ledger.stage_word_evidence(&observation, selected, Some("inprocess:whisper"), "final");
    ledger.stage_word_publication(&observation, band, false, full);
    ledger.admit_word_slots(&observation, selected)
}

#[test]
fn native_coarse_apple_ranges_accept_complete_measured_partitions() {
    for (start, end, capture_end) in [(31_488, 330_240, 397_296), (11_520, 221_280, 283_152)] {
        let width = (end - start) / 12;
        let ranges = (1..=5)
            .map(|i| (start + i * 2 * width, start + (i * 2 + 1) * width))
            .collect::<Vec<_>>();
        let (mut ledger, owner) = fixture_words(start, end, capture_end, &ranges);
        let words = pins(&ranges, (0, capture_end));
        let receipt = publish(&mut ledger, &owner, 2, (0, capture_end), &words, &words);
        assert!(receipt.grants_mutation(), "{owner:?}: {receipt:?}");
        assert_eq!(ledger.slots_of(&owner).unwrap().len(), 5);
        assert!(!ledger.text_recovery_pending(&owner));
        assert_eq!(ledger.conservation().residue(), 0);
    }
}

#[test]
fn owned_prefix_leaves_debt_even_when_full_context_heard_the_owner() {
    for prefix_words in [1, 2] {
        let (mut ledger, owner) = fixture_words(
            24_000,
            408_000,
            480_000,
            &[(48_000, 80_000), (160_000, 192_000), (336_000, 368_000)],
        );
        let full = pins(
            &[(48_000, 80_000), (160_000, 192_000), (336_000, 368_000)],
            (0, 432_000),
        );
        let band_end = if prefix_words == 1 { 144_000 } else { 288_000 };
        let receipt = publish(
            &mut ledger,
            &owner,
            2,
            (0, band_end),
            &full[..prefix_words],
            &full,
        );
        assert!(receipt.grants_mutation(), "{receipt:?}");
        assert!(ledger.text_recovery_pending(&owner));
        assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
        let held = ledger.slots_of(&owner).unwrap().to_vec();
        assert!(!held.iter().any(|word| word.text == "prowizoryczna"));
        let receipt = publish(
            &mut ledger,
            &owner,
            3,
            (band_end, 432_000),
            &full[prefix_words..],
            &full,
        );
        assert!(receipt.grants_mutation(), "tail: {receipt:?}");
        assert!(!ledger.text_recovery_pending(&owner));
        assert_eq!(ledger.slots_of(&owner).unwrap().len(), 3);
        assert!(
            held.iter()
                .all(|slot| ledger.slots_of(&owner).unwrap().contains(slot))
        );
    }
}

#[test]
fn empty_return_does_not_settle_unpublished_speech() {
    let (mut ledger, owner) = fixture(24_000, 408_000, 480_000);
    let full = pins(&[(48_000, 80_000), (336_000, 368_000)], (0, 432_000));
    assert!(publish(&mut ledger, &owner, 2, (0, 288_000), &full[..1], &full).grants_mutation());
    publish(&mut ledger, &owner, 3, (288_000, 432_000), &[], &full);
    assert!(ledger.text_recovery_pending(&owner));
    assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
}

#[test]
fn filtered_subset_cannot_claim_the_whole_publication_band() {
    let (mut ledger, owner) = fixture(24_000, 408_000, 480_000);
    let full = pins(
        &[(48_000, 80_000), (160_000, 192_000), (336_000, 368_000)],
        (0, 432_000),
    );
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    publish(&mut ledger, &owner, 2, (0, 288_000), &full[..1], &full);
    assert_eq!(ledger.slots_of(&owner).unwrap(), before);
}

#[test]
fn published_word_label_and_geometry_survive_later_whisper_partition() {
    let owner = OccurrenceIdentity::new("blyskowicznie-regression", 1, 1_350_000, 1_500_000);
    let mut pcm = vec![0.0; 1_728_000];
    pcm[1_374_720..1_416_960].fill(0.2);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    let good = [WordPin::new(1_374_720, 1_416_960, "błyskowicznie")
        .with_decode_window(1_152_000, 1_584_000)];
    assert!(
        publish(&mut ledger, &owner, 9, (1_296_000, 1_440_000), &good, &good).grants_mutation()
    );
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    let bad = [
        WordPin::new(1_374_720, 1_416_000, "wyskocznie").with_decode_window(1_296_000, 1_728_000)
    ];
    publish(&mut ledger, &owner, 10, (1_440_000, 1_584_000), &bad, &bad);
    assert_eq!(ledger.text_of(&owner), Some("błyskowicznie"));
    assert_eq!(ledger.slots_of(&owner).unwrap(), before);
}

#[test]
fn measured_word_omission_cannot_be_authorized_as_coarse_refinement() {
    let owner = OccurrenceIdentity::new("preserve-negation", 1, 0, 192_000);
    let mut pcm = vec![0.0; 240_000];
    pcm[48_000..144_000].fill(0.2);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    let full = [
        WordPin::new(48_000, 72_000, "nie").with_decode_window(0, 240_000),
        WordPin::new(96_000, 144_000, "usuwaj").with_decode_window(0, 240_000),
    ];
    assert!(publish(&mut ledger, &owner, 1, (0, 192_000), &full, &full).grants_mutation());
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    let omitted = [WordPin::new(48_000, 144_000, "usuwaj").with_decode_window(0, 240_000)];
    publish(&mut ledger, &owner, 2, (0, 192_000), &omitted, &omitted);
    assert_eq!(ledger.slots_of(&owner).unwrap(), before);
    assert_eq!(ledger.text_of(&owner), Some("nie usuwaj"));
}

#[test]
fn quiet_publication_accounts_only_its_measured_band() {
    let owner = OccurrenceIdentity::new("quiet-publication", 1, 0, 192_000);
    let mut pcm = vec![0.00001; 240_000];
    pcm[48_000..96_000].fill(0.2);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    assert!(ledger.require_text_recovery(&owner));
    let observation = ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, owner.clone());
    let quiet = OccurrenceIdentity::new(&owner.session, 1, 144_000, 192_000);
    assert!(ledger.record_quiet_publication_band(&observation, &quiet));
    assert!(
        ledger.text_recovery_pending(&owner),
        "quiet work cannot certify the voiced prefix"
    );
    assert_eq!(ledger.text_of(&owner), None);
    let voiced = OccurrenceIdentity::new(&owner.session, 1, 48_000, 96_000);
    assert!(!ledger.record_quiet_publication_band(&observation, &voiced));
    let foreign = OccurrenceIdentity::new("foreign-capture", 1, 144_000, 192_000);
    assert!(!ledger.record_quiet_publication_band(&observation, &foreign));
    let wrong_epoch = OccurrenceIdentity::new(&owner.session, 2, 144_000, 192_000);
    assert!(!ledger.record_quiet_publication_band(&observation, &wrong_epoch));
}

#[test]
fn unobserved_audio_does_not_become_a_quiet_publication() {
    use crate::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    let owner = OccurrenceIdentity::new("unobserved-publication", 1, 0, 192_000);
    let mut pcm = vec![0.00001; 240_000];
    pcm[48_000..96_000].fill(0.2);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    ledger.speech_evidence = Some(AcousticSpeechEvidence::measured(
        CaptureEvidenceIdentity::new(&owner.session, 1),
        crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
        AcousticAvailability::NotObserved,
        Vec::new(),
    ));
    let observation = ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, owner.clone());
    let quiet = OccurrenceIdentity::new(&owner.session, 1, 144_000, 192_000);
    assert!(!ledger.record_quiet_publication_band(&observation, &quiet));
}

#[test]
fn first_owned_whisper_label_can_correct_one_measured_apple_word_once() {
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailProviderPayload, TailRequestIdentity, TailSegmentGrain, TailTimingQuality,
        TimedTailSegment,
    };
    for defect in ["valid", "narrow", "negation", "unbound", "unowned"] {
        let owner = OccurrenceIdentity::new(format!("first-owned-apple-{defect}"), 1, 0, 432_000);
        let mut pcm = vec![0.00001; 432_000];
        pcm[57_600..76_800].fill(0.2);
        let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 42, 0, owner.clone());
        let held_text = if defect == "negation" { "nie" } else { "stare" };
        assert!(
            ledger
                .admit_word_slots(
                    &apple,
                    &[WordPin::new(57_600, 76_800, held_text).with_decode_window(0, 432_000)]
                )
                .grants_mutation()
        );
        let before = ledger.slots_of(&owner).unwrap().to_vec();
        let start = if defect == "narrow" { 60_000 } else { 57_600 };
        let decode = TailSampleRange {
            session: owner.session.clone(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 432_000,
        };
        let words = [WordPin::new(start, 76_800, "nowe").with_decode_window(0, 432_000)];
        let payload = TailProviderPayload {
            identity: TailRequestIdentity {
                request_id: 7,
                range: decode.clone(),
            },
            text: "nowe".into(),
            segments: vec![TimedTailSegment {
                text: "nowe".into(),
                range: TailSampleRange {
                    sample_start: start,
                    sample_end: 76_800,
                    ..decode
                },
                confidence: None,
                grain: TailSegmentGrain::Word,
            }],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.0),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain: TailSegmentGrain::Word,
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        };
        if defect != "unbound" {
            ledger.bind_word_provider(&payload);
        }
        // Owner-relative stream IDs and capture-grid provider IDs are distinct.
        let observation = ledger.next_word_observation(ObservationProducer::Whisper, 42, &owner);
        ledger.stage_word_evidence(&observation, &words, None, "unknown");
        let band = if defect == "unowned" {
            (288_000, 432_000)
        } else {
            (0, 288_000)
        };
        ledger.stage_word_publication(&observation, band, false, &words);
        let receipt = ledger.admit_word_slots(&observation, &words);
        if defect == "valid" {
            assert!(receipt.grants_mutation(), "{receipt:?}");
            assert_eq!(ledger.text_of(&owner), Some("nowe"));
            assert!(
                ledger
                    .word_choices()
                    .iter()
                    .any(|choice| choice.reason == "owned_whisper_apple_word" && choice.accepted)
            );
            let published = ledger.slots_of(&owner).unwrap().to_vec();
            let bad = [WordPin::new(57_600, 76_800, "gorsze").with_decode_window(0, 432_000)];
            publish(&mut ledger, &owner, 8, (0, 288_000), &bad, &bad);
            assert_eq!(ledger.slots_of(&owner).unwrap(), published);
        } else {
            assert_eq!(
                ledger.slots_of(&owner).unwrap(),
                before,
                "{defect}: {receipt:?}"
            );
        }
    }
}

#[test]
fn native_sie_utterance_label_refines_to_three_measured_words() {
    let ranges = [(45_120, 74_880), (74_880, 79_680), (79_680, 100_800)];
    let (mut ledger, owner) = fixture_words(36_096, 150_720, 203_776, &ranges);
    let words = ranges
        .into_iter()
        .zip(["Spodobało", "Ci", "się?"])
        .map(|((a, b), word)| WordPin::new(a, b, word).with_decode_window(0, 203_776))
        .collect::<Vec<_>>();
    assert!(publish(&mut ledger, &owner, 2, (0, 203_776), &words, &words).grants_mutation());
    assert_eq!(ledger.text_of(&owner), Some("Spodobało Ci się?"));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 3);
    assert!(!ledger.text_recovery_pending(&owner));
    assert_eq!(ledger.conservation().residue(), 0);
}

fn dt_partition_fixture(
    session: &str,
    labels: [&str; 2],
    quiet_join: bool,
    speech_end: u64,
) -> (
    AcousticLedger,
    OccurrenceIdentity,
    crate::audio::capture_receipt::CaptureEnergyOwner,
) {
    use crate::audio::capture_receipt::{CaptureEnergyOwner, CaptureLevelAccumulator};
    let owner = OccurrenceIdentity::new(session, 1, 42_240, 115_200);
    let mut pcm = vec![0.00001; 171_008];
    pcm[58_368..speech_end as usize].fill(0.2);
    if quiet_join {
        pcm[62_880..65_760].fill(0.00001);
    }
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, 48_000);
    let energy = CaptureEnergyOwner::bind(session, 1);
    CaptureLevelAccumulator::bound_to_at_rate(&energy, 48_000).push_samples(&pcm);
    let wrong = [
        WordPin::new(55_680, 64_320, labels[0]).with_decode_window(0, 171_008),
        WordPin::new(64_320, 86_400, labels[1]).with_decode_window(0, 171_008),
    ];
    assert!(publish(&mut ledger, &owner, 10, (0, 171_008), &wrong, &wrong).grants_mutation());
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 2);
    (ledger, owner, energy)
}

fn offer_dt_partition(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    energy: &crate::audio::capture_receipt::CaptureEnergyOwner,
    stage_boundary: bool,
    exact: bool,
    text: &str,
) -> MutationReceipt {
    use crate::stt::tail_provider::*;
    let range = TailSampleRange {
        session: owner.session.clone(),
        capture_epoch: owner.capture_epoch,
        sample_start: 0,
        sample_end: 171_008,
    };
    let word = WordPin::new(57_600, 82_080, text).with_decode_window(0, 171_008);
    let payload = TailProviderPayload {
        identity: TailRequestIdentity {
            request_id: (1_u64 << 62) + 1,
            range: range.clone(),
        },
        text: text.into(),
        segments: vec![TimedTailSegment {
            confidence: None,
            grain: TailSegmentGrain::Phrase,
            text: text.into(),
            range: TailSampleRange {
                sample_start: 57_600,
                sample_end: 82_080,
                ..range
            },
        }],
        avg_logprob: None,
        compression_ratio: None,
        provider_id: TailProviderId::AppleDictation,
        elapsed_ms: 1,
        evidence: TailProviderEvidence {
            segment_grain: TailSegmentGrain::Phrase,
            source: TailEvidenceSource::AppleDictation,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: if exact {
                TailTimingQuality::ExactSampleRange
            } else {
                TailTimingQuality::Synthetic
            },
            avg_logprob: None,
            vad_selected_samples: None,
        },
    };
    payload.validate().unwrap();
    ledger.bind_word_provider(&payload);
    let observation = ledger.next_word_observation(
        ObservationProducer::AppleDictation,
        payload.identity.request_id,
        owner,
    );
    ledger.stage_word_evidence(&observation, std::slice::from_ref(&word), None, "final");
    if stage_boundary {
        ledger.stage_dt_boundary_evidence(&observation, energy, 171_008);
    }
    ledger.admit_word_slots(&observation, &[word])
}

#[test]
fn independent_dt_refines_native_split_preserving_raw_pin_and_uncovered_speech() {
    use crate::pipeline::trail::{TrailEvent, TrailSink, read_trail, replay_decisions, trail_path};
    let session = "native-dziekuje-partition";
    let dir = tempfile::tempdir().unwrap();
    let sink = TrailSink::open_in(dir.path(), session, 1, 256).unwrap();
    let (mut ledger, owner, energy) =
        dt_partition_fixture(session, ["Cię", "kułaj."], false, 90_624);
    let before = ledger.slots_of(&owner).unwrap().to_vec();
    let receipt = offer_dt_partition(&mut ledger, &owner, &energy, true, true, "Dziękuję");
    assert!(
        receipt.grants_mutation(),
        "{receipt:?}; alternatives={:?}",
        ledger.slot_alternatives()
    );
    assert_eq!(ledger.text_of(&owner), Some("Dziękuję"));
    let slots = ledger.slots_of(&owner).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(
        (slots[0].sample_start, slots[0].sample_end),
        (57_600, 82_080),
        "raw DT pin must not stretch"
    );
    let operation = ledger.slot_operations().last().unwrap();
    assert_eq!(operation.sources, before);
    for source in &before {
        assert!(
            operation
                .source_ranges
                .iter()
                .any(|range| range.same_capture(&owner)
                    && range.sample_start <= source.sample_start
                    && source.sample_end <= range.sample_end),
            "every original physical source remains in the operation: {:?}",
            operation.source_ranges
        );
    }
    assert!(
        ledger.text_recovery_pending(&owner),
        "the measured90ms source tail remains debt"
    );
    assert_eq!(ledger.seal(&owner), Err(SealRefusal::TextRecoveryPending));
    assert!(ledger.word_deletions().is_empty());
    assert_eq!(ledger.conservation().residue(), 0);
    drop(sink);
    let rows = read_trail(&trail_path(dir.path(), session).unwrap()).unwrap();
    let replay = replay_decisions(&rows, |_, _, _| {}).unwrap();
    assert_eq!(replay.text_of(&owner), ledger.text_of(&owner));
    assert_eq!(replay.slots_of(&owner), ledger.slots_of(&owner));
    assert!(replay.text_recovery_pending(&owner));
    let mut corrupt = rows.clone();
    let facts = corrupt
        .iter_mut()
        .find_map(|row| match &mut row.event {
            TrailEvent::SlotStart { operation }
                if operation.observation.producer == ObservationProducer::AppleDictation =>
            {
                operation
                    .word_evidence
                    .as_mut()
                    .map(|input| &mut input.boundary_evidence)
            }
            _ => None,
        })
        .expect("persisted DT boundary input");
    assert_eq!(facts.len(), 1);
    facts[0].continuously_voiced = false;
    assert!(
        replay_decisions(&corrupt, |_, _, _| {}).is_err(),
        "quiet boundary cannot replay a committed contraction"
    );
}

#[test]
fn independent_dt_cannot_contract_repetition_negation_quiet_or_unverified_input() {
    use crate::audio::capture_receipt::CaptureEnergyOwner;
    for defect in [
        "repetition",
        "negation",
        "quiet",
        "foreign",
        "missing",
        "inexact",
        "sealed",
    ] {
        let labels = match defect {
            "repetition" => ["raz", "raz"],
            "negation" => ["nie", "usuwaj"],
            _ => ["Cię", "kułaj."],
        };
        let (mut ledger, owner, energy) = dt_partition_fixture(
            &format!("dt-partition-{defect}"),
            labels,
            defect == "quiet",
            if defect == "sealed" { 86_400 } else { 90_624 },
        );
        if defect == "sealed" {
            ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
            ledger.note_frontier_return(&owner, ObservationProducer::Whisper);
            ledger
                .seal(&owner)
                .expect("complete measured words form the sealed control");
        }
        let before = ledger.slots_of(&owner).unwrap().to_vec();
        let foreign = CaptureEnergyOwner::bind("foreign", 1);
        offer_dt_partition(
            &mut ledger,
            &owner,
            if defect == "foreign" {
                &foreign
            } else {
                &energy
            },
            defect != "missing",
            defect != "inexact",
            if defect == "negation" {
                "usuwaj"
            } else {
                "Dziękuję"
            },
        );
        assert_eq!(ledger.slots_of(&owner).unwrap(), before, "{defect}");
        assert!(ledger.word_deletions().is_empty(), "{defect}");
        assert_eq!(ledger.conservation().residue(), 0, "{defect}");
    }
}
