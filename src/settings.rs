//! Simple settings persistence for CodeScribe.
//!
//! This module provides a minimal settings structure stored in `~/.CodeScribe/settings.json`.
//! For the full configuration system, see the `config` module.

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Simple settings structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Transcription language setting
    pub language: String,

    /// Whether global hotkeys are enabled
    pub hotkeys_enabled: bool,

    /// Whether AI formatting is enabled
    pub formatting_enabled: bool,

    /// Whether sound feedback is enabled
    pub sound_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".to_string(),
            hotkeys_enabled: true,
            formatting_enabled: true,
            sound_enabled: true,
        }
    }
}

impl Settings {
    /// Load settings from `~/.CodeScribe/settings.json`.
    ///
    /// Returns default settings if the file doesn't exist or cannot be parsed.
    pub fn load() -> Self {
        let path = Self::settings_path();

        if !path.exists() {
            return Self::default();
        }

        match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str::<Settings>(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save settings to `~/.CodeScribe/settings.json`.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::settings_path();

        // Create directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Serialize settings to JSON with pretty formatting
        let json = serde_json::to_string_pretty(self)?;

        // Write to file
        fs::write(&path, json)?;

        Ok(())
    }

    /// Get the path to the settings file (`~/.CodeScribe/settings.json`).
    fn settings_path() -> PathBuf {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".CodeScribe").join("settings.json"))
            .unwrap_or_else(|| PathBuf::from(".CodeScribe/settings.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.language, "auto");
        assert!(settings.hotkeys_enabled);
        assert!(settings.formatting_enabled);
        assert!(settings.sound_enabled);
    }

    #[test]
    fn test_settings_path() {
        let path = Settings::settings_path();
        assert!(path.to_string_lossy().contains(".CodeScribe"));
        assert!(path.to_string_lossy().ends_with("settings.json"));
    }

    #[test]
    fn test_load_nonexistent() {
        // Loading from non-existent file should return defaults
        let settings = Settings::load();
        assert_eq!(settings.language, "auto");
    }
}
