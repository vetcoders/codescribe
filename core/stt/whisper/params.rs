//! Decoding parameters for Whisper transcription.
//!
//! Based on OpenAI whisper / mlx_whisper / faster-whisper best practices.

/// Decoding parameters for Whisper transcription
/// Based on OpenAI whisper / mlx_whisper / faster-whisper best practices
#[derive(Clone, Debug)]
pub struct DecodingParams {
    /// Temperature for sampling (0.0 = greedy, higher = more random)
    /// mlx_whisper default: 0
    pub temperature: f32,
    /// Prevent repetitions of n-grams with this size (0 = disabled)
    /// faster-whisper default: 3
    pub no_repeat_ngram_size: usize,
    /// Suppress blank/silence tokens early
    pub suppress_blank: bool,
    /// No-speech probability threshold - if no_speech_prob > this, segment is silence
    /// mlx_whisper default: 0.6
    pub no_speech_threshold: f32,
    /// Compression ratio threshold for hallucination detection
    /// If gzip ratio > this, decoding failed (hallucination)
    /// mlx_whisper default: 2.4
    pub compression_ratio_threshold: f32,
    /// Log probability threshold - if avg logprob < this, decoding failed
    /// mlx_whisper default: -1.0
    pub logprob_threshold: f32,
    /// Initial prompt to guide the decoder (helps with vocabulary/formatting)
    /// Can contain domain-specific terms to improve accuracy
    pub initial_prompt: Option<String>,
    /// Emit native Whisper timestamp tokens and parse them into transcript segments.
    pub emit_timestamps: bool,
}

impl Default for DecodingParams {
    fn default() -> Self {
        Self {
            temperature: 0.0,        // greedy (mlx_whisper default)
            no_repeat_ngram_size: 5, // block 5-gram repetitions (catches more Whisper hallucination variants)
            suppress_blank: true,
            // More conservative silence rejection (fewer false-empty transcripts on short utterances)
            no_speech_threshold: 0.72,
            // Trigger anti-repetition cleanup a bit earlier than stock defaults
            compression_ratio_threshold: 2.2,
            logprob_threshold: -1.0, // mlx_whisper default
            initial_prompt: None,    // no prompt by default
            // Enabled so streaming can perform timestamp-aware overlap dedup where
            // segment metadata is available. Callers without timestamp tokens keep
            // the existing text-only fallback (`segments = []`).
            emit_timestamps: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_timestamp_emission_for_segment_aware_pipeline() {
        let params = DecodingParams::default();
        assert!(
            params.emit_timestamps,
            "default decode params should emit timestamps"
        );
    }

    #[test]
    fn default_core_decode_controls_remain_stable() {
        let params = DecodingParams::default();
        assert_eq!(params.temperature, 0.0);
        assert_eq!(params.no_repeat_ngram_size, 5);
        assert!(params.suppress_blank);
        assert_eq!(params.no_speech_threshold, 0.72);
        assert_eq!(params.compression_ratio_threshold, 2.2);
        assert_eq!(params.logprob_threshold, -1.0);
        assert!(params.initial_prompt.is_none());
    }
}
