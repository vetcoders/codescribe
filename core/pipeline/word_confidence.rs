//! Per-word acoustic confidence (A6): raw engine evidence about ONE word pin.
//!
//! Three producers can supply a per-word metric, each on its own
//! incomparable scale:
//!
//! * Whisper local: aggregated natural-log token probability of the word's
//!   own text tokens (aggregation = min, decision d1),
//! * Apple SFSpeech: `SFTranscriptionSegment.confidence` in 0…1, only from an
//!   `isFinal` result (0.0 on partials means "no metric", never "low"),
//! * vendor cloud / remote tail: `words[].probability` in 0…1.
//!
//! The scales are never mixed under one threshold (d2). Classification lives
//! in exactly one function here; Swift and the overlay never threshold
//! anything themselves.
//!
//! `None ≠ low`: a missing metric is the honest absence of evidence and is
//! reported as `source_unavailable`, never as a confident word.
//!
//! **The default thresholds below are NOT CALIBRATED.** No corpus run has
//! justified them yet (audit section f). They are conservative placeholders
//! so the classification path is real and testable; treat every orange word
//! they produce as a candidate, not a verdict. Calibration is a separate cut
//! against a reference corpus.

use serde::{Deserialize, Serialize};

use crate::pipeline::acoustic_ledger::{ObservationProducer, OccurrenceIdentity};

/// Which producer metric a [`WordConfidence`] carries. Scales are
/// incomparable across variants; thresholds are per-source (d2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordConfidenceSource {
    /// Min of the word's own text-token logprobs from the decode that
    /// emitted it (d1). Natural log, so values are ≤ 0.
    WhisperTokenLogprob,
    /// `SFTranscriptionSegment.confidence` in 0…1, only from an `isFinal`
    /// result. Absent everywhere else (d4: never invented).
    AppleSegmentConfidence,
    /// Vendor `words[].probability` in 0…1 (remote tail / cloud live).
    VendorWordProbability,
}

impl WordConfidenceSource {
    /// Wire token for receipts, the projection, and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WhisperTokenLogprob => "whisper_token_logprob",
            Self::AppleSegmentConfidence => "apple_segment_confidence",
            Self::VendorWordProbability => "vendor_word_probability",
        }
    }
}

/// Raw engine evidence about one word pin. Never calibrated, never averaged
/// across producers, never inferred from absence.
///
/// `milli_value` is the source-scale value quantized to 1e-3 so ledger slots
/// keep their `Eq` contract (the text of a word is a label; its confidence is
/// evidence pinned to PCM, and evidence must stay comparable by value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WordConfidence {
    pub source: WordConfidenceSource,
    /// Source-scale value × 1000, rounded. Whisper: logprob ≤ 0. Apple /
    /// vendor: probability 0…1000.
    pub milli_value: i32,
    /// Whisper: text tokens aggregated into the word; 1 otherwise.
    pub token_count: u16,
}

impl WordConfidence {
    pub fn new(source: WordConfidenceSource, value: f32, token_count: u16) -> Self {
        Self {
            source,
            milli_value: (value * 1000.0).round() as i32,
            token_count,
        }
    }

    /// Source-scale value (Whisper: natural-log probability; Apple/vendor:
    /// probability in 0…1).
    pub fn value(&self) -> f32 {
        self.milli_value as f32 / 1000.0
    }
}

/// Min of the token logprobs covered by one word span (d1). One unsure
/// subtoken is enough to make the word unsure. `None` when the span is empty
/// or out of range — absence of evidence, never zero.
pub fn min_logprob_in_span(logprobs: &[f32], start: usize, len: usize) -> Option<f32> {
    logprobs
        .get(start..start.saturating_add(len))
        .filter(|slice| !slice.is_empty())
        .map(|slice| slice.iter().copied().fold(f32::INFINITY, f32::min))
}

/// Per-source thresholds (d2). **NOT CALIBRATED** — conservative defaults
/// pending a corpus run; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WordConfidenceThresholds {
    /// Whisper word is uncertain when its min subtoken logprob is below this.
    /// Default −1.0 mirrors the engine's per-window warning floor; it has
    /// never been validated per word.
    pub whisper_logprob: f32,
    /// Vendor word is uncertain when its probability is below this.
    pub vendor_probability: f32,
    /// Apple word is uncertain when its segment confidence is below this.
    /// `None` by default: no Apple threshold exists until the attribute's
    /// real distribution is measured (d4), so Apple words are never painted
    /// from an invented cutoff.
    pub apple_confidence: Option<f32>,
}

/// Uncalibrated conservative defaults (see module docs). Nothing is "low"
/// without a metric, and no metric is "low" without a threshold.
pub const DEFAULT_WHISPER_LOGPROB_THRESHOLD: f32 = -1.0;
pub const DEFAULT_VENDOR_PROBABILITY_THRESHOLD: f32 = 0.5;

/// Env overrides (reload: restart). Registered in `docs/ENV_REGISTRY.toml`.
pub const WORD_CONFIDENCE_WHISPER_LOGPROB_ENV: &str = "CODESCRIBE_WORD_CONFIDENCE_WHISPER_LOGPROB";
pub const WORD_CONFIDENCE_VENDOR_PROBABILITY_ENV: &str =
    "CODESCRIBE_WORD_CONFIDENCE_VENDOR_PROBABILITY";
pub const WORD_CONFIDENCE_APPLE_CONFIDENCE_ENV: &str =
    "CODESCRIBE_WORD_CONFIDENCE_APPLE_CONFIDENCE";

/// Read thresholds from an env-like lookup. Pure: tests drive it with a map,
/// production drives it with `std::env::var` behind the restart cache below.
pub fn thresholds_from_env(read: impl Fn(&str) -> Option<String>) -> WordConfidenceThresholds {
    let whisper_logprob = read(WORD_CONFIDENCE_WHISPER_LOGPROB_ENV)
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(DEFAULT_WHISPER_LOGPROB_THRESHOLD);
    let vendor_probability = read(WORD_CONFIDENCE_VENDOR_PROBABILITY_ENV)
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(DEFAULT_VENDOR_PROBABILITY_THRESHOLD);
    let apple_confidence = read(WORD_CONFIDENCE_APPLE_CONFIDENCE_ENV)
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|value| value.is_finite());
    WordConfidenceThresholds {
        whisper_logprob,
        vendor_probability,
        apple_confidence,
    }
}

/// Process-wide thresholds. Read once per process (reload: restart), so a
/// live session never reclassifies words mid-document.
pub fn word_confidence_thresholds() -> WordConfidenceThresholds {
    static THRESHOLDS: std::sync::OnceLock<WordConfidenceThresholds> = std::sync::OnceLock::new();
    *THRESHOLDS.get_or_init(|| thresholds_from_env(|key| std::env::var(key).ok()))
}

impl WordConfidence {
    /// The single classification authority (d2): per-source threshold, and
    /// `None` thresholds classify nothing. Apple words additionally never
    /// classify while their threshold is unset.
    pub fn is_uncertain(&self, thresholds: &WordConfidenceThresholds) -> bool {
        match self.source {
            WordConfidenceSource::WhisperTokenLogprob => self.value() < thresholds.whisper_logprob,
            WordConfidenceSource::VendorWordProbability => {
                self.value() < thresholds.vendor_probability
            }
            WordConfidenceSource::AppleSegmentConfidence => thresholds
                .apple_confidence
                .is_some_and(|threshold| self.value() < threshold),
        }
    }
}

/// One uncertain word located in the reducer's committed document (A6).
///
/// Identity is the physical occurrence plus the slot's PCM range — never the
/// word text (d3: each word is its own span; five identical words yield five
/// independently keyed spans). `utf16_start`/`utf16_end` are a half-open
/// range in the revision's `rendered_text`, in UTF-16 code units
/// (NSRange-ready), valid only against the same revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncertainSpan {
    /// Owner key: the physical occurrence (PCM). Never the word text.
    pub occurrence: OccurrenceIdentity,
    /// The word pin inside the owner, on the capture clock.
    pub slot_sample_start: u64,
    pub slot_sample_end: u64,
    /// Half-open range in `rendered_text`, in UTF-16 code units.
    pub utf16_start: u32,
    pub utf16_end: u32,
    /// Producer whose slot supplied the word (d7: live spans come only from
    /// Whisper / CloudLive slots).
    pub producer: ObservationProducer,
    pub source: WordConfidenceSource,
    /// Source-scale value × 1000 (keeps the span `Eq`).
    pub milli_value: i32,
    /// Live lexicon rewrote the surface; confidence describes the heard
    /// audio (d5). Renderers mark lexicon, not uncertainty, for these.
    pub surface_rewritten: bool,
}

impl UncertainSpan {
    /// Source-scale value (Whisper: logprob; Apple/vendor: probability).
    pub fn value(&self) -> f32 {
        self.milli_value as f32 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> WordConfidenceThresholds {
        WordConfidenceThresholds {
            whisper_logprob: -1.0,
            vendor_probability: 0.5,
            apple_confidence: None,
        }
    }

    #[test]
    fn whisper_word_confidence_aggregates_min_subtoken_logprob() {
        // (a) A word of two subtokens takes the worse one (d1 = min).
        let logprobs = [-0.10, -1.42, -0.05];
        let value = min_logprob_in_span(&logprobs, 0, 2).expect("span");
        assert_eq!(value, -1.42);
        let confidence = WordConfidence::new(WordConfidenceSource::WhisperTokenLogprob, value, 2);
        assert!(confidence.is_uncertain(&thresholds()));
        assert_eq!(confidence.token_count, 2);
        // The same word without its weak subtoken is confident.
        let head_only = min_logprob_in_span(&logprobs, 0, 1).expect("span");
        let confidence =
            WordConfidence::new(WordConfidenceSource::WhisperTokenLogprob, head_only, 1);
        assert!(!confidence.is_uncertain(&thresholds()));
        assert!(min_logprob_in_span(&logprobs, 3, 1).is_none());
        assert!(min_logprob_in_span(&logprobs, 0, 0).is_none());
    }

    #[test]
    fn thresholds_are_per_source_and_never_mixed() {
        // (b) The same raw value classifies differently per source.
        let thresholds = thresholds();
        let as_whisper = WordConfidence::new(WordConfidenceSource::WhisperTokenLogprob, -0.4, 1);
        let as_vendor = WordConfidence::new(WordConfidenceSource::VendorWordProbability, 0.4, 1);
        let as_apple = WordConfidence::new(WordConfidenceSource::AppleSegmentConfidence, 0.4, 1);
        assert!(!as_whisper.is_uncertain(&thresholds));
        assert!(as_vendor.is_uncertain(&thresholds));
        // Apple has no threshold until the attribute is measured (d4).
        assert!(!as_apple.is_uncertain(&thresholds));

        let with_apple = WordConfidenceThresholds {
            apple_confidence: Some(0.5),
            ..thresholds
        };
        assert!(as_apple.is_uncertain(&with_apple));
    }

    #[test]
    fn env_thresholds_default_to_uncalibrated_conservative_values() {
        let thresholds = thresholds_from_env(|_| None);
        assert_eq!(thresholds.whisper_logprob, -1.0);
        assert_eq!(thresholds.vendor_probability, 0.5);
        assert_eq!(thresholds.apple_confidence, None);
    }

    #[test]
    fn env_thresholds_parse_and_reject_garbage() {
        let thresholds = thresholds_from_env(|key| match key {
            WORD_CONFIDENCE_WHISPER_LOGPROB_ENV => Some("-1.5".to_string()),
            WORD_CONFIDENCE_VENDOR_PROBABILITY_ENV => Some("not-a-float".to_string()),
            WORD_CONFIDENCE_APPLE_CONFIDENCE_ENV => Some("0.42".to_string()),
            _ => None,
        });
        assert_eq!(thresholds.whisper_logprob, -1.5);
        assert_eq!(thresholds.vendor_probability, 0.5);
        assert_eq!(thresholds.apple_confidence, Some(0.42));
    }

    #[test]
    fn quantization_keeps_value_within_a_milli() {
        let confidence =
            WordConfidence::new(WordConfidenceSource::VendorWordProbability, 0.4999, 1);
        assert!((confidence.value() - 0.5).abs() <= 0.001);
    }
}
