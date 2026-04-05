//! Embedded Whisper model - direct include via generated code
//!
//! Release builds: Model files included directly in binary (~900MB)
//! Debug builds: Empty slices, use CODESCRIBE_MODEL_PATH
//!
//! Model bytes are loaded DIRECTLY to GPU - zero disk I/O, zero temp files.
//!
//! Created by M&K (c)2026 VetCoders

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

/// Check if embedded model is available.
///
/// Runtime truth is the full bundle, not just the weights blob. If any required
/// asset is empty we should fall back to the path-based loader instead of
/// attempting an embedded init that is guaranteed to fail.
pub fn is_embedded_available() -> bool {
    let config_size = data::CONFIG.len();
    let tokenizer_size = data::TOKENIZER.len();
    let mel_filters_size = data::MEL_FILTERS.len();
    let weights_size = data::WEIGHTS.len();
    let available = has_complete_embedded_bundle(
        data::CONFIG,
        data::TOKENIZER,
        data::MEL_FILTERS,
        data::WEIGHTS,
    );
    tracing::debug!(
        config_size,
        tokenizer_size,
        mel_filters_size,
        weights_size,
        available,
        "Embedded Whisper bundle check"
    );
    available
}

/// Get embedded model data if available
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

/// Embedded model data - static byte slices from binary
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

fn has_complete_embedded_bundle(
    config: &[u8],
    tokenizer: &[u8],
    mel_filters: &[u8],
    weights: &[u8],
) -> bool {
    !config.is_empty() && !tokenizer.is_empty() && !mel_filters.is_empty() && !weights.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bundle_requires_all_required_files() {
        assert!(!has_complete_embedded_bundle(&[], &[1], &[1], &[1]));
        assert!(!has_complete_embedded_bundle(&[1], &[], &[1], &[1]));
        assert!(!has_complete_embedded_bundle(&[1], &[1], &[], &[1]));
        assert!(!has_complete_embedded_bundle(&[1], &[1], &[1], &[]));
        assert!(has_complete_embedded_bundle(&[1], &[1], &[1], &[1]));
    }

    #[test]
    fn test_embedded_availability() {
        let available = is_embedded_available();
        println!("Embedded model available: {}", available);

        if available {
            let model = get_embedded_data().unwrap();
            println!(
                "Model size: {:.1} MB",
                model.total_size() as f64 / 1_000_000.0
            );
        }
    }
}
