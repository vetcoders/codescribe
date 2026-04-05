//! Embedded TTS model - direct include via generated code
//!
//! Release builds: Model files included directly in binary (~1GB)
//! Debug builds: Empty slices, use CODESCRIBE_TTS_PATH
//!
//! Model bytes are loaded DIRECTLY to GPU - zero disk I/O, zero temp files.
//!
//! Created by M&K (c)2026 VetCoders

#[cfg(embed_tts)]
mod data {
    include!(concat!(env!("OUT_DIR"), "/embedded_tts_data.rs"));
}

#[cfg(not(embed_tts))]
mod data {
    pub static CONFIG: &[u8] = &[];
    pub static TOKENIZER: &[u8] = &[];
    pub static WEIGHTS: &[u8] = &[];
    pub static MIMI_CONFIG: &[u8] = &[];
    pub static MIMI_WEIGHTS: &[u8] = &[];
    pub static VOICE_TOKENS: &[u8] = &[];
}

/// Check if embedded model is available
pub fn is_embedded_available() -> bool {
    let config_size = data::CONFIG.len();
    let tokenizer_size = data::TOKENIZER.len();
    let weights_size = data::WEIGHTS.len();
    let mimi_config_size = data::MIMI_CONFIG.len();
    let mimi_weights_size = data::MIMI_WEIGHTS.len();
    let voice_tokens_size = data::VOICE_TOKENS.len();
    let available = has_complete_embedded_bundle(
        data::CONFIG,
        data::TOKENIZER,
        data::WEIGHTS,
        data::MIMI_CONFIG,
        data::MIMI_WEIGHTS,
        data::VOICE_TOKENS,
    );
    tracing::debug!(
        config_size,
        tokenizer_size,
        weights_size,
        mimi_config_size,
        mimi_weights_size,
        voice_tokens_size,
        available,
        "[TTS] Embedded bundle check"
    );
    available
}

/// Get embedded model data if available
pub fn get_embedded_data() -> Option<EmbeddedTts> {
    if !is_embedded_available() {
        return None;
    }
    Some(EmbeddedTts {
        config: data::CONFIG,
        tokenizer: data::TOKENIZER,
        weights: data::WEIGHTS,
        mimi_config: data::MIMI_CONFIG,
        mimi_weights: data::MIMI_WEIGHTS,
        voice_tokens: data::VOICE_TOKENS,
    })
}

/// Embedded TTS model data - static byte slices from binary
///
/// Contains:
/// - CSM model config and weights
/// - Mimi codec config and weights (for audio decoding)
/// - Voice tokens for speaker embedding
pub struct EmbeddedTts {
    /// CSM model configuration (config.json)
    pub config: &'static [u8],
    /// Llama tokenizer for text processing (tokenizer.json)
    pub tokenizer: &'static [u8],
    /// CSM model weights (model.safetensors)
    pub weights: &'static [u8],
    /// Mimi codec configuration (mimi_config.json)
    pub mimi_config: &'static [u8],
    /// Mimi codec weights (mimi.safetensors)
    pub mimi_weights: &'static [u8],
    /// Speaker voice tokens (voice.safetensors)
    pub voice_tokens: &'static [u8],
}

impl EmbeddedTts {
    /// Total size in bytes
    pub fn total_size(&self) -> usize {
        self.config.len()
            + self.tokenizer.len()
            + self.weights.len()
            + self.mimi_config.len()
            + self.mimi_weights.len()
            + self.voice_tokens.len()
    }
}

fn has_complete_embedded_bundle(
    config: &[u8],
    tokenizer: &[u8],
    weights: &[u8],
    mimi_config: &[u8],
    mimi_weights: &[u8],
    voice_tokens: &[u8],
) -> bool {
    !config.is_empty()
        && !tokenizer.is_empty()
        && !weights.is_empty()
        && !mimi_config.is_empty()
        && !mimi_weights.is_empty()
        && !voice_tokens.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bundle_requires_all_required_files() {
        assert!(!has_complete_embedded_bundle(
            &[],
            &[1],
            &[1],
            &[1],
            &[1],
            &[1]
        ));
        assert!(!has_complete_embedded_bundle(
            &[1],
            &[],
            &[1],
            &[1],
            &[1],
            &[1]
        ));
        assert!(!has_complete_embedded_bundle(
            &[1],
            &[1],
            &[],
            &[1],
            &[1],
            &[1]
        ));
        assert!(!has_complete_embedded_bundle(
            &[1],
            &[1],
            &[1],
            &[],
            &[1],
            &[1]
        ));
        assert!(!has_complete_embedded_bundle(
            &[1],
            &[1],
            &[1],
            &[1],
            &[],
            &[1]
        ));
        assert!(!has_complete_embedded_bundle(
            &[1],
            &[1],
            &[1],
            &[1],
            &[1],
            &[]
        ));
        assert!(has_complete_embedded_bundle(
            &[1],
            &[1],
            &[1],
            &[1],
            &[1],
            &[1]
        ));
    }

    #[test]
    fn test_embedded_availability() {
        let available = is_embedded_available();
        println!("Embedded TTS model available: {}", available);

        if available {
            let model = get_embedded_data().unwrap();
            println!(
                "TTS model size: {:.1} MB",
                model.total_size() as f64 / 1_000_000.0
            );
        }
    }
}
