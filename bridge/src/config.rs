//! Configuration surface — thin UniFFI wrapper over the live codescribe
//! `config` engine (settings.json / .env tiering, Keychain-backed API keys,
//! prompt files, onboarding state). Split out of `lib.rs` in W3 cut #0 so each
//! bridge slice owns a disjoint file.
//!
//! Sync-only (NOT tokio): every call here is cheap disk / Keychain / env I/O.
//! Secrets NEVER cross the FFI boundary — only `CsKeyStatus` booleans report
//! whether a key is present.

use std::fs;
use std::str::FromStr;

use codescribe_core::config::keychain::{KEYCHAIN_ACCOUNTS, delete_key, save_key};
use codescribe_core::config::prompts::{get_assistive_prompt_path, get_formatting_prompt_path};
use codescribe_core::config::{
    Config, DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, UserSettings, reset_to_defaults,
};
use codescribe_core::llm::model_discovery::{
    ModelDiscoveryStatus, discover_models as discover_provider_models,
};
use codescribe_core::llm::provider::{ALL_PROVIDERS, ProviderKind};

use crate::{CsError, CsLanguage};

/// Full settings snapshot pushed to the Swift Settings UI. Combines real
/// `Config` struct fields (settings.json / .env / defaults already merged by
/// `Config::load()`) with env-only knobs the core reads via `std::env::var`
/// after load (LLM model/endpoint overrides, formatting level, voice-lab).
///
/// API keys are intentionally absent — they live only in `CsKeyStatus` as
/// booleans. Write back through `update_config` / `update_config_many` using the
/// router env keys (see `CodescribeConfig::update_config`).
#[derive(uniffi::Record)]
pub struct CsSettings {
    // ── Hotkeys ──
    pub hold_exclusive: bool,
    pub hold_start_delay_ms: u64,
    pub double_tap_interval_ms: u64,
    pub toggle_silence_sec: f32,
    // ── Language ──
    pub whisper_language: CsLanguage,
    // ── AI / formatting ──
    pub ai_formatting_enabled: bool,
    /// `TranscriptSendMode::as_str()` — `"end_of_utterance"` / `"streaming"`.
    pub transcript_send_mode: String,
    pub transcript_tagging_enabled: bool,
    pub transcript_tag_template: String,
    pub ai_max_tokens: i32,
    pub ai_assistive_max_tokens: i32,
    // ── UI ──
    pub show_tray_glyph: bool,
    pub show_dock_icon: bool,
    pub transcription_overlay_enabled: bool,
    pub hold_indicator: bool,
    pub hold_badge_size: u32,
    pub hold_badge_offset_x: i32,
    pub hold_badge_offset_y: i32,
    /// `OverlayPositionMode::as_str()` — `"snapped_top_right"` / `"custom"`.
    pub overlay_position_mode: String,
    pub overlay_custom_x: Option<f64>,
    pub overlay_custom_y: Option<f64>,
    // ── Sound ──
    pub beep_on_start: bool,
    pub sound_name: String,
    pub sound_volume: f32,
    // ── Audio ──
    pub audio_input_device: Option<String>,
    // ── History / quick notes ──
    pub history_enabled: bool,
    pub quick_notes_enabled: bool,
    pub quick_notes_save_only: bool,
    // ── STT backend ──
    pub use_local_stt: bool,
    pub local_model: String,
    pub stt_endpoint: Option<String>,
    /// STT engine selection (`CODESCRIBE_STT_ENGINE`): `"auto"` | `"apple"` |
    /// `"whisper"`. `None` means the built-in auto policy. Written back via
    /// `update_config` with the same key (promoted → settings.json).
    pub stt_engine: Option<String>,
    // ── LLM backend (base) ──
    pub llm_endpoint: Option<String>,
    // ── Clipboard ──
    pub restore_clipboard: bool,
    pub restore_clipboard_delay_ms: u64,
    // ── System / agent ──
    pub start_at_login: bool,
    pub agent_enter_sends: bool,
    pub dump_audio_logs: bool,
    // ── Env-only knobs (not Config struct fields; read after load) ──
    pub llm_model: Option<String>,
    pub llm_formatting_endpoint: Option<String>,
    pub llm_formatting_model: Option<String>,
    pub llm_assistive_endpoint: Option<String>,
    pub llm_assistive_model: Option<String>,
    /// Assistive/agent-lane provider identity (`LLM_ASSISTIVE_PROVIDER`):
    /// `"openai-responses"` | `"anthropic-messages"`. Written back via
    /// `update_config` with the same key; drives `create_default_provider`.
    pub llm_assistive_provider: Option<String>,
    pub formatting_level: Option<String>,
    pub whisper_model: Option<String>,
    /// Layered incremental transcription phase (`CODESCRIBE_LAYERED_TRANSCRIPTION`):
    /// `"phase1"` | `"off"` (anything non-phase means OFF). Written back via
    /// `update_config` with the same key (promoted → settings.json).
    pub layered_transcription: Option<String>,
    /// Workspace root directories the agent scans (`list_projects` tool) to
    /// resolve project names to paths (`AGENT_WORKSPACE_ROOTS`, colon-joined on
    /// the wire). Effective value: never empty here — defaults to `["~/Git"]` so
    /// the Settings UI always shows the root the tool will actually scan. Written
    /// back via `update_config` with the same key (env-managed, NOT promoted).
    pub agent_workspace_roots: Vec<String>,
    pub buffer_delay_ms: Option<u64>,
    pub typing_cps: Option<f32>,
    pub emit_words_max: Option<u64>,
    pub buffered_interim_sec: Option<f32>,
    pub backend_max_upload_mb: Option<u64>,
}

/// Presence-only view of the Keychain-backed API keys. Booleans only — the
/// secret values themselves never cross FFI. A key counts as "set" when its
/// account env var is present and non-empty after `Config::load()` (which calls
/// `populate_env_from_keychain`).
#[derive(uniffi::Record)]
pub struct CsKeyStatus {
    pub llm_api_key_set: bool,
    pub stt_api_key_set: bool,
    pub llm_formatting_api_key_set: bool,
    pub llm_assistive_api_key_set: bool,
    /// Anthropic assistive-lane key (`LLM_ANTHROPIC_API_KEY`) — separate from the
    /// OpenAI assistive key so both providers can be configured at once.
    pub llm_anthropic_api_key_set: bool,
    pub github_token_set: bool,
}

/// One selectable model for a provider (id sent on the wire + display label).
#[derive(uniffi::Record)]
pub struct CsModelOption {
    pub id: String,
    pub display_name: String,
}

/// Live model discovery result for one provider. `status` is one of:
/// `"fresh"`, `"cached"`, `"no_key"`, `"error"`. Errors never carry secrets.
#[derive(uniffi::Record)]
pub struct CsModelDiscovery {
    pub provider_id: String,
    pub status: String,
    pub message: Option<String>,
    pub models: Vec<CsModelOption>,
}

/// One assistive/agent-lane provider option: canonical id, label, the Keychain
/// account holding its key (+ whether that key is present), and its model
/// catalog. Provider identity is static; models are discovered by
/// `discover_models` from the live provider API using the user's key.
#[derive(uniffi::Record)]
pub struct CsProviderOption {
    /// `LLM_ASSISTIVE_PROVIDER` value: `"openai-responses"` | `"anthropic-messages"`.
    pub id: String,
    pub display_name: String,
    /// Keychain account for this provider's assistive key.
    pub api_key_account: String,
    /// True when that key is present (mirrors `CsKeyStatus`, keyed per provider).
    pub api_key_set: bool,
    /// Always empty for live Settings; retained for bridge compatibility with
    /// older Swift bindings and preview seed objects.
    pub models: Vec<CsModelOption>,
}

/// One config key/value pair for `update_config_many` batch writes. `key` is a
/// router env key (e.g. `"WHISPER_LANGUAGE"`, `"USE_LOCAL_STT"`); `value` is the
/// string form the core parses (bool `"1"`/`"0"`, f32 `"1.00"`, etc.).
#[derive(uniffi::Record)]
pub struct CsConfigEntry {
    pub key: String,
    pub value: String,
}

/// Prompt-free tray subset. Reads only settings.json/defaults and does not touch
/// Keychain-backed API keys.
#[derive(uniffi::Record)]
pub struct CsTrayToggles {
    pub show_dock_icon: bool,
    pub transcription_overlay_enabled: bool,
    /// Notes Mode: voice → daily note (no paste). Backed by the core
    /// `quick_notes_enabled` + `quick_notes_save_only` pair, flipped together.
    pub notes_mode_enabled: bool,
}

/// Thin handle to the codescribe config engine. Stateless: each method reloads
/// or writes through the live `Config` / `UserSettings` / Keychain so Swift
/// always sees on-disk truth.
#[derive(uniffi::Object)]
pub struct CodescribeConfig {}

#[uniffi::export]
impl CodescribeConfig {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self {}
    }

    /// Full settings snapshot for the Settings UI. Reloads from disk so it
    /// reflects any writes made since construction.
    pub fn load_settings(&self) -> CsSettings {
        let config = Config::load();
        CsSettings {
            hold_exclusive: config.hold_exclusive,
            hold_start_delay_ms: config.hold_start_delay_ms,
            double_tap_interval_ms: config.double_tap_interval_ms,
            toggle_silence_sec: config.toggle_silence_sec,
            whisper_language: CsLanguage::from(config.whisper_language),
            ai_formatting_enabled: config.ai_formatting_enabled,
            transcript_send_mode: config.transcript_send_mode.as_str().to_string(),
            transcript_tagging_enabled: config.transcript_tagging_enabled,
            transcript_tag_template: config.transcript_tag_template.clone(),
            ai_max_tokens: config.ai_max_tokens,
            ai_assistive_max_tokens: config.ai_assistive_max_tokens,
            show_tray_glyph: config.show_tray_glyph,
            show_dock_icon: config.show_dock_icon,
            transcription_overlay_enabled: config.transcription_overlay_enabled,
            hold_indicator: config.hold_indicator,
            hold_badge_size: config.hold_badge_size,
            hold_badge_offset_x: config.hold_badge_offset_x,
            hold_badge_offset_y: config.hold_badge_offset_y,
            overlay_position_mode: config.overlay_position_mode.as_str().to_string(),
            overlay_custom_x: config.overlay_custom_x,
            overlay_custom_y: config.overlay_custom_y,
            beep_on_start: config.beep_on_start,
            sound_name: config.sound_name.clone(),
            sound_volume: config.sound_volume,
            audio_input_device: config.audio_input_device.clone(),
            history_enabled: config.history_enabled,
            quick_notes_enabled: config.quick_notes_enabled,
            quick_notes_save_only: config.quick_notes_save_only,
            use_local_stt: config.use_local_stt,
            local_model: config.local_model.clone(),
            stt_endpoint: config.stt_endpoint.clone(),
            stt_engine: env_string("CODESCRIBE_STT_ENGINE"),
            llm_endpoint: config.llm_endpoint.clone(),
            restore_clipboard: config.restore_clipboard,
            restore_clipboard_delay_ms: config.restore_clipboard_delay_ms,
            start_at_login: config.start_at_login,
            agent_enter_sends: config.agent_enter_sends,
            dump_audio_logs: config.dump_audio_logs,
            // Env-only knobs (read after Config::load populated the env).
            llm_model: env_string("LLM_MODEL"),
            llm_formatting_endpoint: env_string("LLM_FORMATTING_ENDPOINT"),
            llm_formatting_model: env_string("LLM_FORMATTING_MODEL"),
            llm_assistive_endpoint: env_string("LLM_ASSISTIVE_ENDPOINT"),
            llm_assistive_model: env_string("LLM_ASSISTIVE_MODEL"),
            llm_assistive_provider: env_string("LLM_ASSISTIVE_PROVIDER"),
            formatting_level: env_string("FORMATTING_LEVEL"),
            whisper_model: env_string("WHISPER_MODEL"),
            layered_transcription: env_string("CODESCRIBE_LAYERED_TRANSCRIPTION"),
            agent_workspace_roots: env_list("AGENT_WORKSPACE_ROOTS", DEFAULT_AGENT_WORKSPACE_ROOT),
            buffer_delay_ms: env_parse("CODESCRIBE_BUFFER_DELAY_MS"),
            typing_cps: env_parse("CODESCRIBE_TYPING_CPS"),
            emit_words_max: env_parse("CODESCRIBE_EMIT_WORDS_MAX"),
            buffered_interim_sec: env_parse("CODESCRIBE_BUFFERED_INTERIM_SEC"),
            backend_max_upload_mb: env_parse("BACKEND_MAX_UPLOAD_MB"),
        }
    }

    /// Lightweight tray-only settings read. Unlike `load_settings`, this never
    /// calls `Config::load()` and therefore never prompts Keychain just because
    /// the user opened the menu.
    pub fn tray_toggles(&self) -> CsTrayToggles {
        let settings = UserSettings::load();
        let defaults = Config::default();
        CsTrayToggles {
            show_dock_icon: settings.show_dock_icon.unwrap_or(defaults.show_dock_icon),
            transcription_overlay_enabled: settings
                .transcription_overlay_enabled
                .unwrap_or(defaults.transcription_overlay_enabled),
            // Notes Mode is "on" only when BOTH flags are set (dictation → note
            // AND no paste). Reading just quick_notes_enabled could show the toggle
            // ON while dictation still pastes (save_only=false) — an edge desync.
            notes_mode_enabled: settings
                .quick_notes_enabled
                .unwrap_or(defaults.quick_notes_enabled)
                && settings
                    .quick_notes_save_only
                    .unwrap_or(defaults.quick_notes_save_only),
        }
    }

    /// Persist one config value, auto-tiered by the core router
    /// (`save_to_env`): API keys → Keychain, promoted keys → settings.json,
    /// power-user keys → `.env`. Also updates the live process env.
    pub fn update_config(&self, key: String, value: String) -> Result<(), CsError> {
        Config::load()
            .save_to_env(&key, &value)
            .map_err(|error| CsError::Config {
                msg: error.to_string(),
            })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Toggle Notes Mode as one explicit two-key operation. Notes Mode means
    /// "dictation → daily note AND no paste", so `QUICK_NOTES_ENABLED` and
    /// `QUICK_NOTES_SAVE_ONLY` are written together in a single settings.json
    /// update — never two independent writes that could half-succeed and desync.
    pub fn set_notes_mode(&self, enabled: bool) -> Result<(), CsError> {
        let value = if enabled { "1" } else { "0" }.to_string();
        self.update_config_many(vec![
            CsConfigEntry {
                key: "QUICK_NOTES_ENABLED".to_string(),
                value: value.clone(),
            },
            CsConfigEntry {
                key: "QUICK_NOTES_SAVE_ONLY".to_string(),
                value,
            },
        ])
    }

    /// Batch variant of `update_config` (`save_to_env_many`) — one settings.json
    /// write and one `.env` rewrite for the whole batch.
    pub fn update_config_many(&self, entries: Vec<CsConfigEntry>) -> Result<(), CsError> {
        let pairs: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.key.as_str(), entry.value.as_str()))
            .collect();
        Config::load()
            .save_to_env_many(&pairs)
            .map_err(|error| CsError::Config {
                msg: error.to_string(),
            })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Absolute path to the config directory (`~/.codescribe`, or the
    /// `CODESCRIBE_DATA_DIR` override).
    pub fn config_dir(&self) -> String {
        Config::config_dir().to_string_lossy().to_string()
    }

    /// Presence booleans for every Keychain-backed API key.
    pub fn key_status(&self) -> CsKeyStatus {
        // This endpoint is explicitly about keys, so it may prompt. Construction
        // of SettingsViewModel/TrayViewModel should remain prompt-free.
        let _ = Config::load();
        CsKeyStatus {
            llm_api_key_set: key_present("LLM_API_KEY"),
            stt_api_key_set: key_present("STT_API_KEY"),
            llm_formatting_api_key_set: key_present("LLM_FORMATTING_API_KEY"),
            llm_assistive_api_key_set: key_present("LLM_ASSISTIVE_API_KEY"),
            llm_anthropic_api_key_set: key_present("LLM_ANTHROPIC_API_KEY"),
            github_token_set: key_present("GITHUB_TOKEN"),
        }
    }

    /// Assistive/agent-lane provider catalog with per-provider key presence.
    /// Model lists are intentionally empty here: Settings must call
    /// `discover_models` so dropdown options come from the provider's live API,
    /// not a static fallback.
    pub fn available_providers(&self) -> Vec<CsProviderOption> {
        let _ = Config::load();
        ALL_PROVIDERS
            .iter()
            .map(|kind| {
                let account = kind.api_key_env_key().to_string();
                CsProviderOption {
                    id: kind.as_str().to_string(),
                    display_name: kind.display_name().to_string(),
                    api_key_set: key_present(&account),
                    api_key_account: account,
                    models: Vec::new(),
                }
            })
            .collect()
    }

    /// Discover model options from the selected provider using the live provider
    /// `/models` API plus the existing config/Keychain/env key resolution path.
    /// Missing key is returned as a typed status, not as a thrown bridge error,
    /// so Settings can render "Add API key to discover models" inline.
    pub fn discover_models(&self, provider_id: String) -> CsModelDiscovery {
        let provider = match ProviderKind::from_str(&provider_id) {
            Ok(provider) => provider,
            Err(error) => {
                return CsModelDiscovery {
                    provider_id,
                    status: "error".to_string(),
                    message: Some(error.to_string()),
                    models: Vec::new(),
                };
            }
        };

        match discover_provider_models(provider) {
            Ok(result) => {
                let (status, message) = match result.status {
                    ModelDiscoveryStatus::Fresh => ("fresh".to_string(), None),
                    ModelDiscoveryStatus::Cached { reason } => ("cached".to_string(), Some(reason)),
                };
                CsModelDiscovery {
                    provider_id: result.provider.as_str().to_string(),
                    status,
                    message,
                    models: result
                        .models
                        .into_iter()
                        .map(|model| CsModelOption {
                            id: model.id,
                            display_name: model.display_name,
                        })
                        .collect(),
                }
            }
            Err(error) => {
                let status = if error.code() == "no_key" {
                    "no_key"
                } else {
                    "error"
                };
                CsModelDiscovery {
                    provider_id: error.provider().as_str().to_string(),
                    status: status.to_string(),
                    message: Some(error.message()),
                    models: Vec::new(),
                }
            }
        }
    }

    /// Canonical list of Keychain account names (`KEYCHAIN_ACCOUNTS`).
    pub fn key_accounts(&self) -> Vec<String> {
        KEYCHAIN_ACCOUNTS.iter().map(|a| a.to_string()).collect()
    }

    /// Store an API key in the Keychain and sync the live process env, exactly
    /// like the core's `save_to_env` path. `account` must be a known
    /// `KEYCHAIN_ACCOUNTS` entry. The secret is never echoed back.
    pub fn set_api_key(&self, account: String, secret: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        save_key(&account, &secret).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        // SAFETY: settings writes are serialized on a single Swift actor (W3
        // contract) and runtime readers consume refreshed Config snapshots, so
        // this mirrors the core's `ui_thread_set_env` after `save_key`.
        unsafe { std::env::set_var(&account, &secret) };
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Delete an API key from the Keychain and clear it from the live process
    /// env. `account` must be a known `KEYCHAIN_ACCOUNTS` entry.
    pub fn clear_api_key(&self, account: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        delete_key(&account).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        // SAFETY: same single-writer invariant as `set_api_key`.
        unsafe { std::env::remove_var(&account) };
        Ok(())
    }

    /// Current BASE formatting prompt (the editable `formatting.txt`, WITHOUT
    /// the appended `*_tuning.txt`). Falls back to the built-in default when the
    /// file does not exist yet.
    pub fn get_formatting_prompt(&self) -> String {
        read_prompt_or_default(
            &get_formatting_prompt_path().to_string_lossy(),
            DEFAULT_FORMATTING_PROMPT,
        )
    }

    /// Current BASE assistive prompt (the editable `assistive.txt`, WITHOUT the
    /// appended `*_tuning.txt`). Falls back to the built-in default when the file
    /// does not exist yet.
    pub fn get_assistive_prompt(&self) -> String {
        read_prompt_or_default(
            &get_assistive_prompt_path().to_string_lossy(),
            DEFAULT_ASSISTIVE_PROMPT,
        )
    }

    /// Overwrite the BASE formatting prompt file. The core has no setter, so we
    /// write `formatting.txt` directly (creating the prompts dir if needed).
    pub fn set_formatting_prompt(&self, content: String) -> Result<(), CsError> {
        write_prompt(&get_formatting_prompt_path(), &content)
    }

    /// Overwrite the BASE assistive prompt file (`assistive.txt`).
    pub fn set_assistive_prompt(&self, content: String) -> Result<(), CsError> {
        write_prompt(&get_assistive_prompt_path(), &content)
    }

    /// Reset both BASE prompt files to their built-in defaults
    /// (`reset_to_defaults`). Does not touch `*_tuning.txt`.
    pub fn reset_prompts_to_defaults(&self) -> Result<(), CsError> {
        reset_to_defaults().map_err(CsError::from)
    }

    /// Built-in default formatting prompt (`DEFAULT_FORMATTING_PROMPT`).
    pub fn default_formatting_prompt(&self) -> String {
        DEFAULT_FORMATTING_PROMPT.to_string()
    }

    /// Built-in default assistive prompt (`DEFAULT_ASSISTIVE_PROMPT`).
    pub fn default_assistive_prompt(&self) -> String {
        DEFAULT_ASSISTIVE_PROMPT.to_string()
    }

    /// Whether the first-run onboarding wizard should be shown (mirrors the
    /// live app gate; re-validates permission markers).
    pub fn should_show_onboarding(&self) -> bool {
        codescribe::should_show_onboarding()
    }

    /// Persisted resume step for the first-run wizard (0-based index into the
    /// canonical 12-step flow). Returns `0` (Welcome) when no progress marker
    /// exists yet, so a fresh install starts at the top.
    pub fn onboarding_progress(&self) -> u32 {
        codescribe::load_onboarding_progress() as u32
    }

    /// Persist the wizard's current step so a relaunch resumes where the user
    /// left off. Writer for the `onboarding_progress` marker that the live gate
    /// (`should_show_onboarding`) already reconciles against permission state.
    pub fn save_onboarding_progress(&self, step: u32) {
        codescribe::save_onboarding_progress(step as usize);
    }

    /// Mark onboarding complete: clears the resume marker and writes the
    /// canonical `setup_done` sentinel so `should_show_onboarding()` returns
    /// `false` on the next launch.
    pub fn mark_onboarding_done(&self) {
        codescribe::mark_onboarding_done();
    }

    /// First-run operating lane chosen during onboarding (`"basic"` /
    /// `"agentic"`), or `None` when not yet chosen.
    pub fn onboarding_mode(&self) -> Option<String> {
        UserSettings::load().onboarding_mode
    }

    /// Persist the onboarding operating lane. Routes to settings.json via the
    /// promoted `ONBOARDING_MODE` key.
    pub fn set_onboarding_mode(&self, mode: String) -> Result<(), CsError> {
        Config::load()
            .save_to_env("ONBOARDING_MODE", &mode)
            .map_err(|error| CsError::Config {
                msg: error.to_string(),
            })
    }
}

/// Re-seed the live hotkey detector atomics after a settings write so mode
/// binding / cadence changes take effect without an app restart. The CGEventTap
/// callback reads these atomics per-event (app/os/hotkeys/platform.rs), so a
/// fresh apply is a true live-reload. Idempotent and cheap (a few atomic stores);
/// applied unconditionally because every settings mutation funnels through
/// `update_config` / `update_config_many`, and re-applying unchanged values is a
/// no-op.
fn reload_hotkey_runtime() {
    codescribe::os::hotkeys::apply_hotkey_config(&Config::load());
}

/// Built-in default workspace root when `AGENT_WORKSPACE_ROOTS` is unset. Kept in
/// sync with the `list_projects` tool default (`app/agent/tools/workspace.rs`).
const DEFAULT_AGENT_WORKSPACE_ROOT: &str = "~/Git";

/// Colon-separated env var into a trimmed, non-empty `Vec<String>`. Falls back to
/// a single-element `[default]` when the var is unset/empty, so the Settings UI
/// always renders the effective root the agent tool will scan.
fn env_list(key: &str, default: &str) -> Vec<String> {
    let roots: Vec<String> = std::env::var(key)
        .ok()
        .map(|value| {
            value
                .split(':')
                .map(|segment| segment.trim().to_string())
                .filter(|segment| !segment.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if roots.is_empty() {
        vec![default.to_string()]
    } else {
        roots
    }
}

/// Non-empty env var as `Some(String)`, else `None`.
fn env_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Parse a non-empty env var into `T`, else `None`.
fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

/// True when the account env var is present and non-empty.
fn key_present(account: &str) -> bool {
    env_string(account).is_some()
}

/// Reject unknown Keychain accounts before touching the Keychain.
fn ensure_known_account(account: &str) -> Result<(), CsError> {
    if KEYCHAIN_ACCOUNTS.contains(&account) {
        Ok(())
    } else {
        Err(CsError::Config {
            msg: format!("unknown keychain account: {account}"),
        })
    }
}

/// Read a BASE prompt file, falling back to the supplied default when it is
/// missing or unreadable.
fn read_prompt_or_default(path: &str, default: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| default.to_string())
}

/// Write a BASE prompt file, creating its parent directory first.
fn write_prompt(path: &std::path::Path, content: &str) -> Result<(), CsError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}
