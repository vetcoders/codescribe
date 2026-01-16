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
pub mod streaming_recorder;

// Re-export main types at module level
pub use loader::load_audio_file;
#[allow(unused_imports)] // Used by E2E tests
pub use loader::resample_to_16k;
pub use playback::play_sound;
// pub use recorder::Recorder; // Internal use only now
