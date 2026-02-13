//! Configuration module for CodeScribe Rust app.
//!
//! Manages persistent settings with a single source of truth:
//! 1. .env file for all configuration
//!
//! Settings are stored in `$HOME/.codescribe/` directory by default.
//!
//! ## Module Structure
//!
//! - `types` - Type definitions (enums, Config struct)
//! - `defaults` - Default value functions for serde
//! - `loader` - Load/save functionality (.env, JSON)
//!
//! Note: Config is loaded via `Config::load()` and accessed via shared state in main.rs.

mod defaults;
pub mod keychain;
mod loader;
pub mod migrate;
pub mod models;
pub mod prompts;
pub mod settings;
mod types;

// Re-export types
pub use types::{Config, HoldMods, OverlayPositionMode, ToggleTrigger, TranscriptSendMode};
// Language re-exported for external consumers (GUI apps)
pub use settings::UserSettings;
pub use types::Language;

// Re-export prompts API (public API for GUI apps)
pub use prompts::{
    DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, get_assistive_prompt,
    get_assistive_prompt_path, get_formatting_prompt, get_formatting_prompt_path, open_prompt_file,
    open_prompts_folder, reset_to_defaults,
};

#[cfg(test)]
mod tests {
    use super::models;
    use super::*;
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.hold_mods, HoldMods::Fn);
        assert_eq!(config.whisper_language, Language::Polish); // Polish is default
        assert_eq!(config.ai_max_tokens, 0); // 0 = no limit (API decides)
        assert!(!config.ai_formatting_enabled);
        assert_eq!(config.double_tap_interval_ms, 200);
        assert_eq!(config.toggle_silence_sec, 5.0);
        assert_eq!(config.local_model, models::DEFAULT_MODEL);
    }

    #[test]
    fn test_hold_mods_parsing() {
        assert_eq!("ctrl".parse::<HoldMods>(), Ok(HoldMods::Ctrl));
        assert_eq!("ctrl_alt".parse::<HoldMods>(), Ok(HoldMods::CtrlAlt));
        assert_eq!("ctrl+shift".parse::<HoldMods>(), Ok(HoldMods::CtrlShift));
        assert_eq!("none".parse::<HoldMods>(), Ok(HoldMods::None));
        assert!("invalid".parse::<HoldMods>().is_err());
    }

    #[test]
    fn test_language_parsing() {
        assert_eq!("pl".parse::<Language>(), Ok(Language::Polish));
        assert_eq!("en".parse::<Language>(), Ok(Language::English));
        // "auto" maps to Polish (legacy compatibility)
        assert_eq!("auto".parse::<Language>(), Ok(Language::Polish));
        assert!("invalid".parse::<Language>().is_err());
    }

    #[test]
    fn test_token_limits_not_overridden() {
        // Token limits: 0 = no limit. Sanitize should NOT override.
        let mut config = Config {
            ai_max_tokens: 0,
            ..Default::default()
        };
        config.sanitize();
        assert_eq!(config.ai_max_tokens, 0); // Stays 0, not overridden
    }

    #[test]
    fn test_sanitize_sound_volume() {
        let mut config = Config {
            sound_volume: 1.5,
            ..Default::default()
        };
        config.sanitize();
        assert_eq!(config.sound_volume, 1.0);

        config.sound_volume = -0.5;
        config.sanitize();
        assert_eq!(config.sound_volume, 0.0);
    }

    #[test]
    fn test_config_dir() {
        let dir = Config::config_dir();
        assert!(dir.to_string_lossy().contains(".codescribe"));
    }

    #[test]
    fn test_env_file_parse_write() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create temporary .env file
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "# Comment line").unwrap();
        writeln!(temp_file, "KEY1=value1").unwrap();
        writeln!(temp_file, "KEY2=\"value2\"").unwrap();
        writeln!(temp_file).unwrap();
        writeln!(temp_file, "KEY3=value3").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();
        let vars = Config::parse_env_file(&path).unwrap();

        assert_eq!(vars.get("KEY1"), Some(&"value1".to_string()));
        assert_eq!(vars.get("KEY2"), Some(&"value2".to_string()));
        assert_eq!(vars.get("KEY3"), Some(&"value3".to_string()));
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn test_transcript_mode_parsing() {
        use types::TranscriptSendMode;
        assert_eq!(
            "streaming".parse::<TranscriptSendMode>(),
            Ok(TranscriptSendMode::Streaming)
        );
        assert_eq!(
            "end_of_utterance".parse::<TranscriptSendMode>(),
            Ok(TranscriptSendMode::EndOfUtterance)
        );
        assert_eq!(
            "end".parse::<TranscriptSendMode>(),
            Ok(TranscriptSendMode::EndOfUtterance)
        );
    }

    #[test]
    fn test_overlay_position_parsing() {
        use types::OverlayPositionMode;
        assert_eq!(
            "snapped_top_right".parse::<OverlayPositionMode>(),
            Ok(OverlayPositionMode::SnappedTopRight)
        );
        assert_eq!(
            "custom".parse::<OverlayPositionMode>(),
            Ok(OverlayPositionMode::Custom)
        );
    }
}
