//! Optional embedded Whisper model bytes.
//!
//! The current build policy disables Whisper embedding, so these helpers
//! usually report unavailable. They remain for experimental builds and tests.
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
