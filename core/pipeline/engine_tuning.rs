//! Engine tuning catalog: the live-engine parameters a developer may turn.
//!
//! Each knob has one name, a unit, a range and the trade-off it moves. The
//! value and the evidence behind it stay beside the mechanism that uses it;
//! this catalog only gathers them into one closed list.
//!
//! [`EngineTuning::DEFAULT`] is today's behaviour. Nothing here reads env or
//! settings yet. The next step freezes one tuning per take into the settings
//! snapshot and records it with the take; only after that can a knob differ
//! from its default.

use super::streaming::layer1_window::{LIVE_STEP_SECS, LIVE_WINDOW_SECS};
use super::streaming::silero_fusion::{
    ACOUSTIC_SPEECH_MERGE_GAP_SECS, ACOUSTIC_SPEECH_PAD_SECS, DEFAULT_LEFT_PAD_SECS,
};
use super::whisper_word_speech::{WHISPER_REPEAT_MIN_MS_PER_SYLLABLE, WHISPER_WORD_SPEECH_NEAR_MS};

/// The trade-off a knob moves, for the surface that draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningAxis {
    /// Overlapping Whisper windows: a second opinion on every word and
    /// duplicated words, against decoder context and cost.
    WindowOverlap,
    /// Words written over silence against real words lost.
    WordOverSilence,
    /// Where speech starts and ends on the capture clock.
    SpeechEdges,
}

/// One knob as a developer surface shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuningKnob {
    pub key: &'static str,
    pub unit: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub axis: TuningAxis,
}

/// Every live-engine parameter in the catalog, with today's values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineTuning {
    /// Layer 1 Whisper window length on the capture clock.
    pub live_window_sec: u32,
    /// Grid step between Layer 1 window starts. Never longer than the window.
    pub live_step_sec: u32,
    /// A Whisper word with no Silero speech this close to its span is refused.
    pub whisper_word_speech_near_ms: u64,
    /// Voiced span per syllable an identical repeat needs to stay a word.
    pub whisper_repeat_min_ms_per_syllable: u64,
    /// Pad around a Silero threshold crossing in the seal speech set.
    pub acoustic_speech_pad_sec: f32,
    /// Padded speech ranges this close together are one range.
    pub acoustic_speech_merge_gap_sec: f32,
    /// Pre-roll replayed ahead of the first speech edge in a fresh epoch.
    pub epoch_left_pad_sec: f32,
}

impl EngineTuning {
    pub const DEFAULT: Self = Self {
        live_window_sec: LIVE_WINDOW_SECS,
        live_step_sec: LIVE_STEP_SECS,
        whisper_word_speech_near_ms: WHISPER_WORD_SPEECH_NEAR_MS,
        whisper_repeat_min_ms_per_syllable: WHISPER_REPEAT_MIN_MS_PER_SYLLABLE,
        acoustic_speech_pad_sec: ACOUSTIC_SPEECH_PAD_SECS,
        acoustic_speech_merge_gap_sec: ACOUSTIC_SPEECH_MERGE_GAP_SECS,
        epoch_left_pad_sec: DEFAULT_LEFT_PAD_SECS,
    };

    /// The catalog in display order. Ranges are what a developer surface may
    /// offer, not proven-safe operating points.
    pub const KNOBS: [TuningKnob; 7] = [
        knob(
            "live_window_sec",
            "s",
            LIVE_WINDOW_SECS as f64,
            3.0,
            28.0,
            TuningAxis::WindowOverlap,
        ),
        knob(
            "live_step_sec",
            "s",
            LIVE_STEP_SECS as f64,
            1.0,
            28.0,
            TuningAxis::WindowOverlap,
        ),
        knob(
            "whisper_word_speech_near_ms",
            "ms",
            WHISPER_WORD_SPEECH_NEAR_MS as f64,
            128.0,
            2_000.0,
            TuningAxis::WordOverSilence,
        ),
        knob(
            "whisper_repeat_min_ms_per_syllable",
            "ms",
            WHISPER_REPEAT_MIN_MS_PER_SYLLABLE as f64,
            50.0,
            200.0,
            TuningAxis::WordOverSilence,
        ),
        knob(
            "acoustic_speech_pad_sec",
            "s",
            ACOUSTIC_SPEECH_PAD_SECS as f64,
            0.0,
            0.25,
            TuningAxis::SpeechEdges,
        ),
        knob(
            "acoustic_speech_merge_gap_sec",
            "s",
            ACOUSTIC_SPEECH_MERGE_GAP_SECS as f64,
            0.0,
            0.5,
            TuningAxis::SpeechEdges,
        ),
        knob(
            "epoch_left_pad_sec",
            "s",
            DEFAULT_LEFT_PAD_SECS as f64,
            0.0,
            1.0,
            TuningAxis::SpeechEdges,
        ),
    ];

    /// The value behind one catalog key, in the knob's unit.
    pub fn value(&self, key: &str) -> Option<f64> {
        Some(match key {
            "live_window_sec" => f64::from(self.live_window_sec),
            "live_step_sec" => f64::from(self.live_step_sec),
            "whisper_word_speech_near_ms" => self.whisper_word_speech_near_ms as f64,
            "whisper_repeat_min_ms_per_syllable" => self.whisper_repeat_min_ms_per_syllable as f64,
            "acoustic_speech_pad_sec" => f64::from(self.acoustic_speech_pad_sec),
            "acoustic_speech_merge_gap_sec" => f64::from(self.acoustic_speech_merge_gap_sec),
            "epoch_left_pad_sec" => f64::from(self.epoch_left_pad_sec),
            _ => return None,
        })
    }
}

impl Default for EngineTuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

const fn knob(
    key: &'static str,
    unit: &'static str,
    default: f64,
    min: f64,
    max: f64,
    axis: TuningAxis,
) -> TuningKnob {
    TuningKnob {
        key,
        unit,
        default,
        min,
        max,
        axis,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_knob_reads_its_default_back_inside_its_range() {
        let mut keys = BTreeSet::new();
        for knob in EngineTuning::KNOBS {
            assert!(keys.insert(knob.key), "{} listed twice", knob.key);
            assert_eq!(
                EngineTuning::DEFAULT.value(knob.key),
                Some(knob.default),
                "{}",
                knob.key
            );
            assert!(
                knob.min <= knob.default && knob.default <= knob.max,
                "{}",
                knob.key
            );
        }
        assert_eq!(EngineTuning::DEFAULT.value("not_a_knob"), None);
    }

    #[test]
    fn default_is_todays_engine() {
        let tuning = EngineTuning::default();
        assert_eq!((tuning.live_window_sec, tuning.live_step_sec), (9, 3));
        assert!(tuning.live_step_sec <= tuning.live_window_sec);
        assert_eq!(tuning.whisper_word_speech_near_ms, 500);
        assert_eq!(tuning.whisper_repeat_min_ms_per_syllable, 100);
    }
}
