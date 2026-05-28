//! Embedded Silero VAD model — direct include via generated code.
//!
//! Release builds: Model file included directly in binary (~2.3MB).
//! Debug builds: Empty slice, use runtime file path instead.

#[cfg(embed_vad)]
mod data {
    include!(concat!(env!("OUT_DIR"), "/embedded_vad_data.rs"));
}

#[cfg(not(embed_vad))]
mod data {
    pub static MODEL: &[u8] = &[];
}

/// Check if embedded VAD model is available (only true in release with embed_vad).
pub fn is_embedded_available() -> bool {
    let size = data::MODEL.len();
    tracing::debug!(size, "Embedded VAD check");
    size > 0
}

/// Get embedded model bytes if available.
pub fn get_embedded_data() -> Option<&'static [u8]> {
    if !is_embedded_available() {
        return None;
    }
    Some(data::MODEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_availability() {
        let available = is_embedded_available();
        println!("Embedded VAD available: {available}");
        if available {
            println!("VAD size: {:.2} MB", data::MODEL.len() as f64 / 1_000_000.0);
        }
    }
}
