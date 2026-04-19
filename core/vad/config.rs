//! VAD configuration.
//!
//! Created by M&K (c)2026 VetCoders

use std::env;

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

    /// Silence run at or below this duration is treated as an in-utterance gap.
    pub utterance_gap_threshold_sec: f32,

    /// Silence run at the end of the recording at or above this duration
    /// becomes `TrailingSilence` and can drop Whisper tail hallucinations.
    pub tail_silence_threshold_sec: f32,

    /// When disabled, the Whisper post-filter keeps segments even if Silero
    /// classified their window as trailing silence.
    pub tail_drop_enabled: bool,
}

pub const SILERO_DEFAULT_THRESHOLD: f32 = 0.5;
pub const SILERO_DEFAULT_MIN_SPEECH_SEC: f32 = 0.064;
pub const SILERO_DEFAULT_MAX_SILENCE_SEC: f32 = 0.3;
pub const SILERO_DEFAULT_MAX_UTTERANCE_SEC: f32 = 30.0;
pub const SILERO_DEFAULT_PRE_ROLL_SEC: f32 = 0.064;
pub const SILERO_DEFAULT_UTTERANCE_GAP_SEC: f32 = 0.5;
pub const SILERO_DEFAULT_TAIL_SILENCE_SEC: f32 = 2.0;
pub const SILERO_DEFAULT_TAIL_DROP_ENABLED: bool = true;

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: SILERO_DEFAULT_THRESHOLD,
            min_speech_duration_sec: SILERO_DEFAULT_MIN_SPEECH_SEC,
            max_silence_duration_sec: SILERO_DEFAULT_MAX_SILENCE_SEC,
            max_utterance_sec: SILERO_DEFAULT_MAX_UTTERANCE_SEC,
            pre_roll_sec: SILERO_DEFAULT_PRE_ROLL_SEC,
            utterance_gap_threshold_sec: env_f32(
                "CODESCRIBE_UTTERANCE_GAP_SEC",
                SILERO_DEFAULT_UTTERANCE_GAP_SEC,
            )
            .max(0.0),
            tail_silence_threshold_sec: env_f32(
                "CODESCRIBE_TAIL_SILENCE_SEC",
                SILERO_DEFAULT_TAIL_SILENCE_SEC,
            )
            .max(0.0),
            tail_drop_enabled: env_bool(
                "CODESCRIBE_TAIL_DROP_ENABLED",
                SILERO_DEFAULT_TAIL_DROP_ENABLED,
            ),
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
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests are serialized and intentionally mutate process env.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests are serialized and intentionally mutate process env.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                // SAFETY: tests are serialized and intentionally mutate process env.
                unsafe { std::env::set_var(self.key, prev) };
            } else {
                // SAFETY: tests are serialized and intentionally mutate process env.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    #[test]
    #[serial]
    fn test_default_config() {
        let _gap = EnvGuard::unset("CODESCRIBE_UTTERANCE_GAP_SEC");
        let _tail = EnvGuard::unset("CODESCRIBE_TAIL_SILENCE_SEC");
        let _drop = EnvGuard::unset("CODESCRIBE_TAIL_DROP_ENABLED");
        let config = VadConfig::default();
        assert!((config.threshold - SILERO_DEFAULT_THRESHOLD).abs() < f32::EPSILON);
        assert!(
            (config.min_speech_duration_sec - SILERO_DEFAULT_MIN_SPEECH_SEC).abs() < f32::EPSILON
        );
        assert!(
            (config.max_silence_duration_sec - SILERO_DEFAULT_MAX_SILENCE_SEC).abs() < f32::EPSILON
        );
        assert!((config.max_utterance_sec - SILERO_DEFAULT_MAX_UTTERANCE_SEC).abs() < f32::EPSILON);
        assert!((config.pre_roll_sec - SILERO_DEFAULT_PRE_ROLL_SEC).abs() < f32::EPSILON);
        assert!(
            (config.utterance_gap_threshold_sec - SILERO_DEFAULT_UTTERANCE_GAP_SEC).abs()
                < f32::EPSILON
        );
        assert!(
            (config.tail_silence_threshold_sec - SILERO_DEFAULT_TAIL_SILENCE_SEC).abs()
                < f32::EPSILON
        );
        assert_eq!(config.tail_drop_enabled, SILERO_DEFAULT_TAIL_DROP_ENABLED);
    }

    #[test]
    #[serial]
    fn test_sensitive_vs_conservative() {
        let _gap = EnvGuard::unset("CODESCRIBE_UTTERANCE_GAP_SEC");
        let _tail = EnvGuard::unset("CODESCRIBE_TAIL_SILENCE_SEC");
        let _drop = EnvGuard::unset("CODESCRIBE_TAIL_DROP_ENABLED");
        let sensitive = VadConfig::sensitive();
        let conservative = VadConfig::conservative();
        assert!(sensitive.threshold < conservative.threshold);
    }

    #[test]
    #[serial]
    fn tail_silence_env_overrides_are_honored() {
        let _gap = EnvGuard::set("CODESCRIBE_UTTERANCE_GAP_SEC", "0.75");
        let _tail = EnvGuard::set("CODESCRIBE_TAIL_SILENCE_SEC", "3.5");
        let _drop = EnvGuard::set("CODESCRIBE_TAIL_DROP_ENABLED", "0");

        let config = VadConfig::default();
        assert!((config.utterance_gap_threshold_sec - 0.75).abs() < f32::EPSILON);
        assert!((config.tail_silence_threshold_sec - 3.5).abs() < f32::EPSILON);
        assert!(!config.tail_drop_enabled);
    }
}
