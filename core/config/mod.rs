//! Configuration module for Codescribe Rust app.
//!
//! Manages persistent settings with a tiered truth model:
//! 1. Code defaults define zero-state runtime behaviour
//! 2. `settings.json` is the canonical persisted store for user-facing settings
//! 3. `.env` is optional and only used for env-managed / developer overrides
//!
//! Runtime/user settings are stored under:
//! - `~/Library/Application Support/Codescribe/settings.json` on macOS
//! - `~/.codescribe/.env` only when an optional power-user env file exists
//!
//! ## Module Structure
//!
//! - `types` - Type definitions (enums, Config struct)
//! - `defaults` - Default value functions for serde
//! - `loader` - Load/save functionality (defaults, JSON, optional env)
//!
//! Note: Config is loaded via `Config::load()` and accessed via shared state in main.rs.

/// Layer 1 ASR product mode, audio-egress consent, and gateway mint config.
pub mod cloud_asr;
/// Serde default helpers and default model/endpoint constants.
mod defaults;
/// Stop-path final-pass routing mode shared by controller and bridge lanes.
pub mod final_pass;
/// macOS Keychain storage for API keys (not plaintext `.env`).
pub mod keychain;
/// Load/save: defaults, settings.json, optional `.env`, process env overrides.
mod loader;
/// One-time legacy `.env` import into settings.json + Keychain.
pub mod migrate;
/// Runtime Whisper fallback model path resolution when not embedded.
pub mod models;
/// Secret-free portable profile export/import with dry-run validation.
pub mod portable;
/// On-disk prompt files (formatting, smart formatting, assistive): built-in
/// defaults, user overrides, snapshots, and restore-to-default.
pub mod prompts;
/// GUI-managed user settings JSON (regular-user tier).
pub mod settings;
/// Process-wide app-data I/O fence used by destructive reset.
pub mod storage_reset;
/// Config enums and the main `Config` struct definitions.
mod types;

pub use defaults::{
    DEFAULT_ASSISTIVE_MODEL, DEFAULT_FORMATTING_MODEL, DEFAULT_LLM_MODEL,
    DEFAULT_OPENAI_RESPONSES_ENDPOINT, default_assistive_model, default_formatting_model,
    default_llm_endpoint, default_llm_endpoint_option, default_llm_model,
};
// Re-export types
pub use types::{
    Config, DeferredInsertShortcut, HoldArmModifier, ModeBinding, OverlayPositionMode,
    ShortcutBinding, TranscriptSendMode, WorkMode,
};
// Language re-exported for external consumers (GUI apps)
pub use cloud_asr::{
    AsrProductMode, AudioEgressConsent, ConsentSource, GatewayMintError, GatewaySessionMint,
    ModeDerivation, ResolvedAsrMode, resolve_asr_product_mode,
};
pub use final_pass::{FinalPassRoutingMode, final_pass_routing_mode};
pub use portable::{
    ImportPlan, PortableProfile, export_portable, import_portable_apply, import_portable_dry_run,
    write_portable_export,
};
pub use settings::{FormattingPolicy, UserSettings};
pub use storage_reset::{AppDataResetGuard, begin_app_data_reset};
pub use types::Language;

// Re-export prompts API (public API for GUI apps)
pub use prompts::{
    DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, DEFAULT_MAX_FORMATTING_PROMPT,
    DEFAULT_SMART_FORMATTING_PROMPT, PromptKind, PromptSnapshot, PromptSource, PromptWriteReason,
    get_assistive_prompt, get_assistive_prompt_path, get_formatting_prompt,
    get_formatting_prompt_for_policy, get_formatting_prompt_path,
    get_formatting_prompt_path_for_policy, open_prompt_file, open_prompts_folder, prompt_snapshot,
    reset_to_defaults, restore_prompt_to_default, write_prompt, write_prompt_bytes,
    write_prompt_bytes_during_reset,
};

#[cfg(test)]
/// Config module unit tests (defaults, env overrides, parsing, sanitize).
mod tests {
    use super::models;
    use super::*;
    use serial_test::serial;
    /// Zero-state `Config::default` matches canonical defaults and flags.
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.whisper_language, Language::Auto);
        assert_eq!(config.ai_max_tokens, 0); // 0 = no limit (API decides)
        assert!(!config.ai_formatting_enabled);
        assert!(!config.transcript_tagging_enabled);
        assert_eq!(config.double_tap_interval_ms, 200);
        assert_eq!(config.toggle_silence_sec, 5.0);
        assert!(config.show_dock_icon);
        assert_eq!(config.local_model, models::DEFAULT_MODEL);
    }

    /// `SHOW_DOCK_ICON=0` forces `show_dock_icon` false on load (restores env).
    #[test]
    #[serial_test::serial]
    fn test_show_dock_icon_env_override_applies() {
        let previous = std::env::var("SHOW_DOCK_ICON").ok();
        unsafe { std::env::set_var("SHOW_DOCK_ICON", "0") };

        let config = Config::load();
        assert!(!config.show_dock_icon);

        if let Some(value) = previous {
            unsafe { std::env::set_var("SHOW_DOCK_ICON", value) };
        } else {
            unsafe { std::env::remove_var("SHOW_DOCK_ICON") };
        }
    }

    /// Transcript tagging env vars override enable flag and template on load.
    #[test]
    #[serial_test::serial]
    fn test_transcript_tagging_env_override_applies() {
        let previous_enabled = std::env::var("CODESCRIBE_TRANSCRIPT_TAGGING").ok();
        let previous_template = std::env::var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE").ok();
        unsafe {
            std::env::set_var("CODESCRIBE_TRANSCRIPT_TAGGING", "1");
            std::env::set_var(
                "CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE",
                "[{mode}/{lang}] {text}",
            );
        }

        let config = Config::load();
        assert!(config.transcript_tagging_enabled);
        assert_eq!(config.transcript_tag_template, "[{mode}/{lang}] {text}");

        match previous_enabled {
            Some(value) => unsafe { std::env::set_var("CODESCRIBE_TRANSCRIPT_TAGGING", value) },
            None => unsafe { std::env::remove_var("CODESCRIBE_TRANSCRIPT_TAGGING") },
        }
        match previous_template {
            Some(value) => unsafe {
                std::env::set_var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE", value)
            },
            None => unsafe { std::env::remove_var("CODESCRIBE_TRANSCRIPT_TAG_TEMPLATE") },
        }
    }

    /// Language string aliases parse to Auto / Polish / English; invalid errs.
    #[test]
    fn test_language_parsing() {
        assert_eq!("auto".parse::<Language>(), Ok(Language::Auto));
        assert_eq!("detect".parse::<Language>(), Ok(Language::Auto));
        assert_eq!("multilingual".parse::<Language>(), Ok(Language::Auto));
        assert_eq!("pl".parse::<Language>(), Ok(Language::Polish));
        assert_eq!("en".parse::<Language>(), Ok(Language::English));
        assert!("invalid".parse::<Language>().is_err());
    }

    /// `ai_max_tokens = 0` means no limit; sanitize must leave it unchanged.
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

    /// Sound volume clamps into [0.0, 1.0] during sanitize.
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

    /// `config_dir` respects `CODESCRIBE_DATA_DIR` or falls under `.codescribe`.
    #[test]
    #[serial]
    fn test_config_dir() {
        // #[serial]: reads the global CODESCRIBE_DATA_DIR env var, which the
        // setup_isolated_data_dir() tests mutate — without serialization this races
        // and flakes (config_dir() vs env::var() observing different values).
        let dir = Config::config_dir();
        if let Ok(custom) = std::env::var("CODESCRIBE_DATA_DIR") {
            assert_eq!(dir, std::path::PathBuf::from(custom));
        } else {
            assert!(dir.to_string_lossy().contains(".codescribe"));
        }
    }

    /// `.env` parser skips comments/blanks and strips surrounding quotes.
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

    /// Transcript send-mode strings map to Streaming / EndOfUtterance.
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

    /// Overlay position mode strings map to snapped / custom variants.
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
