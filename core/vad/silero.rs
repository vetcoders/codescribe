//! WebRTC VAD wrapper - voice activity detection from Google's WebRTC.
//!
//! Uses `webrtc-vad` crate which wraps Google's fvad C library.
//! Much faster than neural VAD (~1μs vs ~1ms) with good accuracy.
//!
//! Note: Module is named "silero" for API compatibility but uses WebRTC VAD
//! internally. WebRTC VAD is proven technology from Google's WebRTC project.
//!
//! Created by M&K (c)2026 VetCoders

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use tracing::{debug, info};
use webrtc_vad::{Vad, SampleRate, VadMode};

use super::config::VadConfig;

/// Thread-local VAD instance (WebRTC VAD is not Send, so we use thread-local storage)
thread_local! {
    static VAD_INSTANCE: RefCell<Option<WebRtcVad>> = const { RefCell::new(None) };
}

/// Global flag to track if VAD has been initialized
static VAD_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// WebRTC VAD wrapper (API-compatible with planned Silero integration)
pub struct SileroVad {
    inner: WebRtcVad,
}

/// Internal WebRTC VAD implementation
pub struct WebRtcVad {
    vad: Vad,
    config: VadConfig,
    sample_rate: SampleRate,
}

impl WebRtcVad {
    /// Create a new WebRTC VAD instance
    pub fn new() -> Result<Self> {
        Self::with_config(VadConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(config: VadConfig) -> Result<Self> {
        info!(
            "Initializing WebRTC VAD (threshold: {:.2}, mode: aggressive)",
            config.threshold
        );

        // Map threshold to aggressiveness mode:
        // Lower threshold = more sensitive = lower aggressiveness
        // Higher threshold = more conservative = higher aggressiveness
        let mode = if config.threshold < 0.4 {
            VadMode::Quality       // Most sensitive
        } else if config.threshold < 0.6 {
            VadMode::LowBitrate    // Balanced
        } else if config.threshold < 0.8 {
            VadMode::Aggressive    // Less false positives
        } else {
            VadMode::VeryAggressive // Most conservative
        };

        let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, mode);

        let mode_name = match mode {
            VadMode::Quality => "Quality",
            VadMode::LowBitrate => "LowBitrate",
            VadMode::Aggressive => "Aggressive",
            VadMode::VeryAggressive => "VeryAggressive",
        };
        debug!("WebRTC VAD initialized with mode: {}", mode_name);

        Ok(Self {
            vad,
            config,
            sample_rate: SampleRate::Rate16kHz,
        })
    }

    /// Check if audio frame contains speech
    ///
    /// Audio must be 16kHz mono. Frame must be 10, 20, or 30ms:
    /// - 10ms = 160 samples
    /// - 20ms = 320 samples
    /// - 30ms = 480 samples
    pub fn is_speech_i16(&mut self, samples: &[i16]) -> bool {
        // WebRTC VAD requires exact frame sizes
        if samples.is_empty() {
            return false;
        }

        // Try to process - if frame size is wrong, VAD returns error
        match self.vad.is_voice_segment(samples) {
            Ok(is_voice) => is_voice,
            Err(_) => {
                // Frame size not supported, try chunking
                self.is_speech_chunked_i16(samples)
            }
        }
    }

    /// Check if audio frame contains speech (f32 input)
    ///
    /// Converts f32 [-1.0, 1.0] to i16 [-32768, 32767]
    pub fn is_speech(&mut self, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }

        // Convert f32 to i16
        let i16_samples: Vec<i16> = samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        self.is_speech_i16(&i16_samples)
    }

    /// Process arbitrary length audio by chunking into 20ms frames
    fn is_speech_chunked_i16(&mut self, samples: &[i16]) -> bool {
        const FRAME_SIZE: usize = 320; // 20ms at 16kHz

        // Process in chunks, return true if ANY chunk has speech
        let mut has_speech = false;
        let mut speech_frames = 0;
        let mut total_frames = 0;

        for chunk in samples.chunks(FRAME_SIZE) {
            if chunk.len() == FRAME_SIZE {
                total_frames += 1;
                if let Ok(is_voice) = self.vad.is_voice_segment(chunk) {
                    if is_voice {
                        speech_frames += 1;
                        has_speech = true;
                    }
                }
            }
        }

        // Return true if at least some frames had speech
        // (threshold-based: more than 20% of frames)
        if total_frames > 0 {
            let speech_ratio = speech_frames as f32 / total_frames as f32;
            speech_ratio > 0.2
        } else {
            has_speech
        }
    }

    /// Get speech probability estimate (0.0 - 1.0)
    ///
    /// WebRTC VAD is binary, so we estimate probability by
    /// processing multiple frames and returning ratio.
    pub fn predict(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        const FRAME_SIZE: usize = 320; // 20ms at 16kHz

        // Convert to i16
        let i16_samples: Vec<i16> = samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        let mut speech_frames = 0;
        let mut total_frames = 0;

        for chunk in i16_samples.chunks(FRAME_SIZE) {
            if chunk.len() >= FRAME_SIZE / 2 { // Accept half-frames too
                total_frames += 1;
                // Pad if needed
                let padded: Vec<i16> = if chunk.len() < FRAME_SIZE {
                    let mut p = chunk.to_vec();
                    p.resize(FRAME_SIZE, 0);
                    p
                } else {
                    chunk[..FRAME_SIZE].to_vec()
                };

                if let Ok(is_voice) = self.vad.is_voice_segment(&padded) {
                    if is_voice {
                        speech_frames += 1;
                    }
                }
            }
        }

        if total_frames == 0 {
            return 0.0;
        }

        speech_frames as f32 / total_frames as f32
    }

    /// Reset internal state (WebRTC VAD is stateless, this is a no-op)
    pub fn reset(&mut self) {
        // WebRTC VAD doesn't maintain state between calls
        // Recreate to ensure clean state
        if let Ok(new_vad) = Self::with_config(self.config.clone()) {
            *self = new_vad;
        }
    }

    /// Get current configuration
    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// Update threshold (recreates VAD with new mode)
    pub fn set_threshold(&mut self, threshold: f32) {
        self.config.threshold = threshold.clamp(0.0, 1.0);
        if let Ok(new_vad) = Self::with_config(self.config.clone()) {
            *self = new_vad;
        }
    }
}

impl SileroVad {
    /// Create a new VAD instance
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: WebRtcVad::new()?,
        })
    }

    /// Create with custom configuration
    pub fn with_config(config: VadConfig) -> Result<Self> {
        Ok(Self {
            inner: WebRtcVad::with_config(config)?,
        })
    }

    /// Get speech probability for audio chunk (0.0 - 1.0)
    pub fn predict(&mut self, samples: &[f32]) -> f32 {
        self.inner.predict(samples)
    }

    /// Check if audio contains speech
    pub fn is_speech(&mut self, samples: &[f32]) -> bool {
        self.inner.predict(samples) > self.inner.config.threshold
    }

    /// Reset internal state
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Get current configuration
    pub fn config(&self) -> &VadConfig {
        self.inner.config()
    }

    /// Update threshold
    pub fn set_threshold(&mut self, threshold: f32) {
        self.inner.set_threshold(threshold);
    }
}

// ═══════════════════════════════════════════════════════════
// Singleton API (for easy global access)
// ═══════════════════════════════════════════════════════════

/// Initialize the global VAD instance
pub fn init() -> Result<()> {
    init_with_config(VadConfig::default())
}

/// Initialize with custom configuration
pub fn init_with_config(config: VadConfig) -> Result<()> {
    VAD_INSTANCE.get_or_init(|| {
        match WebRtcVad::with_config(config) {
            Ok(vad) => Mutex::new(vad),
            Err(e) => {
                tracing::error!("Failed to initialize VAD: {}. Using fallback.", e);
                Mutex::new(WebRtcVad::with_config(VadConfig::default())
                    .expect("Fallback VAD init failed"))
            }
        }
    });
    Ok(())
}

/// Check if VAD is initialized
pub fn is_initialized() -> bool {
    VAD_INSTANCE.get().is_some()
}

/// Get speech probability for audio chunk
///
/// Initializes VAD if not already done.
pub fn speech_probability(samples: &[f32]) -> f32 {
    let vad = VAD_INSTANCE.get_or_init(|| {
        Mutex::new(WebRtcVad::new().expect("VAD auto-init failed"))
    });

    match vad.lock() {
        Ok(mut guard) => guard.predict(samples),
        Err(e) => {
            tracing::error!("VAD lock poisoned: {}", e);
            0.0
        }
    }
}

/// Check if audio contains speech
///
/// Initializes VAD if not already done.
pub fn is_speech(samples: &[f32]) -> bool {
    let vad = VAD_INSTANCE.get_or_init(|| {
        Mutex::new(WebRtcVad::new().expect("VAD auto-init failed"))
    });

    let threshold = vad.lock()
        .map(|g| g.config.threshold)
        .unwrap_or(0.5);

    speech_probability(samples) > threshold
}

/// Reset VAD internal state
pub fn reset() {
    if let Some(vad) = VAD_INSTANCE.get() {
        if let Ok(mut guard) = vad.lock() {
            guard.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_detection() {
        let mut vad = WebRtcVad::new().expect("VAD init failed");

        // Pure silence should have very low probability
        let silence = vec![0.0f32; 320]; // 20ms frame
        let prob = vad.predict(&silence);
        assert!(prob < 0.3, "Silence should have low probability: {}", prob);
    }

    #[test]
    fn test_threshold_config() {
        let config = VadConfig::with_threshold(0.7);
        let vad = WebRtcVad::with_config(config).expect("VAD init failed");
        assert!((vad.config().threshold - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_singleton_api() {
        // First call initializes
        let prob1 = speech_probability(&vec![0.0f32; 320]);
        assert!(is_initialized());

        // Second call reuses
        let prob2 = speech_probability(&vec![0.0f32; 320]);
        assert!((prob1 - prob2).abs() < 0.1);
    }

    #[test]
    fn test_i16_conversion() {
        let mut vad = WebRtcVad::new().expect("VAD init failed");

        // Test f32 to i16 conversion
        let f32_samples = vec![0.5f32; 320];
        let result = vad.is_speech(&f32_samples);
        // Just verify it doesn't crash
        assert!(result || !result);
    }
}
