//! Integrator-owned counterexamples on real microphone PCM: word identity is
//! the physical speech under a word, not the sample range a producer claimed.
//!
//! Fixtures: `tests/fixtures/real_take_word_identity_manifest.json` (both
//! producers' claimed ranges, measured bursts, origin). Recorded 2026-10-13 on
//! dragon, Codescribe build 953, EarPods, local_power + local_tail_patch.
//!
//! No decoder, VAD model, seal or delivery runs here. Only the ledger corridor
//! that turned these takes into "jest w tej Jest w tej" and "pumpy to".
use super::word_adjudication_tests::measured_ledger_at_rate;
use super::*;

const RATE: u32 = 48_000;

fn take_pcm(file: &str) -> Vec<f32> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(file);
    hound::WavReader::open(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / 32_768.0)
        .collect()
}

fn pins(words: &[(&str, u64, u64)]) -> Vec<WordPin> {
    words
        .iter()
        .map(|&(text, start, end)| WordPin::new(start, end, text))
        .collect()
}

fn decoded(words: &[(&str, u64, u64)], decode: (u64, u64)) -> Vec<WordPin> {
    pins(words)
        .into_iter()
        .map(|pin| pin.with_decode_window(decode.0, decode.1))
        .collect()
}

fn lowercase_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| {
            token
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

/// RMS of one 20 ms hop in dBFS.
fn hop_dbfs(frame: &[f32]) -> f64 {
    let rms =
        (frame.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / frame.len() as f64).sqrt();
    if rms > 0.0 {
        20.0 * rms.log10()
    } else {
        -120.0
    }
}

/// Noise floor of a take: the 10th percentile of its 20 ms hop levels. The
/// threshold for speech is the floor plus 8 dB — a fixed −50 dBFS would call
/// most of a −52 dBFS RMS EarPods take silence (the 146 s take of 13 X had its
/// whole voice between −60 and −50 dBFS).
fn speech_floor_dbfs(pcm: &[f32]) -> f64 {
    let hop = (RATE / 50) as usize;
    let mut levels = pcm.chunks_exact(hop).map(hop_dbfs).collect::<Vec<_>>();
    levels.sort_by(|a, b| a.total_cmp(b));
    levels[levels.len() / 10]
}

/// Speech bursts of the microphone inside `[start, end)`: RMS per 20 ms hop
/// above the take's floor + 8 dB, gaps under 60 ms merged, bursts of at least
/// 60 ms kept. No model, no producer timestamps.
fn speech_bursts(pcm: &[f32], start: u64, end: u64) -> Vec<(u64, u64)> {
    let hop = (RATE / 50) as usize;
    let threshold = speech_floor_dbfs(pcm) + 8.0;
    let mut bursts: Vec<(u64, u64)> = Vec::new();
    let mut open: Option<(u64, u64)> = None;
    let mut at = start as usize;
    while at + hop <= end as usize {
        let dbfs = hop_dbfs(&pcm[at..at + hop]);
        let hop_end = (at + hop) as u64;
        if dbfs > threshold {
            open = Some(open.map_or((at as u64, hop_end), |(s, _)| (s, hop_end)));
        } else if let Some((s, e)) = open
            && hop_end.saturating_sub(e) > (RATE / 1000 * 60).into()
        {
            bursts.push((s, e));
            open = None;
        }
        at += hop;
    }
    bursts.extend(open);
    bursts.retain(|(s, e)| e - s >= (RATE / 1000 * 60).into());
    bursts
}

fn has_speech(pcm: &[f32], start: u64, end: u64) -> bool {
    !speech_bursts(pcm, start, end).is_empty()
}

// ---------------------------------------------------------------------------
// Take 018e2177: "jest w tej chwili dopięty", spoken once, 5.2–6.9 s.
// Apple claims "Jest" at 5.93–6.05 s; Whisper claims "jest" at 5.22–5.44 s.
// The microphone has one burst for "jest" (4.8–5.54 s); Apple's claim lies on
// the next burst, the one "w tej chwili" is spoken in. Build 953 delivered
// "jest w tej Jest w tej chwili dopięty".
// ---------------------------------------------------------------------------

const SHORT_TAKE: &str = "take_018e2177_apple_whisper_offset.wav";
const SHORT_OWNER: (u64, u64) = (249_120, 370_560);
const SHORT_WHISPER_DECODE: (u64, u64) = (144_000, 575_488);
const SHORT_APPLE: [(&str, u64, u64); 5] = [
    ("Jest", 284_640, 290_400),
    ("w", 290_400, 291_840),
    ("tej", 291_840, 296_160),
    ("chwili", 296_160, 303_360),
    ("dopięty", 303_360, 337_920),
];
const SHORT_WHISPER: [(&str, u64, u64); 5] = [
    ("jest", 250_560, 261_120),
    ("w", 261_120, 276_480),
    ("tej", 276_480, 284_160),
    ("chwili", 284_160, 299_520),
    ("dopięty", 299_520, 328_320),
];

fn short_take_ledger() -> (AcousticLedger, OccurrenceIdentity, Vec<f32>) {
    let pcm = take_pcm(SHORT_TAKE);
    let owner = OccurrenceIdentity::new("take-018e2177", 1, SHORT_OWNER.0, SHORT_OWNER.1);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, RATE);
    ledger.schedule_frontier(
        owner.clone(),
        [ObservationProducer::Apple, ObservationProducer::Whisper],
    );
    (ledger, owner, pcm)
}

#[test]
fn fixture_018e2177_microphone_has_one_jest_and_apples_claim_lies_on_the_next_burst() {
    // Positive control for the measurement itself, independent of the ledger.
    let pcm = take_pcm(SHORT_TAKE);
    let floor = speech_floor_dbfs(&pcm);
    assert!(
        (-66.0..=-58.0).contains(&floor),
        "EarPods floor of this take: {floor}"
    );
    let bursts = speech_bursts(&pcm, 240_000, 374_400);
    assert_eq!(
        bursts.len(),
        4,
        "four bursts between 5.0 and 7.8 s: jest | w tej chwili | dopię | ty: {bursts:?}"
    );
    let (whisper_jest, apple_jest) = (SHORT_WHISPER[0], SHORT_APPLE[0]);
    let jest_burst = bursts[0];
    assert!(whisper_jest.1 < jest_burst.1 && whisper_jest.2 > jest_burst.0);
    assert!(
        apple_jest.1 >= jest_burst.1,
        "Apple's 'Jest' at 5.93–6.05 s starts after the only 'jest' burst ends"
    );
    let under_apple = speech_bursts(&pcm, apple_jest.1, apple_jest.2);
    assert!(
        under_apple.len() == 1 && under_apple[0].0 == apple_jest.1,
        "and lies inside the burst of 'w tej chwili', not on a burst of its own: {under_apple:?}"
    );
}

#[test]
fn real_take_018e2177_apple_then_whisper_name_the_same_jest_once() {
    // The live order on build 953: Apple's occurrence closed first, the Stop
    // window of Whisper corrected it 0.4 s later.
    let (mut ledger, owner, _pcm) = short_take_ledger();
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, owner.clone());
    assert!(
        ledger
            .admit_word_slots(&apple, &pins(&SHORT_APPLE))
            .grants_mutation()
    );
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 1, 10_001, owner.clone());
    ledger.admit_word_slots(&whisper, &decoded(&SHORT_WHISPER, SHORT_WHISPER_DECODE));

    let text = ledger.text_of(&owner).expect("occurrence keeps a label");
    assert_eq!(
        lowercase_tokens(text),
        ["jest", "w", "tej", "chwili", "dopięty"],
        "one physical 'jest' is one word, whichever producer timed it: {text:?}"
    );
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 5);
    assert_eq!(ledger.conservation().residue(), 0);
}

#[test]
fn real_take_018e2177_whisper_then_apple_name_the_same_jest_once() {
    // Reverse order: Whisper held the words, Apple's final arrived 2.3 s after
    // Stop. Equal words on the same speech stay one word in both orders.
    let (mut ledger, owner, _pcm) = short_take_ledger();
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 1, 10_001, owner.clone());
    assert!(
        ledger
            .admit_word_slots(&whisper, &decoded(&SHORT_WHISPER, SHORT_WHISPER_DECODE))
            .grants_mutation()
    );
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, owner.clone());
    ledger.admit_word_slots(&apple, &pins(&SHORT_APPLE));

    let text = ledger.text_of(&owner).expect("occurrence keeps a label");
    assert_eq!(
        lowercase_tokens(text),
        ["jest", "w", "tej", "chwili", "dopięty"],
        "{text:?}"
    );
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 5);
}

#[test]
fn real_take_018e2177_a_late_pin_without_a_burst_of_its_own_cannot_add_a_word() {
    // Only Apple's displaced "Jest" arrives late, alone. Every burst under its
    // claimed range is already spoken for by admitted words ("tej", "chwili");
    // a pin without a burst of its own is not a second occurrence of anything.
    let (mut ledger, owner, pcm) = short_take_ledger();
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 1, 10_001, owner.clone());
    assert!(
        ledger
            .admit_word_slots(&whisper, &decoded(&SHORT_WHISPER, SHORT_WHISPER_DECODE))
            .grants_mutation()
    );
    let before = ledger.text_of(&owner).unwrap().to_owned();
    let (claim_start, claim_end) = (SHORT_APPLE[0].1, SHORT_APPLE[0].2);
    let owned = ledger.slots_of(&owner).unwrap();
    for (burst_start, burst_end) in speech_bursts(&pcm, claim_start, claim_end) {
        assert!(
            owned
                .iter()
                .any(|slot| slot.sample_start < burst_end && slot.sample_end > burst_start),
            "burst {burst_start}–{burst_end} under Apple's claim is already owned"
        );
    }
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 2, 0, owner.clone());
    ledger.admit_word_slots(&apple, &pins(&SHORT_APPLE[..1]));
    assert_eq!(ledger.text_of(&owner), Some(before.as_str()));
    assert_eq!(ledger.slots_of(&owner).unwrap().len(), 5);
}

#[test]
fn real_take_018e2177_every_admitted_word_sits_on_speech() {
    // The identity of a word is the speech under it. A producer may stretch a
    // short word over the pause beside it: Whisper's "w" at 5.44–5.76 s lies
    // mostly in the valley between "jest" and "w tej" and keeps speech only
    // because it overlaps the last 100 ms of the "jest" burst — the sharing
    // rule for short tokens. The ledger must never hold a word over a range
    // the microphone has no speech for at all; a word that shares a burst
    // with its neighbour is not two words.
    let (mut ledger, owner, pcm) = short_take_ledger();
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 1, 10_001, owner.clone());
    assert!(
        ledger
            .admit_word_slots(&whisper, &decoded(&SHORT_WHISPER, SHORT_WHISPER_DECODE))
            .grants_mutation()
    );
    let slots = ledger.slots_of(&owner).unwrap();
    assert_eq!(slots.len(), 5);
    let silent = slots
        .iter()
        .filter(|slot| !has_speech(&pcm, slot.sample_start, slot.sample_end))
        .map(|slot| (slot.text.clone(), slot.sample_start, slot.sample_end))
        .collect::<Vec<_>>();
    assert!(
        silent.is_empty(),
        "admitted words with no speech under their range: {silent:?}"
    );
}

// ---------------------------------------------------------------------------
// Take 36de5e98 (channel 3), tail: "... przy której się pracuje z computer use".
// Live window 6 was cut at 27.0 s inside the phrase — the microphone burst of
// "computer use" (26.30–27.02 s) runs through the decode fence — and returned
// "pumpy to use."; the occurrence closed with that label. The next window with
// complete fences (generation 7, the Stop window of this take) returned
// "Computer Use." over the same speech and was refused.
// Fixture samples = session samples − 864000.
// ---------------------------------------------------------------------------

const PUMPY_TAKE: &str = "take_36de5e98_stop_window_tail.wav";
const PUMPY_OWNER: (u64, u64) = (82_560, 466_560);
const PUMPY_LIVE_DECODE: (u64, u64) = (0, 432_000);
const PUMPY_STOP_DECODE: (u64, u64) = (288_000, 593_664);
const PUMPY_LIVE_WINDOW_6: [(&str, u64, u64); 13] = [
    ("przecież", 94_080, 117_120),
    ("nie", 117_120, 150_720),
    ("jest", 150_720, 166_080),
    ("to", 166_080, 177_600),
    ("pierwsza", 177_600, 195_840),
    ("aplikacja,", 195_840, 238_080),
    ("przy", 238_080, 283_200),
    ("której", 283_200, 301_440),
    ("się", 301_440, 314_880),
    ("pracuje", 314_880, 341_760),
    ("z", 341_760, 386_880),
    ("pumpy", 386_880, 410_880),
    ("to", 410_880, 416_640),
];
const PUMPY_APPLE: [(&str, u64, u64); 9] = [
    ("bo", 88_320, 95_520),
    ("przecież", 95_520, 124_320),
    ("nie", 141_600, 153_120),
    ("jest", 153_120, 168_960),
    ("to", 168_960, 180_480),
    ("pierwsza", 180_480, 197_760),
    ("aplikacja", 197_760, 233_760),
    ("przy", 275_520, 287_040),
    ("której", 287_040, 307_200),
];
const PUMPY_STOP_WINDOW: [(&str, u64, u64); 7] = [
    ("w", 288_000, 291_840),
    ("której", 291_840, 302_400),
    ("się", 302_400, 314_880),
    ("pracuje", 314_880, 343_680),
    ("z", 343_680, 385_920),
    ("Computer", 385_920, 412_800),
    ("Use.", 412_800, 444_480),
];

#[test]
fn fixture_36de5e98_the_speech_under_pumpy_to_runs_through_the_live_fence() {
    // Positive control for the measurement itself, independent of the ledger.
    let pcm = take_pcm(PUMPY_TAKE);
    let (pumpy, edge_to) = (PUMPY_LIVE_WINDOW_6[11], PUMPY_LIVE_WINDOW_6[12]);
    assert_eq!((pumpy.0, edge_to.0), ("pumpy", "to"));
    let fence = PUMPY_LIVE_DECODE.1;
    let last = *speech_bursts(&pcm, pumpy.1, fence + RATE as u64 / 2)
        .last()
        .expect("speech under the edge words");
    assert!(
        last.0 < edge_to.2 && last.1 >= fence,
        "the burst under 'pumpy to' is not closed by silence before the fence at 27.0 s: {last:?}"
    );
    let (computer, use_) = (PUMPY_STOP_WINDOW[5], PUMPY_STOP_WINDOW[6]);
    assert!(has_speech(&pcm, computer.1, computer.2), "computer");
    assert!(has_speech(&pcm, use_.1, use_.2), "use");
}

#[test]
fn real_take_36de5e98_an_edge_cut_word_is_held_live_and_owned_by_the_next_complete_window() {
    // Live-first: the correction never waits for Stop. A word whose speech runs
    // through the decode fence is not a word yet; its burst stays open and the
    // next window that covers it with complete fences owns it. Here that next
    // window happens to be the Stop window of the take; the rule is the same
    // for any later live window, and Stop only closes the last open burst.
    let pcm = take_pcm(PUMPY_TAKE);
    let owner = OccurrenceIdentity::new("take-36de5e98-tail", 1, PUMPY_OWNER.0, PUMPY_OWNER.1);
    let mut ledger = measured_ledger_at_rate(&owner, &pcm, RATE);
    ledger.schedule_frontier(
        owner.clone(),
        [ObservationProducer::Apple, ObservationProducer::Whisper],
    );
    // 1. Live window 6, decode [18.0, 27.0) s, right fence inside the phrase.
    let live = ObservationIdentity::new(ObservationProducer::Whisper, 5, 2, owner.clone());
    assert!(
        ledger
            .admit_word_slots(&live, &decoded(&PUMPY_LIVE_WINDOW_6, PUMPY_LIVE_DECODE))
            .grants_mutation()
    );
    let live_tokens = lowercase_tokens(ledger.text_of(&owner).unwrap());
    assert!(
        !live_tokens.iter().any(|token| token == "pumpy"),
        "an edge-cut word is held, not delivered (build 953 delivered 'pumpy to'): {live_tokens:?}"
    );
    assert!(
        live_tokens.ends_with(&["pracuje".into(), "z".into()]),
        "the words closed by silence before the fence are delivered live: {live_tokens:?}"
    );
    // 2. Apple's phrase, which never reached the disputed words.
    let apple = ObservationIdentity::new(ObservationProducer::Apple, 3, 0, owner.clone());
    ledger.admit_word_slots(&apple, &pins(&PUMPY_APPLE));
    // 3. The next window with complete fences: decode [24.0, 30.37) s, the
    //    right fence is the end of capture. It names the same speech.
    let next = ObservationIdentity::new(ObservationProducer::Whisper, 5, 7, owner.clone());
    ledger.admit_word_slots(&next, &decoded(&PUMPY_STOP_WINDOW, PUMPY_STOP_DECODE));

    let text = ledger.text_of(&owner).expect("occurrence keeps a label");
    let tokens = lowercase_tokens(text);
    assert!(
        tokens.ends_with(&["z".into(), "computer".into(), "use".into()]),
        "the window with complete fences owns the open burst: {text:?}"
    );
    assert!(
        !tokens.iter().any(|token| token == "pumpy"),
        "the edge-cut word must not survive beside its correction: {text:?}"
    );
    assert_eq!(ledger.conservation().residue(), 0);
}
