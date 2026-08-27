//! Streaming transcription — Apple live capture and buffered replay seams.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! from the audio capture layer. Live capture has one dispatcher and one
//! caller-owned session/capture identity; buffered helpers reuse that path with
//! an explicit offline epoch.
//!
//! Decomposed into responsibility modules; this facade exports only surviving
//! session, receipt, and replay surfaces.

/// Apple on-device recognition session driving the live layer.
pub(crate) mod apple_live_session;
/// Coalesce ~5 Apple segments into one Layer 1 Whisper window.
pub(crate) mod layer1_window;
/// Bounded per-session PCM retention, so a sealed utterance can be re-read for tail-patch.
pub(crate) mod live_audio_buffer;
/// Event-based transcription session and buffered production-replay seams.
pub(crate) mod session;
/// W13-3B Silero identity + conservative per-word fusion (product-owned arming).
pub(crate) mod silero_fusion;
/// Session stream-log sink (`CODESCRIBE_STREAM_LOG*` env contract).
pub(crate) mod stream_log;

pub use apple_live_session::APPLE_FINAL_OVERLAP_WARNING_CODE;
pub use session::{
    SessionConfig, TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE, TailPatchDrainDisposition,
    TailPatchSessionReceipt, collect_buffered_engine_events,
    collect_buffered_engine_events_with_config,
};
pub use silero_fusion::{SILERO_FUSION_ENV, SealLaneProbe, seal_lane_probe};

pub(crate) use session::transcription_session;
pub(crate) use stream_log::stream_log_path;
