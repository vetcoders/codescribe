//! Embedded Whisper model bytes.
//!
//! The default product build embeds Whisper when the model is available at
//! build time. These helpers expose that payload to the singleton. When the
//! build is produced with `CODESCRIBE_NO_EMBED=1` or without a complete model
//! snapshot, the payload is intentionally absent and runtime lookup must take
//! over.

#[cfg(embed_model)]
mod data {
    include!(concat!(env!("OUT_DIR"), "/embedded_model_data.rs"));
}

#[cfg(not(embed_model))]
mod data {
    pub static CONFIG: &[u8] = &[];
    pub static TOKENIZER: &[u8] = &[];
    pub static MEL_FILTERS: &[u8] = &[];
    pub static WEIGHTS: &[u8] = &[];
}

/// Check if optional embedded model bytes are available.
pub fn is_embedded_available() -> bool {
    let weights_size = data::WEIGHTS.len();
    tracing::debug!(weights_size, "Embedded model check");
    weights_size > 0
}

/// Get optional embedded model data if available.
pub fn get_embedded_data() -> Option<EmbeddedModel> {
    if !is_embedded_available() {
        return None;
    }
    Some(EmbeddedModel {
        config: data::CONFIG,
        tokenizer: data::TOKENIZER,
        mel_filters: data::MEL_FILTERS,
        weights: data::WEIGHTS,
    })
}

/// Embedded model data - static byte slices from binary.
pub struct EmbeddedModel {
    pub config: &'static [u8],
    pub tokenizer: &'static [u8],
    pub mel_filters: &'static [u8],
    pub weights: &'static [u8],
}

impl EmbeddedModel {
    /// Total size in bytes
    pub fn total_size(&self) -> usize {
        self.config.len() + self.tokenizer.len() + self.mel_filters.len() + self.weights.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(embed_model)]
    fn embedded_payload_is_available_when_compiled_in() {
        assert!(is_embedded_available());
        let model = get_embedded_data().expect("embedded payload must exist");
        assert!(!model.config.is_empty());
        assert!(!model.tokenizer.is_empty());
        assert!(!model.mel_filters.is_empty());
        assert!(!model.weights.is_empty());
        assert!(model.total_size() > 0);
    }

    #[test]
    #[cfg(not(embed_model))]
    fn embedded_payload_is_absent_when_not_compiled_in() {
        assert!(!is_embedded_available());
        assert!(get_embedded_data().is_none());
    }
}
