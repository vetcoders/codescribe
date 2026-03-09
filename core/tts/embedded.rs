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
    let cfg_set = cfg!(embed_tts);
    let weights_size = data::WEIGHTS.len();
    tracing::debug!(
        "[TTS] Embedded check: cfg={}, weights_size={}",
        cfg_set,
        weights_size
    );
    cfg_set && weights_size > 0
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

#[cfg(test)]
mod tests {
    use super::*;

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
