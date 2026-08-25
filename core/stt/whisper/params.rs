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
    /// Suppress blank/silence tokens early
    pub suppress_blank: bool,
    /// Compression ratio threshold for diagnostic hallucination evidence.
    /// Exceeding it warns but does not authorize decoded-text cleanup.
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
    /// Production defaults: greedy decode, blank suppression, timestamps on.
    fn default() -> Self {
        Self {
            temperature: 0.0, // greedy (mlx_whisper default)
            suppress_blank: true,
            // Diagnostic threshold retained below the stock default for earlier warning.
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

/// Pins default timestamp emission and core decode control values.
#[cfg(test)]
mod tests {
    use super::*;

    /// Segment-aware streaming needs `emit_timestamps` true by default.
    #[test]
    fn default_enables_timestamp_emission_for_segment_aware_pipeline() {
        let params = DecodingParams::default();
        assert!(
            params.emit_timestamps,
            "default decode params should emit timestamps"
        );
    }

    /// Guard against silent drift of temperature and silence thresholds.
    #[test]
    fn default_core_decode_controls_remain_stable() {
        let params = DecodingParams::default();
        assert_eq!(params.temperature, 0.0);
        assert!(params.suppress_blank);
        assert_eq!(params.compression_ratio_threshold, 2.2);
        assert_eq!(params.logprob_threshold, -1.0);
        assert!(params.initial_prompt.is_none());
    }
}
