//! A Whisper word is admitted only when its own sample span carries speech.
//!
//! The session measures Silero and the calibrated existence floor. This module
//! does not read PCM and does not turn a missing channel into silence.

use super::acoustic_ledger::RefuseReason;

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

/// Receipt label when neither Silero nor the calibrated floor was measured.
/// The word is kept. The string is the receipt; it is not a refusal.
pub const WHISPER_WORD_SPEECH_UNMEASURED: &str = "unmeasured";

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
    /// `true` when a hop inside the span is at or above that floor.
    pub energy_above_floor: Option<bool>,
    /// Separation from the previous identical word.
    /// `Some(true)` when a measured channel shows an energy valley or a Silero
    /// dip. `Some(false)` when at least one channel was measured and none of
    /// them found that split. `None` when neither channel was measured.
    pub separated_from_previous: Option<bool>,
}

/// What the judge did with one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperWordSpeechVerdict {
    /// Speech was measured on this span and the repeat rule passed.
    Admit,
    /// A channel the rule needs was not measured. The word keeps today's path.
    Unmeasured,
    /// The span was measured and the word does not enter the document.
    Refuse(RefuseReason),
}

/// One word's verdict and the stable receipt string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperWordSpeechDecision {
    pub verdict: WhisperWordSpeechVerdict,
}

impl WhisperWordSpeechDecision {
    /// Kept words are admitted or explicitly unmeasured. A refusal is not.
    pub fn admits(self) -> bool {
        !matches!(self.verdict, WhisperWordSpeechVerdict::Refuse(_))
    }

    /// Stable receipt text. Unmeasured is a word, not a [`RefuseReason`].
    pub fn receipt(self) -> &'static str {
        match self.verdict {
            WhisperWordSpeechVerdict::Admit => "admitted",
            WhisperWordSpeechVerdict::Unmeasured => WHISPER_WORD_SPEECH_UNMEASURED,
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
    if !span_has_speech(word) {
        return if speech_was_measured(word) {
            WhisperWordSpeechVerdict::Refuse(RefuseReason::NoVoicedHopInPin)
        } else {
            WhisperWordSpeechVerdict::Unmeasured
        };
    }
    if !is_identical_repeat(words, index) {
        return WhisperWordSpeechVerdict::Admit;
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
/// channel saying no is silence. Two missing channels are unmeasured.
fn span_has_speech(word: &WhisperWordSpeech) -> bool {
    word.silero_speech == Some(true) || word.energy_above_floor == Some(true)
}

fn speech_was_measured(word: &WhisperWordSpeech) -> bool {
    word.silero_speech.is_some() || word.energy_above_floor.is_some()
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
            separated_from_previous: separated,
        }
    }

    #[test]
    fn fourteen_nie_on_silence_after_one_voiced_span_keep_one() {
        // 40 ms at 48 kHz. The corpus loop is 20–60 ms per "nie".
        let span = 1_920u64;
        let mut words = vec![word("nie", 0, span, Some(true), Some(true), None)];
        for index in 1..14 {
            let start = u64::from(index) * span;
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
        let words = (0..3)
            .map(|index| {
                let start = u64::from(index) * 48_000;
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
        for index in 0..5 {
            let start = u64::from(index) * (span + gap);
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
        let words = (0..4)
            .map(|index| {
                let start = u64::from(index) * span;
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
        for index in 1..16 {
            let start = u64::from(index) * span;
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
}
