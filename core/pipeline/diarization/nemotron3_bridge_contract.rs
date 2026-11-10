//! Specification for a future Swift actor owning FluidAudio/CoreML inference.
//!
//! This is a Rust contract, not a linked FFI implementation. Transport and Swift
//! changes are outside this cell. Vecs/traits below are not a C ABI.
//!
//! Upstream checked 2026-11-03:
//! - <https://huggingface.co/nvidia/Nemotron-3-Diarization>
//! - <https://huggingface.co/FluidInference/nemotron-3-diarization-coreml>
//! - <https://github.com/FluidInference/FluidAudio/blob/ee78635d23d52379117554ba72358b65a0612447/Documentation/Diarization/Nemotron3.md>
//!
//! The assigned card uses different repository names and calls converted weights
//! Apache-2.0. Public upstream metadata instead declares OpenMDW-1.1 for BOTH
//! original and converted weights; Apache-2.0 belongs to FluidAudio CODE.
//! Retain OpenMDW-1.1 and upstream notices when distributing model materials.
//! No model weights or dependency are bundled or downloaded by this module.
//!
//! VoxConverse dev/test is training data: label it in-domain / non-held-out.
//! An independent public RTTM reference and explicit collar/overlap protocol are
//! required for DER. Oracle disagreement is not DER. Profile buffering budgets
//! exclude compute, frontend, transport and queue delay; RTF/RSS and live p50/p95
//! require real model execution by the integrator.

use super::nemotron3_coreml::{
    Nemotron3Profile, NemotronAudioWindow, NemotronFrameBatch, NemotronFrameLogits,
    NemotronInputError, validate_output_frames,
};

pub const NVIDIA_MODEL_REPOSITORY: &str = "nvidia/Nemotron-3-Diarization";
pub const NVIDIA_MODEL_REVISION: &str = "f667ed73aee57d40cc39428eb768b4fd87a0a29e";
pub const COREML_MODEL_REPOSITORY: &str = "FluidInference/nemotron-3-diarization-coreml";
pub const COREML_MODEL_REVISION: &str = "25a90f97f254428d4b30374b76af9c74fdee8327";
pub const MODEL_WEIGHTS_LICENSE: &str = "OpenMDW-1.1";
pub const FLUIDAUDIO_CODE_LICENSE: &str = "Apache-2.0";
pub const FLUIDAUDIO_SOURCE_REVISION: &str = "ee78635d23d52379117554ba72358b65a0612447";

/// Post-pass receipt: columns are arrival-ordered for this offline epoch only.
/// Offline labels cannot be silently substituted for streaming session labels.
#[derive(Debug, Clone)]
pub struct NemotronOfflineOutput {
    pub session_epoch: u64,
    pub sample_origin: u64,
    pub sample_count: u64,
    pub frames: Vec<NemotronFrameLogits>,
}

impl NemotronOfflineOutput {
    /// Check receipt identity against the REQUEST, not against its own claims.
    pub fn into_validated_frames(
        self,
        expected_epoch: u64,
        expected_origin: u64,
        expected_sample_count: u64,
    ) -> Result<Vec<NemotronFrameLogits>, NemotronInputError> {
        if self.session_epoch != expected_epoch {
            return Err(NemotronInputError::StaleEpoch);
        }
        if self.sample_origin != expected_origin || self.sample_count != expected_sample_count {
            return Err(NemotronInputError::RequestMismatch);
        }
        let end = expected_origin
            .checked_add(expected_sample_count)
            .ok_or(NemotronInputError::SampleOverflow)?;
        validate_output_frames(&self.frames, expected_origin, end, 0)?;
        Ok(self.frames)
    }
}

/// Implement on one serialized Swift actor/task with one Nemotron3Diarizer.
///
/// Required lifecycle:
/// 1. Load pinned model files explicitly; report missing/gated assets as errors.
///    Map presets to Nemotron3Config.offline/low/fast/fast32. The offline preset
///    requires a supported GPU compute route; do not silently change presets.
/// 2. reset clears mel/frontend, speaker cache, FIFO and all output cursors.
///    Tag async callbacks with the submitted epoch/request, including failures.
/// 3. A stream window overlaps the preceding window by right-context samples.
///    Advance the FluidAudio state once per core, preserving its AOSC/FIFO.
///    Consume each real PCM sample once; never append repeated right context
///    as new audio or restart processComplete for each streaming window.
/// 4. Own the exact mel/STFT boundary handling in Swift. The Rust window budget
///    specifies scheduling, not a replacement frontend. If the high-level
///    appendAudio/processBufferedAudio API needs extra frontend lookahead,
///    preserve that context or use its stateful step API; measure the latency.
/// 5. Return exactly the requested core's independent probabilities. Compact
///    strided/padded CoreML tensors using the model's actual strides. No softmax
///    across speakers, context rows, segment smoothing or timestamp rounding.
/// 6. is_final authorizes right padding only after input ends. Flush via
///    finishStream or an equivalent stateful final step; queue any extra tail
///    output until its matching request. Clip the last frame to real PCM.
///    Never emit padding or release microphone ownership from this actor.
/// 7. Surface inference errors and cancel the epoch. Resume only with a fresh
///    epoch and reset caches; do not infer identity from posterior similarity.
///
/// post_pass uses processComplete on the finished immutable recording, reset
/// once. Reconcile native frontend tail rows to ceil(real_samples / 160), with
/// exact sample clipping; do not invent a missing probability row. Empty audio
/// returns an empty receipt without invoking the model. No audio uploads,
/// enrollment, named voiceprints, recording, reducer or bus writes are allowed.
pub trait Nemotron3CoreMlBridge {
    type Error;

    fn reset(&mut self, session_epoch: u64, profile: Nemotron3Profile) -> Result<(), Self::Error>;

    fn post_pass(
        &mut self,
        session_epoch: u64,
        sample_origin: u64,
        pcm_16khz: &[f32],
    ) -> Result<NemotronOfflineOutput, Self::Error>;

    fn process_window(
        &mut self,
        window: &NemotronAudioWindow,
    ) -> Result<NemotronFrameBatch, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> NemotronOfflineOutput {
        NemotronOfflineOutput {
            session_epoch: 7,
            sample_origin: 1000,
            sample_count: 1,
            frames: vec![NemotronFrameLogits {
                frame_index: 0,
                sample_start: 1000,
                sample_end: 1001,
                speaker_probs: [0.0; 8],
            }],
        }
    }

    #[test]
    fn offline_receipt_checks_request_epoch_origin_length_and_tail() {
        assert_eq!(
            output().into_validated_frames(8, 1000, 1),
            Err(NemotronInputError::StaleEpoch)
        );
        assert_eq!(
            output().into_validated_frames(7, 0, 1),
            Err(NemotronInputError::RequestMismatch)
        );
        let mut padded = output();
        padded.frames[0].sample_end = 1160;
        assert_eq!(
            padded.into_validated_frames(7, 1000, 1),
            Err(NemotronInputError::InvalidFrameBatch)
        );
        assert_eq!(output().into_validated_frames(7, 1000, 1).unwrap().len(), 1);
    }
}
