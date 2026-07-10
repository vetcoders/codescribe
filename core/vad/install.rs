//! Silero VAD legacy model path helpers.
//!
//! The embedded/Hugging Face cache path is canonical for runtime loading. These
//! helpers remain for legacy user-model path checks.

use std::path::PathBuf;

/// Model filename (as expected by the loader).
pub const SILERO_VAD_FILE: &str = "silero_vad.onnx";

/// Legacy/user models dir: `~/.codescribe/models/`.
pub fn user_models_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codescribe")
        .join("models")
}

/// Legacy/user model path: `~/.codescribe/models/silero_vad.onnx`.
pub fn user_model_path() -> PathBuf {
    user_models_dir().join(SILERO_VAD_FILE)
}
