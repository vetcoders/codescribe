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
    /// Size 5 catches more variants than default 3 (e.g., "jest." vs "jest")
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
    /// Initial prompt to guide transcription (domain vocabulary, formatting hints)
    /// Tokenized and prepended to decoder context after special tokens
    /// Env: WHISPER_INITIAL_PROMPT
    pub initial_prompt: Option<String>,
}

impl Default for DecodingParams {
    fn default() -> Self {
        Self {
            temperature: 0.0, // greedy (mlx_whisper default)
            // Block 5-gram repetitions during decoding (preventive, not reactive)
            // Size 5 catches more repetition variants than default 3:
            // - "jest." vs "jest" variations
            // - longer phrase loops like "w tej chwili w tej chwili"
            // faster-whisper/whisper.cpp often use 4-5 for better quality
            no_repeat_ngram_size: 5,
            suppress_blank: true,
            // Stricter thresholds (aligned with faster-whisper / API):
            // - Reduces hallucinations by rejecting low-quality decodings
            // - Matches lbrx-services/stt-engine defaults for consistency
            no_speech_threshold: 0.5, // was 0.6 - stricter silence detection
            compression_ratio_threshold: 2.0, // was 2.4 - stricter hallucination detection
            logprob_threshold: -0.5,  // was -1.0 - reject low-confidence output
            // Initial prompt from env - helps with domain vocabulary and formatting
            initial_prompt: std::env::var("WHISPER_INITIAL_PROMPT")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }
}
