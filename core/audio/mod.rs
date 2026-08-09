//! Audio module - recording, loading, and playback
//!
//! ## Submodules
//!
//! - `recorder` - Audio recording from microphone
//! - `loader` - Load audio files (WAV, MP3, etc.)
//! - `playback` - System sound playback

/// Offline audio archive helpers (internal packaging of captured WAV/PCM).
pub(crate) mod archive;
/// Fixed-size PCM framing for STT windows and streaming hops.
pub mod chunker;
/// Decode audio files (WAV/MP3/…) into mono PCM for the STT path.
pub mod loader;
/// System sound playback for dictation start/stop cues.
pub mod playback;
/// Microphone capture into in-memory PCM buffers.
pub mod recorder;
/// Continuous mic capture that yields frames without stopping the session.
pub mod streaming_recorder;

// Re-export main types at module level
pub use loader::load_audio_file;
pub use loader::resample_to_16k;
pub use playback::{play_sound, play_sound_with_volume};
// pub use recorder::Recorder; // Internal use only now
