//! PCM decides where speech exists; a Whisper word is a label laid on it.
//!
//! The session measures Silero and the calibrated existence floor on the
//! capture clock. This module does not read PCM and does not turn a missing
//! channel into silence.
//!
//! A word is refused only when PCM measured no speech where it lies: its span
//! touches no Silero speech island, or, before Silero has observed the span,
//! no hop reaches the existence floor. The pin's length and where its edges
//! fall are decoder timing, not acoustic boundaries, so they never refuse a
//! word. When neither channel has measured the span, the word stays visible
//! and cannot mutate the document.

use super::acoustic_ledger::{RefuseReason, SilenceUnderSpan};
use super::engine_tuning::EngineTuning;

/// Historical tuning default retained by the existing settings surface.
/// Word admission does not infer occurrence identity from syllable duration.
pub const WHISPER_REPEAT_MIN_MS_PER_SYLLABLE: u64 = 100;

/// Radius retained for distance diagnostics and the existing tuning surface.
/// It does not grant admission to a word without its own Silero intersection.
///
/// Archived Div0 evidence from 15–17 October contains 689 words without
/// Silero speech under their spans; 303 lie within 500 ms of speech. That
/// proximity was previously an admission path. It remains useful to diagnose
/// timestamp jitter, but cannot establish a spoken occurrence.
pub const WHISPER_WORD_SPEECH_NEAR_MS: u64 = 500;

/// Receipt label when required evidence was not measured. The word may stay
/// visible, but this verdict never grants document admission.
pub const WHISPER_WORD_SPEECH_UNMEASURED: &str = "unmeasured";

/// Historical receipt label retained for archived decisions. Nearness no
/// longer grants admission.
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

/// One Whisper word plus the speech facts already measured for its span.
#[derive(Debug, Clone, PartialEq)]
pub struct WhisperWordSpeech {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
    pub sample_rate: u32,
    /// `Some` only when Silero observed this span on the capture clock.
    /// `true` when the span overlaps a speech island: Silero speech joined
    /// across the short pauses inside a sentence.
    pub silero_speech: Option<bool>,
    /// `Some` only when the calibrated existence floor was applied to this span.
    /// `true` when a hop inside the span is at or above that floor. `None`
    /// is unknown energy; it cannot override Silero's speech measurement.
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
    /// Speech was measured on this span.
    Admit,
    /// A required speech channel was not measured. Visibility only.
    Unmeasured,
    /// Historical verdict; no current decision uses nearby speech to admit.
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
    /// Only a fully measured admission can mutate the document.
    pub fn admits(self) -> bool {
        matches!(self.verdict, WhisperWordSpeechVerdict::Admit)
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
/// Each pin is judged on its own speech measurement. Word identity and
/// overlapping pins are resolved by the ledger, independently of spelling.
pub fn judge_whisper_words(words: &[WhisperWordSpeech]) -> Vec<WhisperWordSpeechDecision> {
    judge_whisper_words_tuned(words, &EngineTuning::DEFAULT)
}

/// The tuning remains available for the existing take configuration; neither
/// duration nor nearness grants word admission.
pub fn judge_whisper_words_tuned(
    words: &[WhisperWordSpeech],
    _tuning: &EngineTuning,
) -> Vec<WhisperWordSpeechDecision> {
    words
        .iter()
        .map(|word| WhisperWordSpeechDecision {
            verdict: judge_one(word),
        })
        .collect()
}

fn judge_one(word: &WhisperWordSpeech) -> WhisperWordSpeechVerdict {
    // Silero is the speech witness: its island decides, and energy cannot
    // promote room noise it rejected to speech. A word on an island keeps its
    // place even where its own pin covers a pause. Energy decides only where
    // Silero has not observed the span yet.
    match (word.silero_speech, word.energy_above_floor) {
        (Some(false), _) => {
            return WhisperWordSpeechVerdict::Refuse(RefuseReason::SpanWithoutOwnSpeech);
        }
        (None, Some(false)) => {
            return WhisperWordSpeechVerdict::Refuse(RefuseReason::NoVoicedHopInPin);
        }
        (None, None) => return WhisperWordSpeechVerdict::Unmeasured,
        _ => {}
    }
    WhisperWordSpeechVerdict::Admit
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
    fn a_word_on_no_speech_island_is_refused_without_a_calibration_profile() {
        // Div0 cdff34c7 (17 X 03:41Z): "nie," 2 176 ms from the nearest speech,
        // on sound the existence floor calls voiced.
        let decisions = judge_whisper_words(&[silero_distance(
            word("nie,", 0, 9_600, Some(false), None, None),
            SileroSpeechDistance::Ms(2_176),
            SilenceUnderSpan::SoundWithoutSpeech,
        )]);
        assert_eq!(
            decisions[0].verdict,
            WhisperWordSpeechVerdict::Refuse(RefuseReason::SpanWithoutOwnSpeech),
            "no speech island lies under the span; a missing floor must not keep a word \
             written over silence"
        );
        assert_eq!(decisions[0].receipt(), "span_without_own_speech");
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
    fn digital_zero_with_no_speech_in_the_epoch_is_refused() {
        let decisions = judge_whisper_words(&[silero_distance(
            word("Dziękuję.", 0, 48_000, Some(false), Some(false), None),
            SileroSpeechDistance::NoSpeechHeard,
            SilenceUnderSpan::DigitalZero,
        )]);
        assert_eq!(
            decisions[0].verdict,
            WhisperWordSpeechVerdict::Refuse(RefuseReason::SpanWithoutOwnSpeech),
            "digital zero holds no speech island"
        );
    }

    #[test]
    fn nearness_to_speech_off_an_island_grants_nothing() {
        // The session already joins Silero speech across short pauses into
        // islands. A span that still touches none is off speech, however near.
        let edge = judge_whisper_words(&[silero_distance(
            near_speech(word("the", 0, 4_800, Some(false), Some(false), None)),
            SileroSpeechDistance::Ms(WHISPER_WORD_SPEECH_NEAR_MS),
            SilenceUnderSpan::BelowFloor,
        )]);
        assert_eq!(edge[0].receipt(), "span_without_own_speech");
        let jitter = judge_whisper_words(&[silero_distance(
            word("Co?", 0, 4_800, Some(false), None, None),
            SileroSpeechDistance::Ms(64),
            SilenceUnderSpan::EnergyUnmeasured,
        )]);
        assert_eq!(jitter[0].receipt(), "span_without_own_speech");
    }

    #[test]
    fn slider_cannot_reclassify_measured_speech_from_spelling_or_duration() {
        let strict = EngineTuning::with_ledger_sensitivity(-100);
        let loose = EngineTuning::with_ledger_sensitivity(100);

        // A separated one-syllable repeat of 130 ms (6 240 samples at 48 kHz):
        // above today's 100 ms floor, below the strict 160 ms one.
        let repeat = [
            word("nie", 0, 6_240, Some(true), Some(true), None),
            word("nie", 9_600, 15_840, Some(true), Some(true), Some(true)),
        ];
        let short_repeat = [
            word("nie", 0, 3_840, Some(true), Some(true), None),
            word("nie", 9_600, 13_440, Some(true), Some(true), Some(true)),
        ];
        for tuning in [&strict, &loose] {
            assert!(
                judge_whisper_words_tuned(&repeat, tuning)
                    .iter()
                    .all(|word| word.admits())
            );
            assert!(
                judge_whisper_words_tuned(&short_repeat, tuning)
                    .iter()
                    .all(|word| word.admits()),
                "sensitivity cannot erase independently measured short spoken pins"
            );
        }

        assert_eq!(
            judge_whisper_words_tuned(&repeat, &EngineTuning::with_ledger_sensitivity(0)),
            judge_whisper_words(&repeat),
            "the slider's centre is today's judge"
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
                .all(|decision| decision.receipt() == RefuseReason::SpanWithoutOwnSpeech.as_str()),
            "a span measured as silence lies on no speech island, not a string-equality drop"
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
                .all(|decision| decision.receipt() == RefuseReason::SpanWithoutOwnSpeech.as_str()),
            "digital zero lies on no speech island, including the first copy"
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
    fn unmeasured_pcm_keeps_words_visible_and_says_unmeasured() {
        let span = 960u64;
        let words = (0..4u64)
            .map(|index| {
                let start = index * span;
                word("nie", start, start + span, None, None, None)
            })
            .collect::<Vec<_>>();
        let decisions = judge_whisper_words(&words);
        assert!(
            decisions.iter().all(|decision| !decision.admits()),
            "with neither PCM channel measured, no word can mutate the document"
        );
        assert!(
            decisions
                .iter()
                .all(|decision| decision.receipt() == WHISPER_WORD_SPEECH_UNMEASURED),
            "the receipt must say unmeasured so a missing floor is not presented as silence or as speech"
        );
    }

    #[test]
    fn disjoint_measured_pins_do_not_require_a_valley_between_equal_labels() {
        // 400 ms, above the one-syllable floor, so duration is not the reason.
        // One voiced region on one speech island, no valley and no dip inside.
        let span = 19_200u64;
        let mut words = vec![word("trzy", 0, span, Some(true), Some(true), None)];
        for index in 1..16u64 {
            let start = index * span;
            words.push(word(
                "trzy",
                start,
                start + span,
                Some(true),
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
            admitted, 16,
            "each supplied span is measured as speech; one continuous island cannot erase disjoint word pins"
        );
        assert!(decisions.iter().all(|decision| decision.admits()));
    }

    #[test]
    fn short_measured_words_do_not_need_a_letter_derived_duration_floor() {
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
        assert!(
            short[1].admits(),
            "letter-derived syllable counts cannot overrule the pin's measured speech"
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
    fn silero_alone_decides_where_no_floor_is_calibrated() {
        let decisions = judge_whisper_words(&[
            word("Dziękuję", 0, 48_000, Some(false), None, None),
            word("Iwo", 48_000, 60_000, Some(true), None, None),
        ]);
        assert_eq!(
            decisions[0].receipt(),
            RefuseReason::SpanWithoutOwnSpeech.as_str(),
            "off every island is off speech, with or without a floor"
        );
        assert!(
            decisions[1].admits(),
            "a word on an island needs no calibrated floor to have its place"
        );
    }

    #[test]
    fn room_noise_silero_did_not_hear_admits_no_copy() {
        let decisions = judge_whisper_words(&[
            word("trzy", 0, 19_200, Some(false), Some(true), None),
            word("trzy", 19_200, 38_400, Some(false), Some(true), Some(true)),
        ]);
        assert!(
            decisions
                .iter()
                .all(|decision| decision.receipt() == RefuseReason::SpanWithoutOwnSpeech.as_str()),
            "energy above the floor is sound; without a speech island it is no word's place, \
             the first copy included"
        );
    }

    #[test]
    fn a_translated_word_on_a_short_pause_inside_a_sentence_stays() {
        // Polish speech decoded to English: "the" lands on a pause inside the
        // sentence, with speech on both sides of it. The pause is shorter than
        // the island gap, so the span lies on the sentence's island; its own
        // pin holds no hop over the floor.
        let decisions = judge_whisper_words(&[
            word("I", 0, 9_600, Some(true), Some(true), None),
            word("the", 9_600, 14_400, Some(true), Some(false), None),
            word("patient", 14_400, 33_600, Some(true), Some(true), None),
        ]);
        assert!(
            decisions.iter().all(|decision| decision.admits()),
            "a word whose timestamp sits on a pause inside the island must not be lost"
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
            RefuseReason::SpanWithoutOwnSpeech.as_str(),
            "a loop copy right after the one real nie is near speech, but off the island"
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
