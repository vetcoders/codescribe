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

/// Check if embedded model is available
///
/// Note: We only check weights_size, not cfg!(embed_model).
/// The cfg!() macro can return false in workspace builds even when
/// the data was correctly embedded via #[cfg(embed_model)].
/// If weights exist (len > 0), the model is available.
pub fn is_embedded_available() -> bool {
    let weights_size = data::WEIGHTS.len();
    tracing::debug!(weights_size, "Embedded model check");
    weights_size > 0
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

#[cfg(test)]
mod tests {
    use super::*;

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
