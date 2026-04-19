//! Silero-based silence discriminator for file-level Whisper filtering.
//!
//! Consumes the per-window speech probability stream emitted by
//! `vad::extract_speech` and classifies each 500ms window as speech, a short
//! in-utterance pause, a longer internal sentence boundary, or trailing
//! silence at the end of the recording.

use crate::pipeline::contracts::VadClass;
use crate::vad::config::VadConfig;

pub const DISCRIMINATOR_WINDOW_MS: u32 = 500;

#[derive(Debug, Clone)]
pub struct VadTimeline {
    pub classes: Vec<VadClass>,
    pub window_sec: f32,
}

impl VadTimeline {
    pub fn class_at(&self, t: f32) -> Option<VadClass> {
        if !t.is_finite() || t < 0.0 || self.window_sec <= 0.0 {
            return None;
        }
        let idx = (t / self.window_sec).floor() as usize;
        self.classes.get(idx).copied()
    }

    pub fn overlaps_trailing_silence(&self, start_sec: f32, end_sec: f32) -> bool {
        self.range_slice(start_sec, end_sec)
            .iter()
            .copied()
            .any(|class| class == VadClass::TrailingSilence)
    }

    pub fn dominant_class(&self, start_sec: f32, end_sec: f32) -> Option<VadClass> {
        let mut speech = 0usize;
        let mut utterance_gap = 0usize;
        let mut sentence_boundary = 0usize;
        let mut trailing = 0usize;

        for class in self.range_slice(start_sec, end_sec) {
            match class {
                VadClass::Speech => speech += 1,
                VadClass::UtteranceGap => utterance_gap += 1,
                VadClass::SentenceBoundary => sentence_boundary += 1,
                VadClass::TrailingSilence => trailing += 1,
            }
        }

        [
            (VadClass::Speech, speech),
            (VadClass::UtteranceGap, utterance_gap),
            (VadClass::SentenceBoundary, sentence_boundary),
            (VadClass::TrailingSilence, trailing),
        ]
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .and_then(|(class, count)| (count > 0).then_some(class))
    }

    fn range_slice(&self, start_sec: f32, end_sec: f32) -> &[VadClass] {
        if self.classes.is_empty() || self.window_sec <= 0.0 {
            return &[];
        }

        let bounded_start = if start_sec.is_finite() {
            start_sec.max(0.0)
        } else {
            0.0
        };
        let bounded_end = if end_sec.is_finite() {
            end_sec.max(bounded_start)
        } else {
            bounded_start
        };

        let len = self.classes.len();
        let lo = ((bounded_start / self.window_sec).floor() as usize).min(len);
        let mut hi = ((bounded_end / self.window_sec).ceil() as usize).min(len);
        if hi <= lo {
            hi = (lo + 1).min(len);
        }

        &self.classes[lo..hi]
    }
}

/// Classify each VAD probability window into speech or one of three silence
/// semantics used by the file-level Whisper post-filter.
pub fn classify_windows(probabilities: &[f32], config: &VadConfig) -> VadTimeline {
    let window_sec = DISCRIMINATOR_WINDOW_MS as f32 / 1000.0;
    if probabilities.is_empty() {
        return VadTimeline {
            classes: Vec::new(),
            window_sec,
        };
    }

    let mut classes = Vec::with_capacity(probabilities.len());
    let mut i = 0usize;

    while i < probabilities.len() {
        if probabilities[i] >= config.threshold {
            classes.push(VadClass::Speech);
            i += 1;
            continue;
        }

        let run_start = i;
        while i < probabilities.len() && probabilities[i] < config.threshold {
            i += 1;
        }

        let run_len = i - run_start;
        let run_sec = run_len as f32 * window_sec;
        let class = if run_sec <= config.utterance_gap_threshold_sec {
            VadClass::UtteranceGap
        } else {
            VadClass::SentenceBoundary
        };

        classes.extend(std::iter::repeat_n(class, run_len));
    }

    let mut tail_len = 0usize;
    for class in classes.iter().rev() {
        if *class == VadClass::Speech {
            break;
        }
        tail_len += 1;
    }

    let tail_sec = tail_len as f32 * window_sec;
    if tail_len > 0 && tail_sec >= config.tail_silence_threshold_sec {
        let tail_start = classes.len() - tail_len;
        for class in &mut classes[tail_start..] {
            *class = VadClass::TrailingSilence;
        }
    }

    VadTimeline {
        classes,
        window_sec,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::VadConfig;

    fn default_config() -> VadConfig {
        VadConfig::default()
    }

    #[test]
    fn all_speech_classifies_as_speech() {
        let timeline = classify_windows(&[0.9, 0.8, 0.95], &default_config());
        assert_eq!(
            timeline.classes,
            vec![VadClass::Speech, VadClass::Speech, VadClass::Speech]
        );
    }

    #[test]
    fn short_mid_silence_classifies_as_utterance_gap() {
        let timeline = classify_windows(&[0.9, 0.1, 0.92], &default_config());
        assert_eq!(
            timeline.classes,
            vec![VadClass::Speech, VadClass::UtteranceGap, VadClass::Speech]
        );
    }

    #[test]
    fn medium_mid_silence_classifies_as_sentence_boundary() {
        let timeline = classify_windows(&[0.9, 0.1, 0.1, 0.1, 0.92], &default_config());
        assert_eq!(
            timeline.classes,
            vec![
                VadClass::Speech,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::Speech,
            ]
        );
    }

    #[test]
    fn long_trailing_silence_classifies_as_trailing_silence() {
        let timeline = classify_windows(&[0.9, 0.88, 0.1, 0.1, 0.1, 0.1], &default_config());
        assert_eq!(
            timeline.classes,
            vec![
                VadClass::Speech,
                VadClass::Speech,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
            ]
        );
    }

    #[test]
    fn long_silence_followed_by_more_speech_is_sentence_boundary_not_tail() {
        let timeline = classify_windows(&[0.9, 0.1, 0.1, 0.1, 0.1, 0.92], &default_config());
        assert_eq!(
            timeline.classes,
            vec![
                VadClass::Speech,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::Speech,
            ]
        );
    }

    #[test]
    fn empty_input_returns_empty_timeline() {
        let timeline = classify_windows(&[], &default_config());
        assert!(timeline.classes.is_empty());
        assert_eq!(timeline.window_sec, 0.5);
    }

    #[test]
    fn class_at_returns_correct_window() {
        let timeline = classify_windows(&[0.9, 0.1, 0.92], &default_config());
        assert_eq!(timeline.class_at(0.1), Some(VadClass::Speech));
        assert_eq!(timeline.class_at(0.6), Some(VadClass::UtteranceGap));
        assert_eq!(timeline.class_at(1.1), Some(VadClass::Speech));
        assert_eq!(timeline.class_at(-1.0), None);
    }

    #[test]
    fn overlaps_trailing_silence_detects_partial_overlap() {
        let timeline = classify_windows(&[0.9, 0.9, 0.1, 0.1, 0.1, 0.1], &default_config());
        assert!(timeline.overlaps_trailing_silence(1.8, 2.2));
        assert!(!timeline.overlaps_trailing_silence(0.0, 0.8));
    }
}
