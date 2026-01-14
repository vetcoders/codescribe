//! Audio module - recording, loading, and playback
//!
//! ## Submodules
//!
//! - `recorder` - Audio recording from microphone
//! - `loader` - Load audio files (WAV, MP3, etc.)
//! - `playback` - System sound playback
//!
//! Created by M&K (c)2026 VetCoders

pub mod loader;
pub mod playback;
pub mod recorder;

// Re-export main types at module level
pub use loader::{load_audio_file, resample_to_16k};
pub use playback::{play_sound, play_sound_with_volume};
pub use recorder::{Recorder, RecorderConfig, RecorderDiagnostics};
