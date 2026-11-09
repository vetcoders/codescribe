//! Integrator-owned regressions for word policies v38 to v41.
//!
//! Source under test: `dbd2b84b4` keeps an explicit sequence target exclusive
//! across decoder passes, and `d545429fa` lets complete native DT evidence
//! address one unmapped word between two mapped neighbours and keeps the
//! latest accepted 1:1 word decision through the commit guard. `41900ec8a`
//! expires sequence exclusions with their target, keeps the untouched full
//! Whisper frame as context for words this owner already holds, and refuses a
//! lone DT label against distinct complete Whisper windows. `4127d0e2a`
//! records that refusal as a resolved selection of the retained word and
//! marks context-only Whisper evidence, which may corroborate retention but
//! never a lexical mutation.
//!
//! Recorded take S5 (`agent-channels-cf70e849`, app `f90d941d1`, 2026-11-08):
//! the inputs below are copied from the decision trail, lines 351, 375, 394
//! and 414. The ElevenLabs reading of the same WAV (`run.XeKMEY78`) is
//! "agent oparty o Frontier Model, bo mnie stać". No audio is stored here.
//! Synthetic PCM supplies energy over the recorded Silero ranges, exactly as
//! the other recorded fixtures in this ledger do. No decoder runs.
use super::word_adjudication_tests::measured_ledger_at_rate;
use super::*;
use crate::audio::capture_receipt::{
    AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity, SpeechAnchor,
    SpeechAnchorEvidence,
};
use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;
use crate::pipeline::word_confidence::{WordConfidence, WordConfidenceSource};
use crate::stt::tail_provider::{TailRequestIdentity, TailSampleRange};
use std::collections::BTreeSet;

const RATE: u32 = 48_000;
const S5_OWNER: (u64, u64) = (3_649_920, 3_976_800);
const S5_WHISPER_REQUEST: u64 = 25;
const S5_PCM_SAMPLES: usize = 4_386_720;

/// Text, original sample range and Whisper token confidence (milli, tokens).
type Word = (&'static str, u64, u64, Option<(i32, u16)>);

struct WhisperStep {
    generation: u64,
    request: u64,
    decode: (u64, u64),
    band: (u64, u64),
    selected: &'static [Word],
    publication: &'static [Word],
    speech_observed: u64,
    speech: &'static [(u64, u64)],
    anchor_observed: u64,
}

const S5_WHISPER: [WhisperStep; 3] = [
    WhisperStep {
        generation: 0,
        request: 25,
        decode: (3_456_000, 3_888_000),
        band: (3_600_000, 3_744_000),
        selected: &[
            ("będzie", 3_672_960, 3_708_480, Some((-14, 1))),
            ("to", 3_708_480, 3_720_960, Some((-24, 1))),
            ("dobry", 3_720_960, 3_749_760, Some((-35, 1))),
        ],
        publication: &[
            ("wiatr", 3_462_720, 3_474_240, Some((-2375, 3))),
            ("będzie", 3_474_240, 3_484_800, Some((-53, 1))),
            ("skrojony", 3_484_800, 3_553_920, Some((-153, 4))),
            ("przez", 3_553_920, 3_567_360, Some((-117, 1))),
            ("tego", 3_567_360, 3_579_840, Some((-103, 1))),
            ("agenta", 3_579_840, 3_608_640, Some((-78, 2))),
            ("i", 3_608_640, 3_672_960, Some((-134, 1))),
            ("będzie", 3_672_960, 3_708_480, Some((-14, 1))),
            ("to", 3_708_480, 3_720_960, Some((-24, 1))),
            ("dobry", 3_720_960, 3_749_760, Some((-35, 1))),
            ("agent", 3_749_760, 3_769_920, Some((-21, 1))),
            ("oparty", 3_769_920, 3_800_640, Some((-343, 3))),
            ("o", 3_800_640, 3_803_520, Some((-33, 1))),
            ("Frontier", 3_803_520, 3_827_520, Some((-628, 2))),
            ("Model", 3_827_520, 3_840_000, Some((-530, 1))),
        ],
        speech_observed: 3_976_800,
        speech: &[
            (3_058_176, 3_142_656),
            (3_174_912, 3_506_688),
            (3_528_192, 3_624_960),
            (3_655_680, 3_952_128),
        ],
        anchor_observed: 3_976_800,
    },
    WhisperStep {
        generation: 1,
        request: 26,
        decode: (3_600_000, 4_032_000),
        band: (3_744_000, 3_888_000),
        selected: &[
            ("agent", 3_749_760, 3_768_000, Some((-44, 1))),
            ("oparty", 3_768_000, 3_797_760, Some((-220, 3))),
            ("o", 3_797_760, 3_803_520, Some((-33, 1))),
            ("Frontier", 3_803_520, 3_828_480, Some((-554, 2))),
            ("model,", 3_828_480, 3_850_560, Some((-955, 2))),
            ("bo", 3_850_560, 3_877_440, Some((-234, 1))),
        ],
        publication: &[
            ("i", 3_600_000, 3_696_000, Some((-1033, 1))),
            ("będzie", 3_696_000, 3_708_480, Some((-70, 1))),
            ("to", 3_708_480, 3_721_920, Some((-60, 1))),
            ("dobry", 3_721_920, 3_749_760, Some((-54, 1))),
            ("agent", 3_749_760, 3_768_000, Some((-44, 1))),
            ("oparty", 3_768_000, 3_797_760, Some((-220, 3))),
            ("o", 3_797_760, 3_803_520, Some((-33, 1))),
            ("Frontier", 3_803_520, 3_828_480, Some((-554, 2))),
            ("model,", 3_828_480, 3_850_560, Some((-955, 2))),
            ("bo", 3_850_560, 3_877_440, Some((-234, 1))),
            ("mnie", 3_877_440, 3_917_760, Some((-389, 1))),
            ("stać,", 3_917_760, 3_941_760, Some((-1004, 3))),
        ],
        speech_observed: 4_081_440,
        speech: &[
            (3_058_176, 3_142_656),
            (3_174_912, 3_506_688),
            (3_528_192, 3_624_960),
            (3_655_680, 3_952_128),
            (3_990_528, 4_081_440),
        ],
        anchor_observed: 4_081_920,
    },
    WhisperStep {
        generation: 2,
        request: 27,
        decode: (3_744_000, 4_176_000),
        band: (3_888_000, 4_032_000),
        selected: &[
            ("nie", 3_878_400, 3_917_760, Some((-657, 1))),
            ("stać,", 3_917_760, 3_941_760, Some((-718, 3))),
        ],
        publication: &[
            ("dobry", 3_744_000, 3_752_640, Some((-1061, 1))),
            ("agent", 3_752_640, 3_768_000, Some((-31, 1))),
            ("oparty", 3_768_000, 3_800_640, Some((-89, 3))),
            ("o", 3_800_640, 3_803_520, Some((-27, 1))),
            ("Frontier", 3_803_520, 3_828_480, Some((-332, 2))),
            ("Model,", 3_828_480, 3_850_560, Some((-682, 2))),
            ("bo", 3_850_560, 3_878_400, Some((-285, 1))),
            ("nie", 3_878_400, 3_917_760, Some((-657, 1))),
            ("stać,", 3_917_760, 3_941_760, Some((-718, 3))),
            ("więc", 3_941_760, 4_027_200, Some((-185, 1))),
            ("przystaw", 4_027_200, 4_063_680, Some((-822, 2))),
            ("model", 4_063_680, 4_090_560, Some((-61, 1))),
            ("na", 4_090_560, 4_135_680, Some((-14, 1))),
            ("stronie", 4_135_680, 4_169_280, Some((-35, 2))),
        ],
        speech_observed: 4_232_640,
        speech: &[
            (3_058_176, 3_142_656),
            (3_174_912, 3_506_688),
            (3_528_192, 3_624_960),
            (3_655_680, 3_952_128),
            (3_990_528, 4_232_640),
        ],
        anchor_observed: 4_232_640,
    },
];
const S5_DT_REQUEST: u64 = 6_917_529_027_641_081_864;
const S5_DT_DECODE: (u64, u64) = (3_505_920, 4_120_800);
const S5_DT_WORDS: &[Word] = &[
    ("i", 3_697_440, 3_704_640, None),
    ("będzie", 3_704_640, 3_717_600, None),
    ("to", 3_717_600, 3_732_000, None),
    ("dobry", 3_739_200, 3_759_360, None),
    ("Agent", 3_759_360, 3_786_720, None),
    ("oparty", 3_786_720, 3_804_000, None),
    ("o", 3_804_000, 3_808_320, None),
    ("front", 3_808_320, 3_831_360, None),
    ("model", 3_831_360, 3_854_400, None),
    ("bo", 3_876_000, 3_887_520, None),
    ("mnie", 3_913_440, 3_923_520, None),
    ("stać", 3_923_520, 3_945_120, None),
];
const S5_DT_SPEECH_OBSERVED: u64 = 4_386_240;
const S5_DT_SPEECH: &[(u64, u64)] = &[
    (3_058_176, 3_142_656),
    (3_174_912, 3_506_688),
    (3_528_192, 3_624_960),
    (3_655_680, 3_952_128),
    (3_990_528, 4_237_824),
    (4_288_512, 4_386_240),
];
const S5_DT_ANCHOR_OBSERVED: u64 = 4_386_720;
/// Retained slots the recorded DT admission saw (trail line 414).
const S5_SOURCE_SLOTS: &[(&str, u64, u64, u64)] = &[
    ("będzie", 3_672_960, 3_708_480, 0),
    ("to", 3_708_480, 3_720_960, 0),
    ("dobry", 3_720_960, 3_749_760, 0),
    ("agent", 3_749_760, 3_768_000, 1),
    ("oparty", 3_768_000, 3_797_760, 1),
    ("o", 3_797_760, 3_803_520, 1),
    ("Frontier", 3_803_520, 3_828_480, 1),
    ("model,", 3_828_480, 3_850_560, 1),
    ("bo", 3_850_560, 3_877_440, 1),
    ("nie", 3_878_400, 3_917_760, 2),
    ("stać,", 3_917_760, 3_941_760, 2),
];

fn pins_of(words: &[Word], decode: (u64, u64)) -> Vec<WordPin> {
    words
        .iter()
        .map(|&(text, start, end, confidence)| {
            let mut pin = WordPin::new(start, end, text).with_decode_window(decode.0, decode.1);
            pin.confidence = confidence.map(|(milli_value, token_count)| WordConfidence {
                source: WordConfidenceSource::WhisperTokenLogprob,
                milli_value,
                token_count,
            });
            pin
        })
        .collect()
}

fn sample_range(owner: &OccurrenceIdentity, start: u64, end: u64) -> TailSampleRange {
    TailSampleRange {
        session: owner.session.clone(),
        capture_epoch: owner.capture_epoch,
        sample_start: start,
        sample_end: end,
    }
}

/// The capture as one recorded admission saw it: Silero ranges and the single
/// anchor that was still open at the observed end.
fn capture_view(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    speech_observed: u64,
    speech: &[(u64, u64)],
    anchor_observed: u64,
) {
    ledger.speech_evidence = Some(AcousticSpeechEvidence::measured(
        CaptureEvidenceIdentity::new(&owner.session, owner.capture_epoch),
        SILERO_BOUNDARIES_PRODUCER,
        AcousticAvailability::Observed {
            observed_samples: speech_observed,
        },
        speech
            .iter()
            .map(|&(start, end)| sample_range(owner, start, end))
            .collect(),
    ));
    ledger.speech_anchors = Some(SpeechAnchorEvidence {
        identity: CaptureEvidenceIdentity::new(&owner.session, owner.capture_epoch),
        availability: AcousticAvailability::Observed {
            observed_samples: anchor_observed,
        },
        anchors: vec![SpeechAnchor {
            range: sample_range(owner, 0, anchor_observed),
            open_at_end: true,
        }],
    });
}

fn s5_pcm() -> Vec<f32> {
    let mut pcm = vec![0.0; S5_PCM_SAMPLES];
    for &(start, end) in S5_DT_SPEECH {
        pcm[start as usize..end as usize].fill(0.1);
    }
    pcm
}

/// Stage evidence the way the recorded Whisper admission carried it: the
/// selected band words, the untouched full decode and the request envelope.
fn admit_whisper(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    step: &WhisperStep,
) -> MutationReceipt {
    capture_view(
        ledger,
        owner,
        step.speech_observed,
        step.speech,
        step.anchor_observed,
    );
    let observation = ObservationIdentity::new(
        ObservationProducer::Whisper,
        S5_WHISPER_REQUEST,
        step.generation,
        owner.clone(),
    );
    let selected = pins_of(step.selected, step.decode);
    ledger.stage_word_evidence(&observation, &selected, Some("inprocess:whisper"), "final");
    ledger.stage_word_publication(
        &observation,
        step.band,
        false,
        &pins_of(step.publication, step.decode),
    );
    let mut input = ledger
        .word_evidence_input(&observation)
        .expect("staged Whisper evidence")
        .clone();
    input.producer_request = Some(TailRequestIdentity {
        request_id: step.request,
        range: sample_range(owner, step.decode.0, step.decode.1),
    });
    input.timing = "exact_sample_range".into();
    ledger.restore_word_evidence(input);
    ledger.admit_word_slots(&observation, &selected)
}

fn slot_rows(slots: &[WordSlot]) -> Vec<(&str, u64, u64, u64)> {
    slots
        .iter()
        .map(|slot| {
            (
                slot.text.as_str(),
                slot.sample_start,
                slot.sample_end,
                slot.observation.generation,
            )
        })
        .collect()
}

/// The recorded S5 owner just before the native DT batch (trail line 414).
fn s5_fixture(session: &str) -> (AcousticLedger, OccurrenceIdentity) {
    let owner = OccurrenceIdentity::new(session, 1, S5_OWNER.0, S5_OWNER.1);
    let mut ledger = measured_ledger_at_rate(&owner, &s5_pcm(), RATE);
    for step in &S5_WHISPER {
        let receipt = admit_whisper(&mut ledger, &owner, step);
        assert!(
            receipt.grants_mutation(),
            "recorded Whisper generation {} must be admitted: {receipt:?}",
            step.generation
        );
    }
    assert_eq!(
        slot_rows(ledger.slots_of(&owner).unwrap()),
        S5_SOURCE_SLOTS,
        "the fixture must hold the slots the recorded DT batch saw"
    );
    (ledger, owner)
}

fn dt_observation(owner: &OccurrenceIdentity) -> ObservationIdentity {
    ObservationIdentity::new(
        ObservationProducer::AppleDictation,
        S5_DT_REQUEST,
        0,
        owner.clone(),
    )
}

/// Stage one complete native DT batch over the recorded decode window.
fn stage_dt(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    words: &[Word],
) -> (ObservationIdentity, Vec<WordPin>) {
    capture_view(
        ledger,
        owner,
        S5_DT_SPEECH_OBSERVED,
        S5_DT_SPEECH,
        S5_DT_ANCHOR_OBSERVED,
    );
    let dt = dt_observation(owner);
    ledger.word_adjudication.provider = Some((
        TailRequestIdentity {
            request_id: S5_DT_REQUEST,
            range: sample_range(owner, S5_DT_DECODE.0, S5_DT_DECODE.1),
        },
        "apple_dictation:apple_dictation".into(),
        "final".into(),
        "exact_sample_range".into(),
    ));
    let pins = pins_of(words, S5_DT_DECODE);
    ledger.stage_word_evidence(&dt, &pins, Some("apple_dictation:apple_dictation"), "final");
    (dt, pins)
}

fn committed_label(ledger: &AcousticLedger, owner: &OccurrenceIdentity) -> String {
    ledger
        .committed
        .get(owner)
        .map(|committed| committed.label.clone())
        .unwrap_or_default()
}

fn retained(slots: &[WordSlot], text: &str) -> WordSlot {
    slots
        .iter()
        .find(|slot| slot.text == text)
        .unwrap_or_else(|| panic!("recorded slot {text:?} missing"))
        .clone()
}

/// The slot that now holds the exact physical range of a recorded source.
fn held_at(ledger: &AcousticLedger, owner: &OccurrenceIdentity, source: &WordSlot) -> WordSlot {
    ledger
        .slots_of(owner)
        .expect("the S5 owner keeps its slots")
        .iter()
        .find(|slot| {
            slot.sample_start == source.sample_start && slot.sample_end == source.sample_end
        })
        .unwrap_or_else(|| panic!("no slot holds {source:?}"))
        .clone()
}

fn s5_after_dt(
    session: &str,
    words: &[Word],
) -> (
    AcousticLedger,
    OccurrenceIdentity,
    Vec<WordSlot>,
    ObservationIdentity,
) {
    let (mut ledger, owner) = s5_fixture(session);
    let before = ledger
        .slots_of(&owner)
        .expect("the S5 owner keeps its slots")
        .to_vec();
    let (dt, pins) = stage_dt(&mut ledger, &owner, words);
    ledger.admit_word_slots(&dt, &pins);
    (ledger, owner, before, dt)
}

/// The retained Whisper "nie" is still held, unchanged, and no DT output
/// spells "mnie" anywhere in the owner.
fn assert_nie_retained(
    ledger: &AcousticLedger,
    owner: &OccurrenceIdentity,
    nie: &WordSlot,
    case: &str,
) {
    assert_eq!(
        &held_at(ledger, owner, nie),
        nie,
        "{case}: the retained word must stay; committed {:?}",
        committed_label(ledger, owner)
    );
    assert!(
        !ledger
            .slots_of(owner)
            .unwrap()
            .iter()
            .any(|slot| slot.text == "mnie"),
        "{case}: no mapping may place mnie; committed {:?}",
        committed_label(ledger, owner)
    );
    assert_eq!(ledger.conservation().residue(), 0, "{case}");
}

/// Lena's main acceptance: the correct native word reaches the committed
/// document, not only an accepted row in `word_choices`. v37 and v38 park it
/// as `protected_source` against "nie" and "stać,".
#[test]
fn recorded_s5_native_mnie_survives_admission_and_commit() {
    let (ledger, owner, before, dt) = s5_after_dt("s5-mnie-commit", S5_DT_WORDS);
    let nie = retained(&before, "nie");
    let target = SlotTarget::from(&nie);
    let choice = ledger
        .word_choices()
        .iter()
        .rev()
        .find(|choice| choice.observation == dt && choice.targets == [target.clone()])
        .expect("DT mnie must reach the lexical question for the retained nie");
    assert!(
        choice.accepted && choice.lexical_resolved && choice.selected_label == "mnie",
        "{choice:?}"
    );
    let held = held_at(&ledger, &owner, &nie);
    assert_eq!(
        (held.text.as_str(), held.producer),
        ("mnie", ObservationProducer::AppleDictation),
        "the decision must survive the commit; committed {:?}",
        committed_label(&ledger, &owner)
    );
    assert!(ledger.slot_operations().iter().any(|operation| {
        operation.observation == dt
            && operation.kind == SlotOperationKind::Correct
            && operation
                .sources
                .iter()
                .map(SlotTarget::from)
                .collect::<Vec<_>>()
                == [target.clone()]
            && operation.outputs == [held.clone()]
    }));
    assert!(
        !ledger
            .slot_alternatives
            .iter()
            .any(|alternative| alternative.observation == dt && alternative.candidate == "mnie"),
        "mnie must not be parked as an alternative"
    );
    assert!(committed_label(&ledger, &owner).contains("bo mnie stać"));
    assert_eq!(
        ledger.slots_of(&owner).unwrap().len(),
        S5_SOURCE_SLOTS.len(),
        "the correction mints and loses no word"
    );
    assert_eq!(ledger.conservation().residue(), 0);
}

/// The same take, one word earlier. ElevenLabs and all three Whisper decodes
/// (requests 25, 26 and 27; logprob -628, -554, -332) hear "Frontier"; only DT
/// hears "front". An accepted `independent_same_speech_range` decision is
/// geometric eligibility, not lexical evidence that outweighs three decodes,
/// so the commit exemption must not let it overwrite the word.
#[test]
fn recorded_s5_native_dt_cannot_overwrite_a_word_three_whisper_decodes_agree_on() {
    let (ledger, owner, before, _) = s5_after_dt("s5-frontier-commit", S5_DT_WORDS);
    let frontier = retained(&before, "Frontier");
    let held = held_at(&ledger, &owner, &frontier);
    assert_eq!(
        (held.text.as_str(), held.producer),
        ("Frontier", ObservationProducer::Whisper),
        "a lone native label overwrote a word three Whisper decodes agree on; committed {:?}",
        committed_label(&ledger, &owner)
    );
}

/// v38: a DT word with a resolved sequence target addresses only that target,
/// even when its clock overlaps a neighbour from another Whisper generation.
/// v37 gave "bo" (generation 1) the next word "nie" (generation 2) as well.
#[test]
fn recorded_s5_mapped_dt_words_address_only_their_own_target_across_generations() {
    let (ledger, _, before, dt) = s5_after_dt("s5-exclusive-targets", S5_DT_WORDS);
    for (candidate, held) in [("dobry", "dobry"), ("Agent", "agent"), ("bo", "bo")] {
        let target = SlotTarget::from(&retained(&before, held));
        assert!(
            ledger.word_choices().iter().any(|choice| {
                choice.observation == dt
                    && choice.selected_label == candidate
                    && choice.targets == [target.clone()]
            }),
            "{candidate} must pose exactly one target, {held}"
        );
        assert!(
            !ledger.slot_alternatives.iter().any(|alternative| {
                alternative.observation == dt
                    && alternative.candidate == candidate
                    && alternative.sources.len() > 1
            }),
            "{candidate} must not claim a neighbouring word"
        );
    }
    // Physical identity ignores the label: a respelled copy names the same target.
    let nie = retained(&before, "nie");
    let mut respelled = nie.clone();
    respelled.text = "mnie".into();
    assert_eq!(SlotTarget::from(&respelled), SlotTarget::from(&nie));
}

/// v39 mapping preconditions: both immediate neighbours mapped, exactly one
/// held word in the gap, original PCM overlap, complete final native evidence.
#[test]
fn recorded_s5_native_gap_mapping_requires_neighbours_overlap_and_final_evidence() {
    let without_right = &S5_DT_WORDS[..S5_DT_WORDS.len() - 1];
    let without_bo = S5_DT_WORDS
        .iter()
        .copied()
        .filter(|word| word.0 != "bo")
        .collect::<Vec<_>>();
    // Shifted entirely past the held "nie" [3_878_400, 3_917_760), still
    // between the native "bo" and "stać" pins.
    let shifted = S5_DT_WORDS
        .iter()
        .map(|&word| {
            if word.0 == "mnie" {
                ("mnie", 3_917_760, 3_923_520, None)
            } else {
                word
            }
        })
        .collect::<Vec<_>>();
    for (case, words) in [
        ("missing right neighbour", without_right.to_vec()),
        ("two held words in the gap", without_bo),
        ("no original PCM overlap", shifted),
    ] {
        let (ledger, owner, before, _) =
            s5_after_dt(&format!("s5-gap-{}", case.replace(' ', "-")), &words);
        assert_nie_retained(&ledger, &owner, &retained(&before, "nie"), case);
    }

    let (mut ledger, owner) = s5_fixture("s5-gap-nonfinal");
    let before = ledger
        .slots_of(&owner)
        .expect("the S5 owner keeps its slots")
        .to_vec();
    let (dt, pins) = stage_dt(&mut ledger, &owner, S5_DT_WORDS);
    let mut input = ledger
        .word_evidence_input(&dt)
        .expect("the staged DT evidence is retained")
        .clone();
    input.finality = "volatile".into();
    ledger.restore_word_evidence(input);
    ledger.admit_word_slots(&dt, &pins);
    assert_nie_retained(
        &ledger,
        &owner,
        &retained(&before, "nie"),
        "volatile native evidence",
    );
}

/// The commit exemption honours only the latest accepted, lexically resolved
/// decision for this observation, this exact target and this exact output.
#[test]
fn recorded_s5_commit_exemption_binds_the_latest_exact_word_decision() {
    let (mut ledger, _, before, dt) = s5_after_dt("s5-commit-guard", S5_DT_WORDS);
    let nie = retained(&before, "nie");
    let target = SlotTarget::from(&nie);
    let operation = ledger
        .slot_operations()
        .iter()
        .find(|operation| {
            operation.observation == dt
                && operation.kind == SlotOperationKind::Correct
                && operation
                    .sources
                    .iter()
                    .map(SlotTarget::from)
                    .collect::<Vec<_>>()
                    == [target.clone()]
                && operation.outputs.iter().any(|slot| slot.text == "mnie")
        })
        .cloned()
        .expect("the committed nie -> mnie correction");
    assert!(ledger.resolved_independent_word_operation(&operation));

    let mut relabelled = operation.clone();
    relabelled.outputs[0].text = "nic".into();
    let mut moved = operation.clone();
    moved.outputs[0].sample_end -= 960;
    let mut foreign = operation.clone();
    foreign.observation.generation += 1;
    foreign.outputs[0].observation = foreign.observation.clone();
    let mut multiword = operation.clone();
    multiword.sources.push(retained(&before, "stać,"));
    let mut lexicon = operation.clone();
    lexicon.sources[0].producer = ObservationProducer::Lexicon;
    let mut merge = operation.clone();
    merge.kind = SlotOperationKind::Merge;
    for (case, candidate) in [
        ("other output label", &relabelled),
        ("other output geometry", &moved),
        ("other observation", &foreign),
        ("multiword source", &multiword),
        ("lexicon source", &lexicon),
        ("not a correction", &merge),
    ] {
        assert!(
            !ledger.resolved_independent_word_operation(candidate),
            "{case}"
        );
    }

    let latest = ledger
        .word_adjudication
        .choices
        .iter()
        .rev()
        .find(|choice| choice.observation == dt && choice.targets == [target.clone()])
        .cloned()
        .expect("DT left a decision for the exact nie target");
    let mut refusal = latest.clone();
    refusal.accepted = false;
    ledger.word_adjudication.choices.push(refusal);
    assert!(
        !ledger.resolved_independent_word_operation(&operation),
        "a later refusal defeats the earlier acceptance"
    );
    ledger.word_adjudication.choices.pop();
    let mut unresolved = latest;
    unresolved.lexical_resolved = false;
    ledger.word_adjudication.choices.push(unresolved);
    assert!(
        !ledger.resolved_independent_word_operation(&operation),
        "eligibility without lexical resolution cannot open the guard"
    );
    ledger
        .word_adjudication
        .choices
        .retain(|choice| choice.targets != [target.clone()]);
    assert!(
        !ledger.resolved_independent_word_operation(&operation),
        "no decision, no exemption"
    );
}

fn latest_choice<'a>(
    ledger: &'a AcousticLedger,
    dt: &ObservationIdentity,
    target: &SlotTarget,
) -> &'a word_adjudication::WordChoiceReceipt {
    ledger
        .word_choices()
        .iter()
        .rev()
        .find(|choice| &choice.observation == dt && choice.targets == [target.clone()])
        .unwrap_or_else(|| panic!("DT left no decision for {target:?}"))
}

/// Distinct decode windows of complete Whisper hypotheses in a decision's
/// recorded support that hear `label` for exactly `target`.
fn whisper_windows(
    choice: &word_adjudication::WordChoiceReceipt,
    target: &SlotTarget,
    label: &str,
) -> BTreeSet<(u64, u64)> {
    choice
        .support
        .iter()
        .filter(|hypothesis| {
            hypothesis.observation.producer == ObservationProducer::Whisper
                && hypothesis.complete
                && hypothesis.acoustic_boundaries_complete
                && hypothesis.targets == [target.clone()]
                && hypothesis.original_text.as_deref() == Some(label)
        })
        .filter_map(|hypothesis| hypothesis.decode)
        .collect()
}

/// Decode windows of Whisper support for exactly `target`, split by whether
/// the window published the word or only heard it as context.
fn window_provenance(
    choice: &word_adjudication::WordChoiceReceipt,
    target: &SlotTarget,
    context_only: bool,
) -> BTreeSet<(u64, u64)> {
    choice
        .support
        .iter()
        .filter(|hypothesis| {
            hypothesis.observation.producer == ObservationProducer::Whisper
                && hypothesis.targets == [target.clone()]
                && hypothesis.publication_context_only == context_only
        })
        .filter_map(|hypothesis| hypothesis.decode)
        .collect()
}

/// Acceptance on one ledger, as Lena asked: the same recorded batch corrects
/// "nie" to "mnie" and keeps "Frontier". The refusal is the decision recorded
/// in `word_choices`, not only a commit that did not happen, and it rests on
/// Whisper windows the ledger actually holds for that word. Under v41 that
/// refusal resolves the lexical question in favour of the retained word.
///
/// Request 25 heard "Frontier" too, but it published before request 26 put
/// the word into this owner; context is kept only for words already held.
/// The windows are therefore requests 26 (the word's own decode) and 27.
#[test]
fn recorded_s5_one_batch_corrects_nie_and_keeps_frontier_on_two_windows() {
    let (ledger, owner, before, dt) = s5_after_dt("s5-v40-acceptance", S5_DT_WORDS);
    let frontier = retained(&before, "Frontier");
    let target = SlotTarget::from(&frontier);
    let choice = latest_choice(&ledger, &dt, &target);
    assert_eq!(
        (
            choice.candidate.surface.as_str(),
            choice.accepted,
            choice.lexical_resolved,
            choice.reason.as_str(),
        ),
        ("front", false, true, "whisper_consensus_retained"),
        "{choice:#?}"
    );
    assert_eq!(choice.selected_label, "Frontier");
    assert!(
        !ledger
            .word_choices()
            .iter()
            .any(|choice| choice.observation == dt
                && choice.targets == [target.clone()]
                && choice.accepted),
        "no DT decision for Frontier may be accepted"
    );
    let windows = whisper_windows(choice, &target, "Frontier");
    assert_eq!(
        windows,
        BTreeSet::from([S5_WHISPER[1].decode, S5_WHISPER[2].decode]),
        "support: {:#?}",
        choice.support
    );
    let requests = choice
        .support
        .iter()
        .filter(|hypothesis| {
            hypothesis
                .decode
                .is_some_and(|decode| windows.contains(&decode))
        })
        .filter_map(|hypothesis| {
            hypothesis
                .producer_request
                .as_ref()
                .map(|request| request.request_id)
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        requests,
        BTreeSet::from([S5_WHISPER[1].request, S5_WHISPER[2].request]),
        "each window comes from its own decoder request"
    );
    assert_eq!(
        (
            window_provenance(choice, &target, false),
            window_provenance(choice, &target, true),
        ),
        (
            BTreeSet::from([S5_WHISPER[1].decode]),
            BTreeSet::from([S5_WHISPER[2].decode]),
        ),
        "request 26 published Frontier; request 27 heard it only as context"
    );

    let held = held_at(&ledger, &owner, &frontier);
    assert_eq!(
        (held.text.as_str(), held.producer),
        ("Frontier", ObservationProducer::Whisper)
    );
    let nie = retained(&before, "nie");
    let mnie = held_at(&ledger, &owner, &nie);
    assert_eq!(
        (mnie.text.as_str(), mnie.producer),
        ("mnie", ObservationProducer::AppleDictation)
    );
    let label = committed_label(&ledger, &owner);
    assert!(
        label.contains("Frontier") && label.contains("bo mnie stać"),
        "{label:?}"
    );
    assert_eq!(ledger.conservation().residue(), 0);
}

/// Lena's open question on v40: a refused lone label must close the dispute,
/// not leave it open. A conflicted component keeps `word_trials_pending`
/// true, so the seal is refused as `FrontierOpen` until an acoustic trial
/// runs, keeps `word_labels_settled` false, so no settled formatter runs, and
/// `word_finality`, which the seal carries, reports the word as unresolved.
#[test]
fn recorded_s5_retained_frontier_is_settled_not_left_in_dispute() {
    let (ledger, owner, before, _) = s5_after_dt("s5-v40-finality", S5_DT_WORDS);
    let target = SlotTarget::from(&retained(&before, "Frontier"));
    let finality = ledger.word_finality(&owner);
    let unresolved = finality
        .iter()
        .filter(|word| word.unresolved)
        .map(|word| (word.selected.surface.clone(), word.targets.clone()))
        .collect::<Vec<_>>();
    let frontier = finality
        .iter()
        .find(|word| word.targets == [target.clone()])
        .expect("the seal must carry a finality row for Frontier");
    assert_eq!(
        (
            frontier.unresolved,
            ledger.word_trials_pending(&owner),
            ledger.word_labels_settled(&owner),
        ),
        (false, false, true),
        "(Frontier unresolved, trials pending, labels settled); unresolved rows: {unresolved:#?}"
    );
}

/// A later Whisper frame staged for the S5 owner after the recorded batches,
/// before the recorded DT batch. Its words are request 27's publication words
/// that fit the frame's decode window.
struct ContextFrame {
    case: &'static str,
    decode: (u64, u64),
    band: (u64, u64),
    finality: &'static str,
    exact_timing: bool,
    /// This word carries request 27's decode window instead of the frame's.
    foreign_decode_word: Option<&'static str>,
    /// This word was rewritten by the live lexicon.
    rewritten_word: Option<&'static str>,
    second_window: bool,
}

/// One more Whisper frame, distinct from the recorded three.
const DISTINCT_DECODE: (u64, u64) = (3_792_000, 4_224_000);
const DISTINCT_BAND: (u64, u64) = (4_032_000, 4_224_000);

fn record_context_frame(
    ledger: &mut AcousticLedger,
    owner: &OccurrenceIdentity,
    frame: &ContextFrame,
) {
    capture_view(
        ledger,
        owner,
        S5_DT_SPEECH_OBSERVED,
        S5_DT_SPEECH,
        S5_DT_ANCHOR_OBSERVED,
    );
    let observation = ObservationIdentity::new(
        ObservationProducer::Whisper,
        S5_WHISPER_REQUEST,
        3,
        owner.clone(),
    );
    let words = S5_WHISPER[2]
        .publication
        .iter()
        .copied()
        .filter(|word| frame.decode.0 <= word.1 && word.2 <= frame.decode.1)
        .collect::<Vec<_>>();
    let mut publication = pins_of(&words, frame.decode);
    for pin in &mut publication {
        if frame.foreign_decode_word == Some(pin.text.as_str()) {
            *pin = pin
                .clone()
                .with_decode_window(S5_WHISPER[2].decode.0, S5_WHISPER[2].decode.1);
        }
        if frame.rewritten_word == Some(pin.text.as_str()) {
            pin.surface_rewritten = true;
        }
    }
    let selected = publication
        .iter()
        .filter(|pin| frame.band.0 <= pin.sample_start && pin.sample_end <= frame.band.1)
        .cloned()
        .collect::<Vec<_>>();
    ledger.stage_word_evidence(
        &observation,
        &selected,
        Some("inprocess:whisper"),
        frame.finality,
    );
    ledger.stage_word_publication(&observation, frame.band, false, &publication);
    let mut input = ledger
        .word_evidence_input(&observation)
        .expect("staged context frame")
        .clone();
    input.producer_request = Some(TailRequestIdentity {
        request_id: 28,
        range: sample_range(owner, frame.decode.0, frame.decode.1),
    });
    if frame.exact_timing {
        input.timing = "exact_sample_range".into();
    }
    ledger.restore_word_evidence(input);
    // The reducer's order: the frame's decode bounds, then its context.
    ledger.record_word_decode_bounds(&observation, &selected);
    ledger.record_whisper_publication_context(&observation);
}

/// Which later frame is a second Whisper window for the held "nie". Only a
/// distinct, clean, final, exactly timed decode is; then the lone DT "mnie"
/// is refused as consensus, which is the positive control that makes every
/// other row mean something. A replay of request 27's own decode window
/// under a later band, a frame with one word from another decode, a volatile
/// frame, a frame without exact timing, a rewritten "nie" and a band that
/// leaves its decode cast no vote, and "mnie" still corrects "nie".
#[test]
fn recorded_s5_only_a_distinct_clean_final_window_is_a_second_witness() {
    let clean = ContextFrame {
        case: "distinct clean window",
        decode: DISTINCT_DECODE,
        band: DISTINCT_BAND,
        finality: "final",
        exact_timing: true,
        foreign_decode_word: None,
        rewritten_word: None,
        second_window: true,
    };
    let frames = [
        ContextFrame {
            case: "replay of request 27's window",
            decode: S5_WHISPER[2].decode,
            band: (4_032_000, S5_WHISPER[2].decode.1),
            second_window: false,
            ..clean
        },
        ContextFrame {
            case: "one word from another decode",
            foreign_decode_word: Some("stronie"),
            second_window: false,
            ..clean
        },
        ContextFrame {
            case: "volatile frame",
            finality: "volatile",
            second_window: false,
            ..clean
        },
        ContextFrame {
            case: "no exact timing",
            exact_timing: false,
            second_window: false,
            ..clean
        },
        ContextFrame {
            case: "rewritten nie",
            rewritten_word: Some("nie"),
            second_window: false,
            ..clean
        },
        ContextFrame {
            case: "band leaves the decode",
            band: (DISTINCT_BAND.0, DISTINCT_DECODE.1 + 48_000),
            second_window: false,
            ..clean
        },
        clean,
    ];
    let mut outcomes = Vec::new();
    let mut support = Vec::new();
    let mut retained_by_control = None;
    for frame in &frames {
        let (mut ledger, owner) =
            s5_fixture(&format!("s5-context-{}", frame.case.replace(' ', "-")));
        let before = ledger
            .slots_of(&owner)
            .expect("the S5 owner keeps its slots")
            .to_vec();
        record_context_frame(&mut ledger, &owner, frame);
        let (dt, pins) = stage_dt(&mut ledger, &owner, S5_DT_WORDS);
        ledger.admit_word_slots(&dt, &pins);
        let nie = retained(&before, "nie");
        let target = SlotTarget::from(&nie);
        let choice = latest_choice(&ledger, &dt, &target);
        let windows = whisper_windows(choice, &target, "nie").len();
        let held = held_at(&ledger, &owner, &nie);
        outcomes.push((
            frame.case,
            windows,
            choice.accepted,
            choice.reason == "whisper_consensus_retained",
            held.text.clone(),
        ));
        for hypothesis in &choice.support {
            support.push((
                frame.case,
                hypothesis.observation.producer,
                hypothesis.decode,
                hypothesis.original_text.clone(),
                hypothesis.complete,
                hypothesis.acoustic_boundaries_complete,
                hypothesis.targets == [target.clone()],
            ));
        }
        if frame.second_window {
            retained_by_control = Some((ledger, owner, nie));
        }
    }
    let expected = frames
        .iter()
        .map(|frame| {
            if frame.second_window {
                (frame.case, 2_usize, false, true, "nie".to_string())
            } else {
                (frame.case, 1_usize, true, false, "mnie".to_string())
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes, expected,
        "(case, windows, accepted, consensus, held); support per case: {support:#?}"
    );
    let (ledger, owner, nie) = retained_by_control.expect("the positive control ran");
    assert_nie_retained(&ledger, &owner, &nie, "distinct clean window");
}

/// v40 takes a true from either side of `same_anchored_word` before a false.
/// Under v39 a live false recorded for the first argument hid a live true
/// recorded for the second, so the answer depended on argument order. Over
/// every word the recorded take held, retired or proposed, the relation must
/// not depend on order.
#[test]
fn recorded_s5_same_anchored_word_does_not_depend_on_argument_order() {
    let (ledger, owner, before, _) = s5_after_dt("s5-v40-symmetry", S5_DT_WORDS);
    let mut words: Vec<WordSlot> = Vec::new();
    let mut add = |slot: &WordSlot| {
        if !words.contains(slot) {
            words.push(slot.clone());
        }
    };
    before.iter().for_each(&mut add);
    ledger
        .slots_of(&owner)
        .expect("the S5 owner keeps its slots")
        .iter()
        .for_each(&mut add);
    for operation in ledger.slot_operations() {
        operation.sources.iter().for_each(&mut add);
        operation.outputs.iter().for_each(&mut add);
    }
    for alternative in &ledger.slot_alternatives {
        alternative.sources.iter().for_each(&mut add);
    }
    let mut asymmetric = Vec::new();
    let mut same = 0;
    for (index, left) in words.iter().enumerate() {
        for right in &words[index + 1..] {
            let forward = ledger.same_anchored_word(left, right);
            let reverse = ledger.same_anchored_word(right, left);
            same += usize::from(forward && reverse);
            if forward != reverse {
                asymmetric.push((
                    (
                        left.text.clone(),
                        left.observation.generation,
                        left.sample_start,
                    ),
                    (
                        right.text.clone(),
                        right.observation.generation,
                        right.sample_start,
                    ),
                    forward,
                ));
            }
        }
    }
    assert!(words.len() > S5_SOURCE_SLOTS.len(), "{} words", words.len());
    assert!(same > 0, "the take holds words that are the same speech");
    assert!(
        asymmetric.is_empty(),
        "{} words; order-dependent pairs: {asymmetric:#?}",
        words.len()
    );
}

/// v41: retaining consensus resolves the lexical question without accepting
/// the replacement, and a resolved refusal is still a refusal. DT recorded no
/// operation on Frontier, and a "front" correction of Frontier built like the
/// committed "nie" -> "mnie" one cannot pass the commit guard. The control
/// accepts the same decision and opens the guard for that exact operation,
/// so the refusal closes it, not a malformed operation.
#[test]
fn recorded_s5_retained_consensus_resolves_but_opens_no_commit_guard() {
    let (mut ledger, owner, before, dt) = s5_after_dt("s5-v41-guard", S5_DT_WORDS);
    let frontier = retained(&before, "Frontier");
    let target = SlotTarget::from(&frontier);
    let decision = latest_choice(&ledger, &dt, &target).clone();
    assert_eq!(
        (
            decision.accepted,
            decision.lexical_resolved,
            decision.selected_label.as_str(),
            decision.candidate.surface.as_str(),
        ),
        (false, true, "Frontier", "front")
    );
    assert!(
        !ledger.slot_operations().iter().any(|operation| {
            operation.observation == dt
                && operation
                    .sources
                    .iter()
                    .any(|source| SlotTarget::from(source) == target)
        }),
        "DT recorded no operation on Frontier"
    );
    let correction = ledger
        .slot_operations()
        .iter()
        .find(|operation| {
            operation.observation == dt
                && operation.kind == SlotOperationKind::Correct
                && operation.outputs.iter().any(|slot| slot.text == "mnie")
        })
        .cloned()
        .expect("the committed nie -> mnie correction");
    let mut front = correction;
    front.sources = vec![frontier.clone()];
    front.outputs[0].sample_start = frontier.sample_start;
    front.outputs[0].sample_end = frontier.sample_end;
    front.outputs[0].text = decision.candidate.surface.clone();
    front.source_ranges = vec![OccurrenceIdentity::new(
        &owner.session,
        owner.capture_epoch,
        frontier.sample_start,
        frontier.sample_end,
    )];
    assert!(
        !ledger.resolved_independent_word_operation(&front),
        "the recorded refusal keeps the guard closed"
    );

    let mut accepted = decision.clone();
    accepted.accepted = true;
    accepted.selected_label = accepted.candidate.surface.clone();
    ledger.word_adjudication.choices.push(accepted);
    assert!(
        ledger.resolved_independent_word_operation(&front),
        "control: an accepted decision opens the guard for this exact operation"
    );
    ledger.word_adjudication.choices.pop();

    let mut relabelled = decision;
    relabelled.selected_label = relabelled.candidate.surface.clone();
    ledger.word_adjudication.choices.push(relabelled);
    assert!(
        !ledger.resolved_independent_word_operation(&front),
        "accepted=false keeps the guard closed although the decision is resolved"
    );
}

/// v41: a context window corroborates retention but never becomes a
/// dictionary witness. The distinct clean frame that keeps "nie" against the
/// lone DT "mnie" leaves the dictionary witnesses of "nie" exactly as request
/// 27 alone gave them, although the same window does reach the decision as
/// the second Whisper window. Under v40 such a window counted, which is how
/// "szew" became "Szwy".
#[test]
fn recorded_s5_context_window_never_becomes_a_dictionary_witness() {
    let (mut ledger, owner) = s5_fixture("s5-v41-dictionary");
    let before = ledger
        .slots_of(&owner)
        .expect("the S5 owner keeps its slots")
        .to_vec();
    let nie = retained(&before, "nie");
    let target = SlotTarget::from(&nie);
    let witnesses = ledger.dictionary_witnesses(&owner, std::slice::from_ref(&target));
    assert_eq!(witnesses, vec![(S5_WHISPER[2].decode, "nie".to_string())]);
    record_context_frame(
        &mut ledger,
        &owner,
        &ContextFrame {
            case: "distinct clean window",
            decode: DISTINCT_DECODE,
            band: DISTINCT_BAND,
            finality: "final",
            exact_timing: true,
            foreign_decode_word: None,
            rewritten_word: None,
            second_window: true,
        },
    );
    assert_eq!(
        ledger.dictionary_witnesses(&owner, std::slice::from_ref(&target)),
        witnesses,
        "a context window is not a dictionary witness"
    );
    let (dt, pins) = stage_dt(&mut ledger, &owner, S5_DT_WORDS);
    ledger.admit_word_slots(&dt, &pins);
    let choice = latest_choice(&ledger, &dt, &target);
    assert_eq!(
        (
            whisper_windows(choice, &target, "nie"),
            choice.accepted,
            choice.reason.as_str(),
        ),
        (
            BTreeSet::from([S5_WHISPER[2].decode, DISTINCT_DECODE]),
            false,
            "whisper_consensus_retained",
        ),
        "the same window corroborates retention: {:#?}",
        choice.support
    );
}

/// v41 order on one PCM frame. Request 26 published Frontier. A later frame
/// replaying request 26's decode window, under a band that leaves Frontier to
/// context, is the same audio: it adds no window and cannot downgrade the
/// published one to context, so Frontier keeps its dictionary witness. The
/// control shifts that frame's decode by 10 ms; the same words then reach
/// Frontier as a new context-only window, so the replay did address it.
#[test]
fn recorded_s5_replayed_publication_window_keeps_its_publication_rights() {
    let replay = ContextFrame {
        case: "replay of request 26's window",
        decode: S5_WHISPER[1].decode,
        band: (S5_WHISPER[2].band.0, S5_WHISPER[1].decode.1),
        finality: "final",
        exact_timing: true,
        foreign_decode_word: None,
        rewritten_word: None,
        second_window: false,
    };
    let shifted = ContextFrame {
        case: "request 26's window shifted by 10 ms",
        decode: (S5_WHISPER[1].decode.0 + 480, S5_WHISPER[1].decode.1),
        ..replay
    };
    let mut outcomes = Vec::new();
    for frame in [&replay, &shifted] {
        let (mut ledger, owner) = s5_fixture(&format!("s5-v41-order-{}", frame.decode.0));
        let before = ledger
            .slots_of(&owner)
            .expect("the S5 owner keeps its slots")
            .to_vec();
        let target = SlotTarget::from(&retained(&before, "Frontier"));
        record_context_frame(&mut ledger, &owner, frame);
        let dictionary = ledger.dictionary_witnesses(&owner, std::slice::from_ref(&target));
        let (dt, pins) = stage_dt(&mut ledger, &owner, S5_DT_WORDS);
        ledger.admit_word_slots(&dt, &pins);
        let choice = latest_choice(&ledger, &dt, &target);
        outcomes.push((
            frame.case,
            window_provenance(choice, &target, false),
            window_provenance(choice, &target, true),
            dictionary,
            choice.reason.clone(),
        ));
    }
    let published = BTreeSet::from([S5_WHISPER[1].decode]);
    let witness = vec![(S5_WHISPER[1].decode, "Frontier".to_string())];
    assert_eq!(
        outcomes,
        vec![
            (
                replay.case,
                published.clone(),
                BTreeSet::from([S5_WHISPER[2].decode]),
                witness.clone(),
                "whisper_consensus_retained".to_string(),
            ),
            (
                shifted.case,
                published,
                BTreeSet::from([shifted.decode, S5_WHISPER[2].decode]),
                witness,
                "whisper_consensus_retained".to_string(),
            ),
        ],
        "(case, published windows, context windows, dictionary, reason)"
    );
}
