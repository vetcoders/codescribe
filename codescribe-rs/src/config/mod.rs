//! Configuration module for CodeScribe Rust app.
//!
//! Manages persistent settings with dual-layer storage:
//! 1. .env file for all configuration (primary source)
//! 2. settings.json for backwards compatibility
//!
//! Settings are stored in `$HOME/.codescribe/` directory by default.
//! .env file takes precedence over settings.json when both exist.
//!
//! ## Module Structure
//!
//! - `types` - Type definitions (enums, Config struct)
//! - `defaults` - Default value functions for serde
//! - `loader` - Load/save functionality (.env, JSON)
//! - `global` - Thread-safe global configuration state
//!
//! Note: Global config API not yet wired up to main.rs (pending integration)
#![allow(dead_code)]

mod defaults;
mod global;
mod loader;
mod types;

// Re-export types
pub use types::{AiProvider, Config, HoldMods, Language, ToggleTrigger};

// Re-export global API
pub use global::{get, init, save, update};

// Re-export defaults for external use
pub use defaults::default_backend_ports;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.hold_mods, HoldMods::Ctrl);
        assert_eq!(config.whisper_language, Language::Auto);
        assert_eq!(config.ai_provider, AiProvider::Harmony);
        assert_eq!(config.ai_max_tokens, 512);
        assert!(!config.ai_formatting_enabled);
        assert_eq!(config.backend_ports, vec![8237, 7237, 6237, 5237]);
    }

    #[test]
    fn test_hold_mods_parsing() {
        assert_eq!("ctrl".parse::<HoldMods>(), Ok(HoldMods::Ctrl));
        assert_eq!("ctrl_alt".parse::<HoldMods>(), Ok(HoldMods::CtrlAlt));
        assert_eq!("ctrl+shift".parse::<HoldMods>(), Ok(HoldMods::CtrlShift));
        assert!("invalid".parse::<HoldMods>().is_err());
    }

    #[test]
    fn test_language_parsing() {
        assert_eq!("auto".parse::<Language>(), Ok(Language::Auto));
        assert_eq!("pl".parse::<Language>(), Ok(Language::Polish));
        assert_eq!("en".parse::<Language>(), Ok(Language::English));
        assert!("invalid".parse::<Language>().is_err());
    }

    #[test]
    fn test_sanitize_token_limits() {
        let mut config = Config::default();
        config.ai_max_tokens = -1;
        config.sanitize();
        assert_eq!(config.ai_max_tokens, 512);
    }

    #[test]
    fn test_sanitize_sound_volume() {
        let mut config = Config::default();
        config.sound_volume = 1.5;
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
        writeln!(temp_file, "").unwrap();
        writeln!(temp_file, "KEY3=value3").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();
        let vars = Config::parse_env_file(&path).unwrap();

        assert_eq!(vars.get("KEY1"), Some(&"value1".to_string()));
        assert_eq!(vars.get("KEY2"), Some(&"value2".to_string()));
        assert_eq!(vars.get("KEY3"), Some(&"value3".to_string()));
        assert_eq!(vars.len(), 3);
    }
}
