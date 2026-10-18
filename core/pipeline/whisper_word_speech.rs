//! A Whisper word is admitted only when its own sample span carries speech.
//!
//! The session measures Silero and the calibrated existence floor. This module
//! does not read PCM and does not turn a missing channel into silence.
//!
//! One rule needs no calibration: a word with no Silero speech anywhere within
//! [`WHISPER_WORD_SPEECH_NEAR_MS`] of its span was written over silence and is
//! refused, in the ledger lane and in the direct lane alike.

use super::acoustic_ledger::{RefuseReason, SilenceUnderSpan};

/// Shortest voiced span, in milliseconds, that can be one syllable of an
/// identical Whisper repeat.
///
/// Div0 corpus of 15–17 X (`whisper-dup-runs-15-17X-div0.txt`): one-syllable
/// loops that sit on silence or a pause are 60 ms ("no" ×4) and 74 ms
/// ("to" ×7). Repeats the same scan marks plausible start at 147 ms ("to" ×3),
/// 160 ms ("nie" ×6) and 180 ms ("i" ×3). 100 ms per syllable sits between
/// those two groups. A repeat that is separated by a valley or a Silero dip
/// and is at least this long stays. A shorter measured repeat is not its own
/// syllable. Digital zero and a span below the existence floor are refused by
/// [`RefuseReason::NoVoicedHopInPin`] before this floor is consulted.
///
/// The floor is not applied when speech evidence is unmeasured. The live
/// "Iwo Iwo Iwo Iwo Iwo" capture trace uses 3 000-sample pins at 16 kHz
/// (187 ms, two vowel groups, so 200 ms at this floor) on flat PCM and does
/// not bind this floor; those pins stay on today's path.
pub const WHISPER_REPEAT_MIN_MS_PER_SYLLABLE: u64 = 100;

/// How far from its own span a word that is not a repeat may find speech.
///
/// Polish speech decoded to English is a working use of Whisper: the
/// translated words do not line up one to one with the voiced words, and
/// their timestamps can land on a short pause inside the sentence. Such a
/// word keeps today's path with its own receipt. 500 ms is chosen, not
/// measured; it is wider than the 200 ms Silero merge gap and shorter than
/// the silences the corpus hallucinations sit on (47–612 s of digital zero).
///
/// The same radius bounds the Silero-only rule. Div0 census of 15–17 X
/// (231 takes, 21 080 Whisper words, `stasiu-silence/REPORT.md`): 689 words
/// have no Silero speech under their span; 303 of them lie within 500 ms of
/// speech (pin jitter is 0.1–0.3 s), 49 at 0.5–1 s and 337 further than 1 s.
/// Every such word but two is Whisper's, and most sit on sound without speech,
/// not on digital zero.
pub const WHISPER_WORD_SPEECH_NEAR_MS: u64 = 500;

/// Receipt label when neither Silero nor the calibrated floor was measured.
/// The word is kept. The string is the receipt; it is not a refusal.
pub const WHISPER_WORD_SPEECH_UNMEASURED: &str = "unmeasured";

/// Receipt label for a word whose own span is silent while speech lies within
/// [`WHISPER_WORD_SPEECH_NEAR_MS`]. The word is kept.
pub const WHISPER_WORD_SPEECH_NEAR: &str = "speech_near_span";

/// How far Silero places the nearest speech from one word's span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SileroSpeechDistance {
    /// Silero has not observed the span and the radius after it.
    Unmeasured,
    /// Silero observed the span and the radius after it and heard no speech
    /// anywhere in this capture epoch.
    NoSpeechHeard,
    /// Milliseconds from the span to the nearest speech range; 0 on overlap.
    Ms(u64),
}

impl SileroSpeechDistance {
    /// `true` only when Silero measured that no speech lies within the radius.
    fn beyond_near_radius(self) -> bool {
        match self {
            Self::Unmeasured => false,
            Self::NoSpeechHeard => true,
            Self::Ms(distance) => distance > WHISPER_WORD_SPEECH_NEAR_MS,
        }
    }

    /// Receipt value: the measured distance, `None` when nothing was heard.
    fn receipt_ms(self) -> Option<u32> {
        match self {
            Self::Ms(distance) => Some(u32::try_from(distance).unwrap_or(u32::MAX)),
            Self::Unmeasured | Self::NoSpeechHeard => None,
        }
    }
}

/// One Whisper word plus the speech facts already measured for its span.
#[derive(Debug, Clone, PartialEq)]
pub struct WhisperWordSpeech {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
    pub sample_rate: u32,
    /// `Some` only when Silero observed this span on the capture clock.
    /// `true` when a session-threshold speech range overlaps the span.
    pub silero_speech: Option<bool>,
    /// `Some` only when the calibrated existence floor was applied to this span.
    /// `true` when a hop inside the span is at or above that floor. `None`
    /// keeps the word unmeasured, whatever Silero says.
    pub energy_above_floor: Option<bool>,
    /// The same two channels on the span widened by
    /// [`WHISPER_WORD_SPEECH_NEAR_MS`] on each side. `true` when either finds
    /// speech there.
    pub speech_near_span: Option<bool>,
    /// Separation from the previous identical word, looked for in
    /// [`separation_window`].
    /// `Some(true)` when a measured channel shows an energy valley or a Silero
    /// dip. `Some(false)` when at least one channel was measured and none of
    /// them found that split. `None` when neither channel was measured.
    pub separated_from_previous: Option<bool>,
    /// Silero alone, with no energy channel: the nearest speech it heard.
    pub silero_speech_distance: SileroSpeechDistance,
    /// What the span itself holds, for the receipt of a word over silence.
    pub silence_under_span: SilenceUnderSpan,
}

/// What the judge did with one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperWordSpeechVerdict {
    /// Speech was measured on this span and the repeat rule passed.
    Admit,
    /// A channel the rule needs was not measured. The word keeps today's path.
    Unmeasured,
    /// Not a repeat; its own span is silent and speech lies near it. The
    /// word keeps today's path.
    NearSpeech,
    /// The span was measured and the word does not enter the document.
    Refuse(RefuseReason),
}

/// One word's verdict and the stable receipt string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperWordSpeechDecision {
    pub verdict: WhisperWordSpeechVerdict,
}

impl WhisperWordSpeechDecision {
    /// Kept words are admitted, near speech or explicitly unmeasured. A
    /// refusal is not.
    pub fn admits(self) -> bool {
        !matches!(self.verdict, WhisperWordSpeechVerdict::Refuse(_))
    }

    /// Stable receipt text. Unmeasured is a word, not a [`RefuseReason`].
    pub fn receipt(self) -> &'static str {
        match self.verdict {
            WhisperWordSpeechVerdict::Admit => "admitted",
            WhisperWordSpeechVerdict::Unmeasured => WHISPER_WORD_SPEECH_UNMEASURED,
            WhisperWordSpeechVerdict::NearSpeech => WHISPER_WORD_SPEECH_NEAR,
            WhisperWordSpeechVerdict::Refuse(reason) => reason.as_str(),
        }
    }
}

/// Judge every word of one Whisper window, in order.
///
/// The first word of an identical run is judged only on its own span. Each
/// later word also needs its own voiced segment: a measured valley or Silero
/// dip behind it, and at least [`WHISPER_REPEAT_MIN_MS_PER_SYLLABLE`] per
/// syllable. A missing measurement never becomes a refusal.
pub fn judge_whisper_words(words: &[WhisperWordSpeech]) -> Vec<WhisperWordSpeechDecision> {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| WhisperWordSpeechDecision {
            verdict: judge_one(words, index, word),
        })
        .collect()
}

fn judge_one(
    words: &[WhisperWordSpeech],
    index: usize,
    word: &WhisperWordSpeech,
) -> WhisperWordSpeechVerdict {
    // No speech anywhere near the word: Silero alone decides this, with or
    // without a calibration profile. Energy is not consulted, because sound
    // above the floor without speech is where most of these words sit.
    if word.silero_speech_distance.beyond_near_radius() {
        return WhisperWordSpeechVerdict::Refuse(RefuseReason::NoSpeechNearSpan {
            silence: word.silence_under_span,
            distance_ms: word.silero_speech_distance.receipt_ms(),
        });
    }
    // Without the calibrated floor Silero alone does not decide: it merges
    // gaps shorter than its merge window, so two real repeats inside one
    // range look like one word. A take without a profile keeps today's path.
    if word.energy_above_floor.is_none() {
        return WhisperWordSpeechVerdict::Unmeasured;
    }
    let repeat = is_identical_repeat(words, index);
    if !span_has_speech(word) {
        // A repeat needs its own voiced segment, so nearby speech does not
        // count for it. A loop copy beside the one real word stays refused.
        if !repeat && word.speech_near_span == Some(true) {
            return WhisperWordSpeechVerdict::NearSpeech;
        }
        return WhisperWordSpeechVerdict::Refuse(RefuseReason::NoVoicedHopInPin);
    }
    if !repeat {
        return WhisperWordSpeechVerdict::Admit;
    }
    // Energy above the floor is also what a loop on room noise has. A repeat
    // that Silero observed and did not hear is not its own voiced segment.
    if word.silero_speech == Some(false) {
        return WhisperWordSpeechVerdict::Refuse(
            RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment,
        );
    }
    match word.separated_from_previous {
        Some(false) => {
            WhisperWordSpeechVerdict::Refuse(RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment)
        }
        Some(true) => match span_meets_syllable_floor(word) {
            Some(true) => WhisperWordSpeechVerdict::Admit,
            Some(false) => WhisperWordSpeechVerdict::Refuse(
                RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment,
            ),
            None => WhisperWordSpeechVerdict::Unmeasured,
        },
        None => WhisperWordSpeechVerdict::Unmeasured,
    }
}

/// Speech is present when any measured channel says so. Every measured
/// channel saying no is silence.
fn span_has_speech(word: &WhisperWordSpeech) -> bool {
    word.silero_speech == Some(true) || word.energy_above_floor == Some(true)
}

fn is_identical_repeat(words: &[WhisperWordSpeech], index: usize) -> bool {
    let Some(key) = repeat_key(&words[index].text) else {
        return false;
    };
    index > 0 && repeat_key(&words[index - 1].text).as_deref() == Some(key.as_str())
}

/// One whitespace token, compared case-insensitively. A phrase is not a run.
pub(crate) fn repeat_key(text: &str) -> Option<String> {
    let mut tokens = text.split_whitespace();
    let word = tokens.next()?;
    if tokens.next().is_some() {
        return None;
    }
    Some(word.to_lowercase())
}

/// Where a split between two identical neighbours is looked for: from the
/// middle of the previous word to the middle of the current one.
///
/// Whisper word spans usually touch. In the truth files of 15–17 X, 13 925 of
/// 19 967 consecutive Whisper word pairs share a boundary and 1 011 overlap;
/// so do 535 of 803 identical neighbours. The open gap between two spans is
/// therefore usually empty, and the split has to be looked for across the
/// boundary. `None` when the current word does not start after the previous.
pub(crate) fn separation_window(previous: (u64, u64), current: (u64, u64)) -> Option<(u64, u64)> {
    let previous_mid = previous.0 + previous.1.saturating_sub(previous.0) / 2;
    let current_mid = current.0 + current.1.saturating_sub(current.0) / 2;
    (current_mid > previous_mid).then_some((previous_mid, current_mid))
}

/// `true` when the hops at or above the floor do not cover the whole window:
/// the energy fell below the floor somewhere between the two words.
pub(crate) fn window_has_energy_valley(window: (u64, u64), voiced: &[(u64, u64)]) -> bool {
    let covered = voiced
        .iter()
        .map(|&(start, end)| end.min(window.1).saturating_sub(start.max(window.0)))
        .sum::<u64>();
    covered < window.1.saturating_sub(window.0)
}

/// `true` when no single Silero speech range holds the whole window.
pub(crate) fn window_has_silero_dip(
    window: (u64, u64),
    mut speech_ranges: impl Iterator<Item = (u64, u64)>,
) -> bool {
    !speech_ranges.any(|(start, end)| start <= window.0 && window.1 <= end)
}

/// `Some` when the sample rate can turn the span into milliseconds.
fn span_meets_syllable_floor(word: &WhisperWordSpeech) -> Option<bool> {
    if word.sample_rate == 0 || word.sample_end <= word.sample_start {
        return None;
    }
    let syllables = u64::from(syllable_groups(&word.text).max(1));
    let milliseconds =
        word.sample_end.saturating_sub(word.sample_start) * 1_000 / u64::from(word.sample_rate);
    Some(milliseconds >= WHISPER_REPEAT_MIN_MS_PER_SYLLABLE.saturating_mul(syllables))
}

/// Vowel groups in lowercase. Polish `i`/`y` immediately before another vowel
/// is a glide and does not open a syllable: "nie" is 1, "Iwo" is 2,
/// "dziękuję" is 3.
fn syllable_groups(text: &str) -> u32 {
    let chars: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut count = 0u32;
    let mut index = 0;
    while index < chars.len() {
        if !is_vowel(chars[index]) {
            index += 1;
            continue;
        }
        let glide = matches!(chars[index], 'i' | 'y')
            && chars.get(index + 1).is_some_and(|next| is_vowel(*next));
        if glide {
            index += 1;
            continue;
        }
        count += 1;
        index += 1;
        while index < chars.len() && is_vowel(chars[index]) {
            index += 1;
        }
    }
    count
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'ą' | 'ę' | 'ó')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(
        text: &str,
        start: u64,
        end: u64,
        silero: Option<bool>,
        energy: Option<bool>,
        separated: Option<bool>,
    ) -> WhisperWordSpeech {
        WhisperWordSpeech {
            text: text.to_string(),
            sample_start: start,
            sample_end: end,
            sample_rate: 48_000,
            silero_speech: silero,
            energy_above_floor: energy,
            speech_near_span: None,
            separated_from_previous: separated,
            silero_speech_distance: SileroSpeechDistance::Unmeasured,
            silence_under_span: SilenceUnderSpan::EnergyUnmeasured,
        }
    }

    fn near_speech(mut word: WhisperWordSpeech) -> WhisperWordSpeech {
        word.speech_near_span = Some(true);
        word
    }

    fn silero_distance(
        mut word: WhisperWordSpeech,
        distance: SileroSpeechDistance,
        silence: SilenceUnderSpan,
    ) -> WhisperWordSpeech {
        word.silero_speech_distance = distance;
        word.silence_under_span = silence;
        word
    }

    #[test]
    fn a_word_far_from_any_silero_speech_is_refused_without_a_calibration_profile() {
        // Div0 cdff34c7 (17 X 03:41Z): "nie," 2 176 ms from the nearest speech,
        // on sound the existence floor calls voiced.
        let decisions = judge_whisper_words(&[silero_distance(
            word("nie,", 0, 9_600, Some(false), None, None),
            SileroSpeechDistance::Ms(2_176),
            SilenceUnderSpan::SoundWithoutSpeech,
        )]);
        assert_eq!(
            decisions[0].verdict,
            WhisperWordSpeechVerdict::Refuse(RefuseReason::NoSpeechNearSpan {
                silence: SilenceUnderSpan::SoundWithoutSpeech,
                distance_ms: Some(2_176),
            }),
            "Silero heard no speech within the radius; a missing floor must not keep a word \
             written over silence, and the receipt carries the class and the distance"
        );
        assert_eq!(decisions[0].receipt(), "no_speech_near_span");
    }

    #[test]
    fn energy_above_the_floor_does_not_keep_a_word_silero_heard_no_speech_near() {
        let decisions = judge_whisper_words(&[silero_distance(
            word("trzy", 0, 19_200, Some(false), Some(true), None),
            SileroSpeechDistance::Ms(6_680),
            SilenceUnderSpan::SoundWithoutSpeech,
        )]);
        assert!(
            !decisions[0].admits(),
            "room noise above the existence floor is sound, not speech"
        );
    }

    #[test]
    fn digital_zero_with_no_speech_in_the_epoch_is_refused_with_no_distance() {
        let decisions = judge_whisper_words(&[silero_distance(
            word("Dziękuję.", 0, 48_000, Some(false), Some(false), None),
            SileroSpeechDistance::NoSpeechHeard,
            SilenceUnderSpan::DigitalZero,
        )]);
        assert_eq!(
            decisions[0].verdict,
            WhisperWordSpeechVerdict::Refuse(RefuseReason::NoSpeechNearSpan {
                silence: SilenceUnderSpan::DigitalZero,
                distance_ms: None,
            }),
            "no speech heard in the epoch has no distance to report"
        );
    }

    #[test]
    fn speech_within_the_radius_leaves_the_word_to_the_existing_rules() {
        let edge = judge_whisper_words(&[silero_distance(
            near_speech(word("the", 0, 4_800, Some(false), Some(false), None)),
            SileroSpeechDistance::Ms(WHISPER_WORD_SPEECH_NEAR_MS),
            SilenceUnderSpan::BelowFloor,
        )]);
        assert_eq!(
            edge[0].receipt(),
            WHISPER_WORD_SPEECH_NEAR,
            "speech exactly at the radius is near; the word keeps its near-speech receipt"
        );
        let jitter = judge_whisper_words(&[silero_distance(
            word("Co?", 0, 4_800, Some(false), None, None),
            SileroSpeechDistance::Ms(64),
            SilenceUnderSpan::EnergyUnmeasured,
        )]);
        assert_eq!(
            jitter[0].receipt(),
            WHISPER_WORD_SPEECH_UNMEASURED,
            "a pin 64 ms off speech is Whisper's timing, not a word over silence"
        );
    }

    #[test]
    fn unmeasured_silero_never_refuses_a_word_as_over_silence() {
        let decisions = judge_whisper_words(&[word("nie", 0, 1_920, None, None, None)]);
        assert_eq!(
            decisions[0].receipt(),
            WHISPER_WORD_SPEECH_UNMEASURED,
            "without Silero on the span and its radius, nothing is known about silence"
        );
    }

    #[test]
    fn fourteen_nie_on_silence_after_one_voiced_span_keep_one() {
        // 40 ms at 48 kHz. The corpus loop is 20–60 ms per "nie".
        let span = 1_920u64;
        let mut words = vec![word("nie", 0, span, Some(true), Some(true), None)];
        for index in 1..14u64 {
            let start = index * span;
            words.push(word(
                "nie",
                start,
                start + span,
                Some(false),
                Some(false),
                Some(false),
            ));
        }
        let decisions = judge_whisper_words(&words);
        let admitted = decisions
            .iter()
            .filter(|decision| decision.admits())
            .count();
        let refusals: Vec<_> = decisions
            .iter()
            .filter(|decision| !decision.admits())
            .collect();
        assert_eq!(
            admitted, 1,
            "one voiced nie must stay; thirteen silence copies must not occupy word slots"
        );
        assert_eq!(
            refusals.len(),
            13,
            "each silent nie needs its own refusal receipt, not a silent drop"
        );
        assert!(
            refusals
                .iter()
                .all(|decision| decision.receipt() == RefuseReason::NoVoicedHopInPin.as_str()),
            "a span measured as silence is no voiced hop in that pin, not a string-equality drop"
        );
    }

    #[test]
    fn dziekuje_three_times_on_digital_zero_is_all_refused() {
        let words = (0..3u64)
            .map(|index| {
                let start = index * 48_000;
                word(
                    "Dziękuję",
                    start,
                    start + 48_000,
                    Some(false),
                    Some(false),
                    Some(false),
                )
            })
            .collect::<Vec<_>>();
        let decisions = judge_whisper_words(&words);
        assert!(
            decisions.iter().all(|decision| !decision.admits()),
            "Dziękuję on digital zero has no speech under any copy and must not enter the document"
        );
        assert_eq!(
            decisions.len(),
            3,
            "each digital-zero Dziękuję is receipted on its own"
        );
        assert!(
            decisions
                .iter()
                .all(|decision| { decision.receipt() == RefuseReason::NoVoicedHopInPin.as_str() }),
            "digital zero is measured absence of a voiced hop, including the first copy"
        );
    }

    #[test]
    fn five_iwo_on_separate_voiced_segments_all_stay() {
        // 250 ms at 48 kHz. "Iwo" is two vowel groups, so the floor is 200 ms.
        let span = 12_000u64;
        let gap = 4_800u64;
        let mut words = Vec::new();
        for index in 0..5u64 {
            let start = index * (span + gap);
            words.push(word(
                "Iwo",
                start,
                start + span,
                Some(true),
                Some(true),
                if index == 0 { None } else { Some(true) },
            ));
        }
        let decisions = judge_whisper_words(&words);
        assert!(
            decisions.iter().all(|decision| decision.admits()),
            "five Iwo, each on its own voiced segment with a valley between, are real repeats and must all stay"
        );
        assert_eq!(
            decisions.len(),
            5,
            "each Iwo is judged, and none is dropped"
        );
    }

    #[test]
    fn unmeasured_evidence_keeps_words_and_says_unmeasured() {
        let span = 960u64;
        let words = (0..4u64)
            .map(|index| {
                let start = index * span;
                word("nie", start, start + span, None, None, None)
            })
            .collect::<Vec<_>>();
        let decisions = judge_whisper_words(&words);
        assert!(
            decisions.iter().all(|decision| decision.admits()),
            "unmeasured speech evidence must keep today's words, including a short identical run"
        );
        assert!(
            decisions
                .iter()
                .all(|decision| decision.receipt() == WHISPER_WORD_SPEECH_UNMEASURED),
            "the receipt must say unmeasured so a missing floor is not presented as silence or as speech"
        );
    }

    #[test]
    fn flat_noise_repeat_without_a_valley_keeps_only_the_first() {
        // 400 ms, above the one-syllable floor, so duration is not the reason.
        let span = 19_200u64;
        let mut words = vec![word("trzy", 0, span, Some(false), Some(true), None)];
        for index in 1..16u64 {
            let start = index * span;
            words.push(word(
                "trzy",
                start,
                start + span,
                Some(false),
                Some(true),
                Some(false),
            ));
        }
        let decisions = judge_whisper_words(&words);
        let admitted = decisions
            .iter()
            .filter(|decision| decision.admits())
            .count();
        assert_eq!(
            admitted, 1,
            "noise above the floor can admit the first trzy; fifteen copies that share one region must not"
        );
        assert!(
            decisions.iter().skip(1).all(|decision| {
                decision.receipt() == RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment.as_str()
            }),
            "a repeat with no valley and no Silero dip is not its own voiced segment"
        );
    }

    #[test]
    fn separated_iwo_shorter_than_two_syllables_is_refused() {
        // 150 ms at 48 kHz. Two vowel groups need 200 ms. "nie" at 120 ms is one.
        let short_iwo = vec![
            word("Iwo", 0, 12_000, Some(true), Some(true), None),
            word("Iwo", 14_000, 21_200, Some(true), Some(true), Some(true)),
        ];
        let short = judge_whisper_words(&short_iwo);
        assert!(
            short[0].admits(),
            "the first Iwo is not a repeat and its own speech is enough"
        );
        assert_eq!(
            short[1].receipt(),
            RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment.as_str(),
            "a separated Iwo of 150 ms is under 100 ms per syllable and is not its own repeat"
        );
        let nie = judge_whisper_words(&[
            word("nie", 0, 5_760, Some(true), Some(true), None),
            word("nie", 8_000, 13_760, Some(true), Some(true), Some(true)),
        ]);
        assert!(
            nie.iter().all(|decision| decision.admits()),
            "nie is one syllable; 120 ms with its own valley clears the 100 ms floor"
        );
    }

    #[test]
    fn silero_alone_without_the_calibrated_floor_keeps_the_word_unmeasured() {
        let decisions = judge_whisper_words(&[
            word("Dziękuję", 0, 48_000, Some(false), None, None),
            word("Iwo", 48_000, 60_000, Some(true), None, None),
            word("Iwo", 60_000, 72_000, Some(true), None, Some(false)),
        ]);
        assert!(
            decisions
                .iter()
                .all(|decision| decision.receipt() == WHISPER_WORD_SPEECH_UNMEASURED),
            "a take without a calibration profile keeps today's words: Silero merges short gaps, \
             so on its own it would refuse real repeats inside one speech range"
        );
    }

    #[test]
    fn a_repeat_silero_did_not_hear_is_refused_even_with_a_valley() {
        let decisions = judge_whisper_words(&[
            word("trzy", 0, 19_200, Some(false), Some(true), None),
            word("trzy", 19_200, 38_400, Some(false), Some(true), Some(true)),
        ]);
        assert!(
            decisions[0].admits(),
            "the first word is judged on its own span; energy above the floor is speech evidence"
        );
        assert_eq!(
            decisions[1].receipt(),
            RefuseReason::IdenticalRepeatWithoutOwnVoicedSegment.as_str(),
            "a loop on fluctuating room noise has valleys too; a repeat Silero observed and did \
             not hear is not its own voiced segment"
        );
    }

    #[test]
    fn a_translated_word_on_a_short_pause_beside_speech_stays() {
        // Polish speech decoded to English: "the" lands on a pause inside the
        // sentence, with speech on both sides of it.
        let decisions = judge_whisper_words(&[
            word("I", 0, 9_600, Some(true), Some(true), None),
            near_speech(word("the", 9_600, 14_400, Some(false), Some(false), None)),
            word("patient", 14_400, 33_600, Some(true), Some(true), None),
        ]);
        assert!(
            decisions.iter().all(|decision| decision.admits()),
            "translated words do not line up with the voiced Polish words; a word whose \
             timestamp sits on a short pause beside speech must not be lost"
        );
        assert_eq!(
            decisions[1].receipt(),
            WHISPER_WORD_SPEECH_NEAR,
            "the kept word says why it was kept: its own span was silent"
        );
    }

    #[test]
    fn nearby_speech_does_not_admit_a_silent_repeat() {
        let decisions = judge_whisper_words(&[
            word("nie", 0, 1_920, Some(true), Some(true), None),
            near_speech(word(
                "nie",
                1_920,
                3_840,
                Some(false),
                Some(false),
                Some(false),
            )),
        ]);
        assert_eq!(
            decisions[1].receipt(),
            RefuseReason::NoVoicedHopInPin.as_str(),
            "a loop copy right after the one real nie is near speech, but a repeat needs its own"
        );
    }

    #[test]
    fn touching_whisper_spans_still_find_the_valley_between_two_words() {
        // Two "Iwo" whose Whisper spans share the boundary at 12 000, the usual
        // shape: 13 925 of 19 967 consecutive Whisper pairs in 15–17 X touch.
        let window = separation_window((0, 12_000), (12_000, 24_000))
            .expect("touching spans still have a window between their middles");
        assert_eq!(window, (6_000, 18_000));
        let two_words = [(1_000, 11_000), (13_000, 23_000)];
        assert!(
            window_has_energy_valley(window, &two_words),
            "energy below the floor between two touching words is a valley, or real \
             'Iwo Iwo' with touching timestamps would be refused"
        );
        let one_noise_region = [(0, 24_000)];
        assert!(
            !window_has_energy_valley(window, &one_noise_region),
            "one region above the floor across both words is not a split"
        );
        assert!(
            window_has_silero_dip(window, [(0, 11_000), (13_000, 24_000)].into_iter()),
            "two Silero ranges around the boundary are a dip"
        );
        assert!(
            !window_has_silero_dip(window, [(0, 24_000)].into_iter()),
            "one Silero range across both middles is not a dip"
        );
        assert_eq!(
            separation_window((0, 12_000), (0, 12_000)),
            None,
            "a word that does not start after the previous one has no window"
        );
    }
}
