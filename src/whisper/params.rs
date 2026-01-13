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
}

impl Default for DecodingParams {
    fn default() -> Self {
        Self {
            temperature: 0.0,        // greedy (mlx_whisper default)
            no_repeat_ngram_size: 3, // block 3-gram repetitions (faster-whisper)
            suppress_blank: true,
            no_speech_threshold: 0.6,         // mlx_whisper default
            compression_ratio_threshold: 2.4, // mlx_whisper default
            logprob_threshold: -1.0,          // mlx_whisper default
        }
    }
}
