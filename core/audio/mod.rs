//! Audio module - recording, loading, and playback
//!
//! ## Submodules
//!
//! - `recorder` - Audio recording from microphone
//! - `loader` - Load audio files (WAV, MP3, etc.)
//! - `playback` - System sound playback

pub(crate) mod archive;
pub mod chunker;
pub mod loader;
pub mod playback;
pub mod recorder;
pub mod streaming_recorder;

// Re-export main types at module level
pub use loader::load_audio_file;
pub use loader::resample_to_16k;
pub use playback::{play_sound, play_sound_with_volume};
// pub use recorder::Recorder; // Internal use only now
