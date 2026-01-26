//! VAD configuration.
//!
//! Created by M&K (c)2026 VetCoders

/// Configuration for Silero VAD
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// Speech probability threshold (0.0 - 1.0)
    /// Higher = more conservative (fewer false positives)
    /// Lower = more sensitive (catches quiet speech)
    pub threshold: f32,

    /// Minimum speech duration in seconds before triggering
    /// Helps filter out brief sounds like clicks
    pub min_speech_duration_sec: f32,

    /// Maximum silence duration in seconds before ending speech segment
    pub max_silence_duration_sec: f32,

    /// Maximum utterance duration in seconds (force flush after this)
    pub max_utterance_sec: f32,

    /// Pre-roll duration in seconds to keep before speech onset
    pub pre_roll_sec: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: env_f32("CODESCRIBE_VAD_THRESHOLD", 0.5),
            min_speech_duration_sec: env_f32("CODESCRIBE_VAD_MIN_SPEECH_SEC", 0.1),
            max_silence_duration_sec: env_f32("CODESCRIBE_VAD_MAX_SILENCE_SEC", 0.8),
            max_utterance_sec: env_f32("CODESCRIBE_VAD_MAX_UTTERANCE_SEC", 30.0),
            pre_roll_sec: env_f32("CODESCRIBE_VAD_PRE_ROLL_SEC", 0.3),
        }
    }
}

impl VadConfig {
    /// Create config with custom threshold
    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            threshold,
            ..Default::default()
        }
    }

    /// Get min speech duration in milliseconds (for Silero API)
    pub fn min_speech_ms(&self) -> u64 {
        (self.min_speech_duration_sec * 1000.0) as u64
    }

    /// Get max silence duration in milliseconds (for Silero API)
    pub fn max_silence_ms(&self) -> u64 {
        (self.max_silence_duration_sec * 1000.0) as u64
    }

    /// More sensitive detection (catches quiet speech, more false positives)
    pub fn sensitive() -> Self {
        Self {
            threshold: 0.3,
            min_speech_duration_sec: 0.05,
            ..Default::default()
        }
    }

    /// More conservative detection (fewer false positives, may miss quiet speech)
    pub fn conservative() -> Self {
        Self {
            threshold: 0.7,
            min_speech_duration_sec: 0.2,
            ..Default::default()
        }
    }
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = VadConfig::default();
        assert!(config.threshold >= 0.0 && config.threshold <= 1.0);
        assert!(config.min_speech_duration_sec > 0.0);
        assert!(config.max_silence_duration_sec > 0.0);
    }

    #[test]
    fn test_sensitive_vs_conservative() {
        let sensitive = VadConfig::sensitive();
        let conservative = VadConfig::conservative();
        assert!(sensitive.threshold < conservative.threshold);
    }
}
