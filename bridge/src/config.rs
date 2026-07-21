//! Configuration surface — thin UniFFI wrapper over the live codescribe
//! `config` engine (settings.json / .env tiering, Keychain-backed API keys,
//! prompt files, onboarding state). Split out of `lib.rs` in W3 cut #0 so each
//! bridge slice owns a disjoint file.
//!
//! Sync-only (NOT tokio): every call here is cheap disk / Keychain / env I/O.
//! Secrets NEVER cross the FFI boundary — only `CsKeyStatus` booleans report
//! whether a key is present.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, Once, OnceLock};

use chrono::{DateTime, SecondsFormat, Utc};
use codescribe_core::config::keychain::{KEYCHAIN_ACCOUNTS, delete_key, save_key};
use codescribe_core::config::{
    Config, DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, FormattingPolicy, PromptKind,
    PromptSnapshot, PromptWriteReason, UserSettings, prompt_snapshot, prompts, reset_to_defaults,
    restore_prompt_to_default, write_prompt, write_prompt_bytes,
};
use codescribe_core::llm::account_auth;
use codescribe_core::llm::key_liveness::{
    ApiKeyLivenessResult, ApiKeyLivenessStatus, probe_api_key_liveness,
};
use codescribe_core::llm::lane_truth;
use codescribe_core::llm::model_discovery::{
    ModelDiscoveryStatus, discover_models as discover_provider_models,
};
use codescribe_core::llm::provider::{ALL_PROVIDERS, ProviderKind};
use directories::BaseDirs;

use crate::{CsError, CsLanguage};

/// Full settings snapshot pushed to the Swift Settings UI. Combines real
/// `Config` struct fields (settings.json / .env / defaults already merged by
/// `Config::load()`) with env-only knobs read from persisted settings / .env
/// without relying on runtime process-env mutation.
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

/// Live, non-secret impact summary shown before a full local-data reset.
/// Counts come from the same two runtime roots that the reset moves to Trash.
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct CsResetPreview {
    pub audio_files: u64,
    pub transcript_days: u64,
    pub threads: u64,
    pub total_bytes: u64,
}

/// UI-safe view of one base prompt. Content is included because this surface is
/// the prompt editor itself; audit records never include it.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CsPromptSnapshot {
    pub content: String,
    pub path: String,
    /// `custom_file`, `built_in_fallback`, or `read_error`.
    pub source: String,
    pub read_error: Option<String>,
}

impl From<PromptSnapshot> for CsPromptSnapshot {
    fn from(snapshot: PromptSnapshot) -> Self {
        Self {
            content: snapshot.content,
            path: snapshot.path.to_string_lossy().into_owned(),
            source: snapshot.source.as_str().to_string(),
            read_error: snapshot.read_error,
        }
    }
}

fn formatting_prompt_kind(level: &str) -> Result<PromptKind, CsError> {
    let policy = FormattingPolicy::parse(level).map_err(|error| CsError::Config {
        msg: error.to_string(),
    })?;
    PromptKind::for_formatting_policy(policy).ok_or_else(|| CsError::Config {
        msg: "Off has no formatting prompt; choose correction, smart, or max".to_string(),
    })
}

/// Presence-only view of the Keychain-backed API keys. Booleans only — the
/// secret values themselves never cross FFI. A key counts as "set" when its
/// account env var or Keychain account is present and non-empty.
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

/// UI-safe API-key liveness bucket. No variant carries secret material.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsApiKeyProbeStatus {
    Ok,
    Invalid,
    NoQuota,
    Network,
    Missing,
    Unsupported,
}

/// Result of one Settings "Test" action for a Keychain account.
#[derive(uniffi::Record)]
pub struct CsApiKeyProbeResult {
    pub account: String,
    pub status: CsApiKeyProbeStatus,
    pub message: String,
    pub probed_endpoint: Option<String>,
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
    /// True when provider-account tokens are stored for this provider.
    pub account_signed_in: bool,
    /// True when the account-login flow can start. For OpenAI this requires a
    /// configured OAuth client id (settings `LLM_OPENAI_OAUTH_CLIENT_ID`, or
    /// dev env `CODESCRIBE_OPENAI_OAUTH_CLIENT_ID`); until then Settings
    /// renders the disabled "Sign in with ChatGPT" affordance.
    pub account_login_enabled: bool,
    /// Human-readable account status ("signed in as <email>", "not signed in",
    /// or "awaiting app registration"). Never contains secrets.
    pub account_status_message: String,
    /// Operator-configured OAuth client id (settings → env resolution). A
    /// non-secret app identity — shown and editable in the Keys panel. `None`
    /// means the account login is still gated on app registration.
    pub oauth_client_id: Option<String>,
    /// Always empty for live Settings; retained for bridge compatibility with
    /// older Swift bindings and preview seed objects.
    pub models: Vec<CsModelOption>,
}

/// Stable lane identity used by the secret-free lane truth FFI snapshot.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsLlmLane {
    Main,
    Formatting,
    Assistive,
}

impl From<CsLlmLane> for lane_truth::LaneTruthLane {
    fn from(value: CsLlmLane) -> Self {
        match value {
            CsLlmLane::Main => Self::Main,
            CsLlmLane::Formatting => Self::Formatting,
            CsLlmLane::Assistive => Self::Assistive,
        }
    }
}

/// Complete canonical truth for one LLM lane. Credentials never cross the
/// bridge: only the owning account name and presence/auth booleans are exposed.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsLaneTruthSnapshot {
    pub lane: CsLlmLane,
    pub provider_id: String,
    pub endpoint: String,
    pub model: String,
    pub key_account: String,
    pub key_present: bool,
    pub account_auth: bool,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

impl From<lane_truth::LaneTruthSnapshot> for CsLaneTruthSnapshot {
    fn from(value: lane_truth::LaneTruthSnapshot) -> Self {
        Self {
            lane: match value.lane {
                lane_truth::LaneTruthLane::Main => CsLlmLane::Main,
                lane_truth::LaneTruthLane::Formatting => CsLlmLane::Formatting,
                lane_truth::LaneTruthLane::Assistive => CsLlmLane::Assistive,
            },
            provider_id: value.provider_id,
            endpoint: value.endpoint,
            model: value.model,
            key_account: value.key_account,
            key_present: value.key_present,
            account_auth: value.account_auth,
            available: value.available,
            unavailable_reason: value.unavailable_reason,
        }
    }
}

/// Project the live Rust lane truth through UniFFI without exposing secrets.
#[uniffi::export]
pub fn lane_truth_snapshot(lane: CsLlmLane) -> CsLaneTruthSnapshot {
    lane_truth::lane_truth_snapshot(lane.into(), &Config::load()).into()
}

/// Result of starting the provider-account login flow. `auth_url` is present
/// when the local callback server is listening and the UI should open a browser.
#[derive(uniffi::Record)]
pub struct CsAccountLoginResult {
    pub provider_id: String,
    pub status: String,
    pub message: String,
    pub auth_url: Option<String>,
    pub signed_in: bool,
    pub client_id_configured: bool,
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
    /// User-owned automatic delivery policy. Assistive and controller safety
    /// vetoes can still prevent a paste for a particular recording.
    pub auto_paste_enabled: bool,
    /// Normalized automatic formatting policy: off, correction, smart, or max.
    pub formatting_level: String,
    /// UI-initiated recording starts in the assistive lane when enabled.
    pub start_assistive: bool,
    /// Notes Mode: voice → daily note (no paste). Backed by the core
    /// `quick_notes_enabled` + `quick_notes_save_only` pair, flipped together.
    pub notes_mode_enabled: bool,
}

/// Thin handle to the codescribe config engine. Stateless: each method reloads
/// or writes through the live `Config` / `UserSettings` / Keychain so Swift
/// always sees on-disk truth.
#[derive(uniffi::Object)]
pub struct CodescribeConfig {}

/// Seed the recorder's legacy process-env input selector exactly once, while
/// the macOS app is still constructing its bridge handles and before hotkey or
/// recording worker threads start. Persisted settings remain the source of
/// truth; an explicit launch-time process override keeps precedence.
fn bootstrap_audio_input_runtime() {
    static AUDIO_INPUT_BOOTSTRAP: Once = Once::new();
    AUDIO_INPUT_BOOTSTRAP.call_once(|| {
        if cfg!(test) || std::env::var_os("AUDIO_INPUT_DEVICE").is_some() {
            return;
        }
        let Some(device) = Config::load_without_keychain()
            .audio_input_device
            .filter(|device| !device.trim().is_empty())
        else {
            return;
        };

        // SAFETY: `CodescribeConfig` is an AppDelegate-owned bridge property,
        // constructed before `CodescribeHotkeys.start()` spawns runtime workers.
        // The Once guard prevents later Settings-scene handles from mutating it.
        unsafe { std::env::set_var("AUDIO_INPUT_DEVICE", device) };
    });
}

#[uniffi::export]
impl CodescribeConfig {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        bootstrap_audio_input_runtime();
        Self {}
    }

    /// Full settings snapshot for the Settings UI. Reloads from disk so it
    /// reflects any writes made since construction.
    pub fn load_settings(&self) -> CsSettings {
        let config = Config::load();
        let settings = UserSettings::load();
        let env_file = load_config_env_file();
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
            // This is the saved user choice, not the process-env selector held
            // by the already-running recorder. AudioPanel gets that live truth
            // separately from `CsAudioInputSnapshot`.
            audio_input_device: setting_string(settings.audio_input_device.clone()),
            history_enabled: config.history_enabled,
            quick_notes_enabled: config.quick_notes_enabled,
            quick_notes_save_only: config.quick_notes_save_only,
            use_local_stt: config.use_local_stt,
            local_model: config.local_model.clone(),
            stt_endpoint: config.stt_endpoint.clone(),
            stt_engine: effective_env_string(
                "CODESCRIBE_STT_ENGINE",
                settings.stt_engine.clone(),
                &env_file,
            ),
            llm_endpoint: config.llm_endpoint.clone(),
            restore_clipboard: config.restore_clipboard,
            restore_clipboard_delay_ms: config.restore_clipboard_delay_ms,
            start_at_login: config.start_at_login,
            agent_enter_sends: config.agent_enter_sends,
            dump_audio_logs: config.dump_audio_logs,
            // Env-only knobs: read the persisted stores first so a runtime UI
            // write is visible without mutating the process environment.
            llm_model: effective_settings_string(
                "LLM_MODEL",
                settings.llm_model.clone(),
                &env_file,
            ),
            llm_formatting_endpoint: effective_settings_string(
                "LLM_FORMATTING_ENDPOINT",
                settings.llm_formatting_endpoint.clone(),
                &env_file,
            ),
            llm_formatting_model: effective_settings_string(
                "LLM_FORMATTING_MODEL",
                settings.llm_formatting_model.clone(),
                &env_file,
            ),
            llm_assistive_endpoint: effective_settings_string(
                "LLM_ASSISTIVE_ENDPOINT",
                settings.llm_assistive_endpoint.clone(),
                &env_file,
            ),
            llm_assistive_model: effective_settings_string(
                "LLM_ASSISTIVE_MODEL",
                settings.llm_assistive_model.clone(),
                &env_file,
            ),
            llm_assistive_provider: effective_settings_string(
                "LLM_ASSISTIVE_PROVIDER",
                settings.llm_assistive_provider.clone(),
                &env_file,
            ),
            formatting_level: Config::formatting_policy()
                .ok()
                .map(|policy| policy.as_str().to_string()),
            whisper_model: effective_settings_string(
                "WHISPER_MODEL",
                settings.whisper_model.clone(),
                &env_file,
            ),
            layered_transcription: effective_env_string(
                "CODESCRIBE_LAYERED_TRANSCRIPTION",
                settings.layered_transcription.clone(),
                &env_file,
            ),
            agent_workspace_roots: effective_env_list(
                "AGENT_WORKSPACE_ROOTS",
                settings.agent_workspace_roots.clone(),
                &env_file,
                DEFAULT_AGENT_WORKSPACE_ROOT,
            ),
            buffer_delay_ms: effective_settings_parse(
                "CODESCRIBE_BUFFER_DELAY_MS",
                settings.buffer_delay_ms,
                &env_file,
            ),
            typing_cps: effective_settings_parse(
                "CODESCRIBE_TYPING_CPS",
                settings.typing_cps,
                &env_file,
            ),
            emit_words_max: effective_settings_parse(
                "CODESCRIBE_EMIT_WORDS_MAX",
                settings.emit_words_max,
                &env_file,
            ),
            buffered_interim_sec: effective_settings_parse(
                "CODESCRIBE_BUFFERED_INTERIM_SEC",
                settings.buffered_interim_sec,
                &env_file,
            ),
            backend_max_upload_mb: effective_settings_parse(
                "BACKEND_MAX_UPLOAD_MB",
                settings.backend_max_upload_mb,
                &env_file,
            ),
        }
    }

    /// Persist Auto Paste and return the prompt-free post-write truth in one
    /// result. Callers may re-read `tray_toggles()` after an error; no optimistic
    /// bridge cache is retained.
    pub fn set_auto_paste_enabled(&self, enabled: bool) -> Result<CsTrayToggles, CsError> {
        let mut settings = UserSettings::load();
        settings.auto_paste_enabled = Some(enabled);
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(self.tray_toggles())
    }

    /// Persist a normalized Auto Format policy and return prompt-free
    /// post-write truth. Unknown policy IDs fail without changing disk state.
    pub fn set_auto_format_level(&self, level: String) -> Result<CsTrayToggles, CsError> {
        let policy = FormattingPolicy::parse(&level).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        let mut settings = UserSettings::load();
        settings.formatting_level = Some(policy.as_str().to_string());
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(self.tray_toggles())
    }

    /// Lightweight tray-only settings read. Unlike `load_settings`, this never
    /// populates the Keychain, so it never prompts just because the user opened
    /// the menu. It DOES honor the full tier stack (defaults < settings.json <
    /// .env < process-env) so env overrides such as `SHOW_DOCK_ICON=0` take
    /// effect — reading `UserSettings` + defaults alone silently dropped them.
    pub fn tray_toggles(&self) -> CsTrayToggles {
        let config = Config::load_without_keychain();
        CsTrayToggles {
            show_dock_icon: config.show_dock_icon,
            transcription_overlay_enabled: config.transcription_overlay_enabled,
            auto_paste_enabled: config.auto_paste_enabled,
            formatting_level: Config::formatting_policy()
                .unwrap_or_default()
                .as_str()
                .to_string(),
            start_assistive: config.tray_start_assistive,
            // Notes Mode is "on" only when BOTH flags are set (dictation → note
            // AND no paste). Reading just quick_notes_enabled could show the toggle
            // ON while dictation still pastes (save_only=false) — an edge desync.
            notes_mode_enabled: config.quick_notes_enabled && config.quick_notes_save_only,
        }
    }

    /// Persist one config value, auto-tiered by the core router
    /// (`save_to_env`): API keys → Keychain, promoted keys → settings.json,
    /// power-user keys → `.env`. Runtime readers reload persisted snapshots
    /// instead of mutating the process env.
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

    /// Clear the persisted input-device preference so the recorder falls back
    /// to the live system default. This is an actual `None` in settings.json,
    /// never `Some("")`; an explicit power-user env override still wins by the
    /// existing three-tier contract.
    pub fn reset_audio_input_device(&self) -> Result<(), CsError> {
        let mut settings = UserSettings::load();
        settings.audio_input_device = None;
        settings.save().map_err(|error| CsError::Config {
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

    /// Canonical normalization for OpenAI Responses endpoints.
    /// Strips known suffixes (/v1/responses, /chat/completions, /completions, /v1)
    /// and forces the /v1/responses tail. Single source of truth in lane_truth;
    /// Swift SettingsViewModel delegates here to eliminate duplication (P2-05).
    pub fn normalize_openai_responses_endpoint(&self, endpoint: String) -> String {
        lane_truth::normalize_openai_responses_endpoint(&endpoint)
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

    /// Probe one Keychain-backed API key account with a single cheap provider
    /// request. Blocking by design: Swift calls this from a background queue.
    /// The secret never crosses FFI; this method reads env/Keychain internally.
    pub fn test_api_key(&self, account: String) -> Result<CsApiKeyProbeResult, CsError> {
        ensure_known_account(&account)?;
        Ok(probe_api_key_liveness(&account).into())
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
                let account_status = account_auth::account_status(*kind);
                CsProviderOption {
                    id: kind.as_str().to_string(),
                    display_name: kind.display_name().to_string(),
                    api_key_set: key_present(&account),
                    api_key_account: account,
                    account_signed_in: account_status.signed_in,
                    account_login_enabled: account_status.client_id_configured,
                    account_status_message: account_status.message,
                    oauth_client_id: matches!(kind, ProviderKind::OpenAiResponses)
                        .then(account_auth::configured_client_id)
                        .flatten(),
                    models: Vec::new(),
                }
            })
            .collect()
    }

    /// Start provider-account login for the selected provider. Today this is
    /// only supported for OpenAI Responses and is gated by the configured OAuth
    /// client id (settings `LLM_OPENAI_OAUTH_CLIENT_ID`, dev-env fallback
    /// `CODESCRIBE_OPENAI_OAUTH_CLIENT_ID`); absent client id returns a config
    /// error whose message contains "awaiting app registration".
    pub fn start_account_login(
        &self,
        provider_id: String,
    ) -> Result<CsAccountLoginResult, CsError> {
        let provider = ProviderKind::from_str(&provider_id).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        let client_id =
            account_auth::client_id_for_provider(provider).map_err(account_auth_to_cs)?;
        let mut opts = account_auth::ServerOptions::new(client_id);
        opts.issuer = account_auth::issuer_from_env();

        let login = account_auth_runtime()?
            .block_on(account_auth::run_login_server(opts))
            .map_err(account_auth_to_cs)?;
        let auth_url = login.auth_url.clone();
        let mut guard = active_account_login().lock().map_err(|_| CsError::Config {
            msg: "account login state lock poisoned".to_string(),
        })?;
        if let Some(previous) = guard.take() {
            previous.cancel();
        }
        *guard = Some(login);

        Ok(CsAccountLoginResult {
            provider_id: provider.as_str().to_string(),
            status: "started".to_string(),
            message: "open the browser to finish sign-in".to_string(),
            auth_url: Some(auth_url),
            signed_in: false,
            client_id_configured: true,
        })
    }

    /// Block until the in-flight provider-account login completes, fails, or
    /// times out. Swift calls this from a background queue right after
    /// `start_account_login` opened the browser. On timeout (user closed the
    /// browser, walked away) the local callback server is shut down — honest
    /// status, no zombie port. A second `start_account_login` while pending
    /// cancels the first, so this returns "failed" for the superseded attempt.
    ///
    /// P2-09: 300s default (from caller) is intentional; OAuth human steps can
    /// exceed short timeouts. No configurability knob added. P2-08: discovery
    /// flows share this; cancel is best-effort via supersede + teardown.
    pub fn await_account_login(
        &self,
        provider_id: String,
        timeout_seconds: u64,
    ) -> Result<CsAccountLoginResult, CsError> {
        let provider = ProviderKind::from_str(&provider_id).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        let login = {
            let mut guard = active_account_login().lock().map_err(|_| CsError::Config {
                msg: "account login state lock poisoned".to_string(),
            })?;
            guard.take()
        };
        let Some(login) = login else {
            return Ok(account_login_result(
                provider,
                "idle",
                "no sign-in in progress",
            ));
        };

        let cancel = login.cancel_handle();
        let outcome = account_auth_runtime()?.block_on(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_seconds.max(1)),
                login.block_until_done(),
            )
            .await
        });

        match outcome {
            Ok(Ok(())) => {
                let message = account_auth::account_status(provider).message;
                Ok(account_login_result(provider, "signed_in", &message))
            }
            Ok(Err(error)) => Ok(account_login_result(provider, "failed", &error.to_string())),
            Err(_elapsed) => {
                cancel.shutdown();
                let message = format!(
                    "sign-in was not completed within {timeout_seconds}s; the local login server was shut down"
                );
                Ok(account_login_result(provider, "timeout", &message))
            }
        }
    }

    /// Cancel any in-flight provider-account login and free the callback port.
    pub fn cancel_account_login(&self) {
        if let Ok(mut guard) = active_account_login().lock()
            && let Some(login) = guard.take()
        {
            login.cancel();
        }
    }

    /// Sign out of the provider account: remove the stored tokens (Keychain +
    /// env mirror). The API-key path is untouched.
    pub fn sign_out_account(&self, provider_id: String) -> Result<(), CsError> {
        let provider = ProviderKind::from_str(&provider_id).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        account_auth::clear_account_tokens(provider).map_err(account_auth_to_cs)
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

    /// Store an API key in the Keychain. `account` must be a known
    /// `KEYCHAIN_ACCOUNTS` entry. The secret is never echoed back.
    pub fn set_api_key(&self, account: String, secret: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        save_key(&account, &secret).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Delete an API key from the Keychain. `account` must be a known
    /// `KEYCHAIN_ACCOUNTS` entry.
    pub fn clear_api_key(&self, account: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        delete_key(&account).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        Ok(())
    }

    /// Current BASE formatting prompt (the editable `formatting.txt`, WITHOUT
    /// the appended `*_tuning.txt`). Falls back to the built-in default when the
    /// file does not exist yet.
    pub fn get_formatting_prompt(&self) -> String {
        prompt_snapshot(PromptKind::Formatting).content
    }

    /// Current BASE assistive prompt (the editable `assistive.txt`, WITHOUT the
    /// appended `*_tuning.txt`). Falls back to the built-in default when the file
    /// does not exist yet.
    pub fn get_assistive_prompt(&self) -> String {
        prompt_snapshot(PromptKind::Assistive).content
    }

    /// Content plus provenance/path for the formatting prompt editor.
    pub fn formatting_prompt_snapshot(&self) -> CsPromptSnapshot {
        prompt_snapshot(PromptKind::Formatting).into()
    }

    /// Content plus provenance/path for one explicit formatting policy prompt.
    pub fn formatting_prompt_snapshot_for_level(
        &self,
        level: String,
    ) -> Result<CsPromptSnapshot, CsError> {
        Ok(prompt_snapshot(formatting_prompt_kind(&level)?).into())
    }

    /// Content plus provenance/path for the assistive prompt editor.
    pub fn assistive_prompt_snapshot(&self) -> CsPromptSnapshot {
        prompt_snapshot(PromptKind::Assistive).into()
    }

    /// Save the BASE formatting prompt through the core's atomic writer.
    pub fn set_formatting_prompt(&self, content: String) -> Result<(), CsError> {
        write_prompt(
            PromptKind::Formatting,
            &content,
            PromptWriteReason::SettingsSave,
        )
        .map_err(CsError::from)
    }

    /// Save one explicit formatting policy prompt through the shared owner.
    pub fn set_formatting_prompt_for_level(
        &self,
        level: String,
        content: String,
    ) -> Result<(), CsError> {
        write_prompt(
            formatting_prompt_kind(&level)?,
            &content,
            PromptWriteReason::SettingsSave,
        )
        .map_err(CsError::from)
    }

    /// Overwrite the BASE assistive prompt file (`assistive.txt`).
    pub fn set_assistive_prompt(&self, content: String) -> Result<(), CsError> {
        write_prompt(
            PromptKind::Assistive,
            &content,
            PromptWriteReason::SettingsSave,
        )
        .map_err(CsError::from)
    }

    /// Restore only the formatting base prompt after explicit UI confirmation.
    pub fn restore_formatting_prompt_to_default(&self) -> Result<(), CsError> {
        restore_prompt_to_default(PromptKind::Formatting).map_err(CsError::from)
    }

    /// Restore one explicit formatting policy prompt after UI confirmation.
    pub fn restore_formatting_prompt_for_level_to_default(
        &self,
        level: String,
    ) -> Result<(), CsError> {
        restore_prompt_to_default(formatting_prompt_kind(&level)?).map_err(CsError::from)
    }

    /// Restore only the assistive base prompt after explicit UI confirmation.
    pub fn restore_assistive_prompt_to_default(&self) -> Result<(), CsError> {
        restore_prompt_to_default(PromptKind::Assistive).map_err(CsError::from)
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

    /// Count the live reset impact without mutating disk or Keychain state.
    pub fn reset_preview(&self) -> CsResetPreview {
        reset_preview_for_dirs(&app_data_dirs())
    }

    /// Move only `mcp.json` to Trash. This intentionally does not touch any
    /// other config, transcripts, threads, logs, preferences, or Keychain keys.
    pub fn clear_mcp_configuration(&self) -> Result<(), CsError> {
        let trash = codescribe_trash_dir()?;
        clear_mcp_configuration_to(&Config::config_dir().join("mcp.json"), &trash)
            .map(|_| ())
            .map_err(|error| CsError::Config {
                msg: format!("failed to move MCP configuration to Trash: {error}"),
            })
    }

    /// Move all local codescribe data to a recoverable folder in the user's
    /// Trash, append an external audit entry, and optionally remove Keychain
    /// keys. The two data roots come from the runtime path helpers, so
    /// `CODESCRIBE_DATA_DIR` overrides remain authoritative, including when the
    /// source lives on another volume.
    ///
    /// Same-volume moves use an atomic rename. Cross-volume moves copy the full
    /// tree first, sync copied files, and only then remove the source without
    /// following symlinks. UserDefaults are cleared by the Swift caller before
    /// relaunch; TCC grants remain untouched.
    pub fn reset_app_data(&self, include_keys: bool, include_prompts: bool) -> Result<(), CsError> {
        let dirs = app_data_dirs();
        let preview = reset_preview_for_dirs(&dirs);
        let preserved_prompts = if include_prompts {
            Vec::new()
        } else {
            capture_base_prompts().map_err(|error| CsError::Config {
                msg: format!("failed to preserve base prompts before reset: {error}"),
            })?
        };
        let now = Utc::now();
        let trash_root = codescribe_trash_dir()?;
        let audit_path = reset_audit_path()?;
        let reset_destination =
            create_reset_destination(&trash_root, &now).map_err(|error| CsError::Config {
                msg: format!("failed to prepare Trash destination: {error}"),
            })?;
        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit_path,
            timestamp: &now,
            status: "started",
            source_paths: &dirs,
            moved_paths: &[],
            trash_path: &reset_destination,
            preview: &preview,
            include_keys,
            include_prompts,
            preserved_prompt_files: preserved_prompts.len(),
        })
        .map_err(|error| CsError::Config {
            msg: format!("failed to append reset audit log: {error}"),
        })?;

        let moved_paths =
            match move_reset_dirs_to_destination(&dirs, &reset_destination, &trash_root) {
                Ok(moved_paths) => moved_paths,
                Err(error) => {
                    let _ = append_reset_audit(&ResetAuditEvent {
                        audit_path: &audit_path,
                        timestamp: &now,
                        status: "move_failed",
                        source_paths: &dirs,
                        moved_paths: &[],
                        trash_path: &reset_destination,
                        preview: &preview,
                        include_keys,
                        include_prompts,
                        preserved_prompt_files: preserved_prompts.len(),
                    });
                    return Err(CsError::Config {
                        msg: format!("failed to move app data to Trash: {error}"),
                    });
                }
            };

        if let Err(error) = restore_base_prompts(&preserved_prompts) {
            let _ = append_reset_audit(&ResetAuditEvent {
                audit_path: &audit_path,
                timestamp: &now,
                status: "data_moved_prompt_restore_failed",
                source_paths: &dirs,
                moved_paths: &moved_paths,
                trash_path: &reset_destination,
                preview: &preview,
                include_keys,
                include_prompts,
                preserved_prompt_files: preserved_prompts.len(),
            });
            return Err(CsError::Config {
                msg: format!("app data moved to Trash but base prompt restoration failed: {error}"),
            });
        }

        let key_error = if include_keys {
            let mut failure = None;
            for account in KEYCHAIN_ACCOUNTS {
                if let Err(error) = delete_key(account) {
                    failure = Some(CsError::Config {
                        msg: format!("failed to remove keychain key {account}: {error}"),
                    });
                    break;
                }
            }
            failure
        } else {
            None
        };

        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit_path,
            timestamp: &now,
            status: if key_error.is_some() {
                "data_moved_keychain_failed"
            } else {
                "completed"
            },
            source_paths: &dirs,
            moved_paths: &moved_paths,
            trash_path: &reset_destination,
            preview: &preview,
            include_keys,
            include_prompts,
            preserved_prompt_files: preserved_prompts.len(),
        })
        .map_err(|error| CsError::Config {
            msg: format!("failed to append reset audit log: {error}"),
        })?;

        if let Some(error) = key_error {
            return Err(error);
        }
        Ok(())
    }
}

/// Re-seed the live hotkey detector atomics after a settings write so mode
/// binding / cadence changes take effect without an app restart. The CGEventTap
/// callback reads these atomics per-event (app/os/hotkeys/platform.rs), so a
/// fresh apply is a true live-reload. Idempotent and cheap (a few atomic stores);
/// applied unconditionally because every settings mutation funnels through
/// `update_config` / `update_config_many`, and re-applying unchanged values is a
/// no-op.
///
/// Loads WITHOUT the Keychain: hotkey bindings and cadence live in
/// settings.json / .env / defaults, never in secrets, so this reload has no
/// reason to `populate_env_from_keychain()`. Using the plain `Config::load()`
/// here fired a Keychain read on every settings write (dock toggle, language,
/// overlay position, …), the same hot-path prompt class as the AX-probe defer.
/// Mirrors `hotkeys::reload_hotkey_runtime_after_write`.
fn reload_hotkey_runtime() {
    codescribe::os::hotkeys::apply_hotkey_config(&Config::load_without_keychain());
}

#[derive(Debug)]
struct PreservedPrompt {
    kind: PromptKind,
    bytes: Vec<u8>,
}

fn capture_base_prompts() -> std::io::Result<Vec<PreservedPrompt>> {
    let mut preserved = Vec::new();
    for kind in PromptKind::USER_OWNED {
        let path = prompts::prompts_dir().join(kind.filename());
        match fs::read(path) {
            Ok(bytes) => preserved.push(PreservedPrompt { kind, bytes }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(preserved)
}

fn restore_base_prompts(prompts: &[PreservedPrompt]) -> std::io::Result<()> {
    for prompt in prompts {
        write_prompt_bytes(
            prompt.kind,
            &prompt.bytes,
            PromptWriteReason::AppResetPreservation,
        )?;
    }
    Ok(())
}

/// App-owned data directories a full reset moves to Trash: config / logs /
/// transcriptions first, then the Application Support store. Both come from the
/// runtime helpers; an override that collapses them onto one path is deduplicated.
fn app_data_dirs() -> Vec<PathBuf> {
    let config_dir = Config::config_dir();
    let settings_dir = codescribe_core::config::UserSettings::settings_dir();
    let mut selected: Vec<(PathBuf, PathBuf)> = Vec::new();
    for candidate in [config_dir, settings_dir] {
        let identity = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if selected
            .iter()
            .any(|(_, existing)| identity == *existing || identity.starts_with(existing))
        {
            continue;
        }
        selected.retain(|(_, existing)| !existing.starts_with(&identity));
        selected.push((candidate, identity));
    }
    selected.into_iter().map(|(path, _)| path).collect()
}

fn codescribe_trash_dir() -> Result<PathBuf, CsError> {
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".Trash"))
        .ok_or_else(|| CsError::Config {
            msg: "could not resolve the user's Trash directory".to_string(),
        })
}

fn reset_audit_path() -> Result<PathBuf, CsError> {
    BaseDirs::new()
        .map(|dirs| {
            dirs.home_dir()
                .join("Library/Logs/Codescribe/reset-audit.log")
        })
        .ok_or_else(|| CsError::Config {
            msg: "could not resolve the Codescribe audit log directory".to_string(),
        })
}

fn reset_preview_for_dirs(dirs: &[PathBuf]) -> CsResetPreview {
    let mut preview = CsResetPreview::default();
    for root in dirs {
        preview.total_bytes = preview.total_bytes.saturating_add(path_size(root));

        let transcriptions = root.join("transcriptions");
        let (audio_files, transcript_days) = transcription_counts(&transcriptions);
        preview.audio_files = preview.audio_files.saturating_add(audio_files);
        preview.transcript_days = preview.transcript_days.saturating_add(transcript_days);

        if preview.threads == 0 {
            preview.threads = thread_index_count(&root.join("threads/index.json"));
        }
    }
    preview
}

fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        return 0;
    }
    if metadata.is_file() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| path_size(&entry.path()))
        .fold(0_u64, u64::saturating_add)
}

fn transcription_counts(path: &Path) -> (u64, u64) {
    let Ok(entries) = fs::read_dir(path) else {
        return (0, 0);
    };
    let mut audio_files = 0_u64;
    let mut days = 0_u64;
    for entry in entries.filter_map(Result::ok) {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            let (day_audio, day_files) = count_transcription_tree(&entry_path);
            audio_files = audio_files.saturating_add(day_audio);
            if day_files > 0 {
                days = days.saturating_add(1);
            }
        } else if is_audio_file(&entry_path) {
            audio_files = audio_files.saturating_add(1);
        }
    }
    (audio_files, days)
}

fn count_transcription_tree(path: &Path) -> (u64, u64) {
    let Ok(entries) = fs::read_dir(path) else {
        return (0, 0);
    };
    let mut audio = 0_u64;
    let mut files = 0_u64;
    for entry in entries.filter_map(Result::ok) {
        let entry_path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let (nested_audio, nested_files) = count_transcription_tree(&entry_path);
            audio = audio.saturating_add(nested_audio);
            files = files.saturating_add(nested_files);
        } else if metadata.is_file() {
            files = files.saturating_add(1);
            if is_audio_file(&entry_path) {
                audio = audio.saturating_add(1);
            }
        }
    }
    (audio, files)
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "m4a" | "mp3" | "aac" | "caf" | "flac"
            )
        })
}

fn thread_index_count(path: &Path) -> u64 {
    let Ok(bytes) = fs::read(path) else {
        return 0;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("threads")?
                .as_array()
                .map(|threads| threads.len())
        })
        .map_or(0, |count| count as u64)
}

fn timestamp_slug(now: &DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H-%M-%S%.3fZ").to_string()
}

fn unique_destination(root: &Path, stem: &str, extension: Option<&str>) -> PathBuf {
    for suffix in 0_u32..=10_000 {
        let numbered = if suffix == 0 {
            stem.to_string()
        } else {
            format!("{stem}-{suffix}")
        };
        let candidate = match extension {
            Some(extension) => root.join(format!("{numbered}.{extension}")),
            None => root.join(numbered),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{stem}-{}", std::process::id()))
}

fn validate_reset_source(source: &Path, trash_root: &Path) -> std::io::Result<()> {
    let source = source.canonicalize()?;
    let trash = trash_root.canonicalize()?;
    let home = BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    let audit_log = reset_audit_path().ok();
    if source.parent().is_none()
        || home.as_deref().is_some_and(|home| source == home)
        || trash.starts_with(&source)
        || audit_log
            .as_deref()
            .is_some_and(|audit_log| audit_log.starts_with(&source))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing unsafe reset source {}", source.display()),
        ));
    }
    Ok(())
}

fn create_reset_destination(trash_root: &Path, now: &DateTime<Utc>) -> std::io::Result<PathBuf> {
    fs::create_dir_all(trash_root)?;
    let reset_destination = unique_destination(
        trash_root,
        &format!("codescribe-reset-{}", timestamp_slug(now)),
        None,
    );
    fs::create_dir(&reset_destination)?;
    Ok(reset_destination)
}

fn move_reset_dirs_to_destination(
    dirs: &[PathBuf],
    reset_destination: &Path,
    trash_root: &Path,
) -> std::io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut moved_paths = Vec::new();
    for (index, source) in dirs.iter().filter(|source| source.exists()).enumerate() {
        validate_reset_source(source, trash_root)?;
        let label = if index == 0 {
            "codescribe-data".to_string()
        } else {
            format!("application-support-{index}")
        };
        let destination = reset_destination.join(label);
        move_path_recoverably(source, &destination)?;
        moved_paths.push((source.clone(), destination));
    }
    Ok(moved_paths)
}

/// Prefer same-volume rename. If the source is on another volume, create and
/// sync the complete destination before removing the source tree.
fn move_path_recoverably(source: &Path, destination: &Path) -> std::io::Result<()> {
    move_path_recoverably_with(source, destination, |source, destination| {
        fs::rename(source, destination)
    })
}

fn move_path_recoverably_with<F>(
    source: &Path,
    destination: &Path,
    rename: F,
) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    match rename(source, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            if let Err(copy_error) = copy_path_without_following_symlinks(source, destination) {
                return Err(std::io::Error::new(
                    copy_error.kind(),
                    format!("rename failed ({rename_error}); fallback copy failed ({copy_error})"),
                ));
            }
            remove_path_without_following_symlinks(source)
        }
    }
}

fn copy_path_without_following_symlinks(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        // The destination is an app-created child of ~/.Trash, and this call
        // recreates the link itself without following or writing through its
        // target. Preserving the link is required for a recoverable reset.
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
        std::os::unix::fs::symlink(target, destination)?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path_without_following_symlinks(
                &entry.path(),
                &destination.join(entry.file_name()),
            )?;
        }
        return Ok(());
    }
    fs::copy(source, destination)?;
    OpenOptions::new().read(true).open(destination)?.sync_all()
}

fn remove_path_without_following_symlinks(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for entry in fs::read_dir(path)? {
            remove_path_without_following_symlinks(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

fn clear_mcp_configuration_to(
    mcp_path: &Path,
    trash_root: &Path,
) -> std::io::Result<Option<PathBuf>> {
    if !mcp_path.exists() {
        return Ok(None);
    }
    fs::create_dir_all(trash_root)?;
    let now = Utc::now();
    let destination = unique_destination(
        trash_root,
        &format!("codescribe-mcp-{}", timestamp_slug(&now)),
        Some("json"),
    );
    move_path_recoverably(mcp_path, &destination)?;
    Ok(Some(destination))
}

struct ResetAuditEvent<'a> {
    audit_path: &'a Path,
    timestamp: &'a DateTime<Utc>,
    status: &'a str,
    source_paths: &'a [PathBuf],
    moved_paths: &'a [(PathBuf, PathBuf)],
    trash_path: &'a Path,
    preview: &'a CsResetPreview,
    include_keys: bool,
    include_prompts: bool,
    preserved_prompt_files: usize,
}

fn append_reset_audit(event: &ResetAuditEvent<'_>) -> std::io::Result<()> {
    if let Some(parent) = event.audit_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let entry = serde_json::json!({
        "timestamp": event.timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
        "action": "reset_app_data",
        "status": event.status,
        "source_paths": event.source_paths.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
        "moved_paths": event.moved_paths.iter().map(|(source, destination)| serde_json::json!({
            "source": source.to_string_lossy(),
            "destination": destination.to_string_lossy(),
        })).collect::<Vec<_>>(),
        "trash_path": event.trash_path.to_string_lossy(),
        "audio_files": event.preview.audio_files,
        "transcript_days": event.preview.transcript_days,
        "threads": event.preview.threads,
        "total_bytes": event.preview.total_bytes,
        "include_keys": event.include_keys,
        "include_prompts": event.include_prompts,
        "preserved_prompt_files": event.preserved_prompt_files,
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(event.audit_path)?;
    writeln!(file, "{entry}")?;
    file.sync_data()
}

/// Built-in default workspace root when `AGENT_WORKSPACE_ROOTS` is unset. Kept in
/// sync with the `list_projects` tool default (`app/agent/tools/workspace.rs`).
const DEFAULT_AGENT_WORKSPACE_ROOT: &str = "~/Git";

fn load_config_env_file() -> HashMap<String, String> {
    let path = Config::env_path();
    if path.exists() {
        Config::parse_env_file(&path).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn setting_string(value: Option<String>) -> Option<String> {
    value.and_then(non_empty)
}

fn file_env_string(key: &str, env_file: &HashMap<String, String>) -> Option<String> {
    env_file.get(key).cloned().and_then(non_empty)
}

/// Promoted settings are settings.json-owned; prefer that store over process
/// env so stale bootstrap-seeded env does not mask a fresh UI write.
fn effective_settings_string(
    key: &str,
    setting: Option<String>,
    env_file: &HashMap<String, String>,
) -> Option<String> {
    setting_string(setting)
        .or_else(|| file_env_string(key, env_file))
        .or_else(|| env_string(key))
}

/// Env-managed settings are persisted to .env when changed from the UI. Read
/// that file before process env so runtime writes are visible without set_var.
fn effective_env_string(
    key: &str,
    setting: Option<String>,
    env_file: &HashMap<String, String>,
) -> Option<String> {
    file_env_string(key, env_file)
        .or_else(|| setting_string(setting))
        .or_else(|| env_string(key))
}

fn effective_settings_parse<T>(
    key: &str,
    setting: Option<T>,
    env_file: &HashMap<String, String>,
) -> Option<T>
where
    T: std::str::FromStr,
{
    setting
        .or_else(|| file_env_string(key, env_file).and_then(|value| value.parse().ok()))
        .or_else(|| env_parse(key))
}

fn parse_roots(value: &str) -> Vec<String> {
    value
        .split(':')
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Colon-separated env var into a trimmed, non-empty `Vec<String>`. Falls back to
/// a single-element `[default]` when the var is unset/empty, so the Settings UI
/// always renders the effective root the agent tool will scan.
fn effective_env_list(
    key: &str,
    setting: Option<Vec<String>>,
    env_file: &HashMap<String, String>,
    default: &str,
) -> Vec<String> {
    let roots = file_env_string(key, env_file)
        .map(|value| parse_roots(&value))
        .or_else(|| {
            setting.map(|roots| {
                roots
                    .into_iter()
                    .map(|segment| segment.trim().to_string())
                    .filter(|segment| !segment.is_empty())
                    .collect()
            })
        })
        .or_else(|| std::env::var(key).ok().map(|value| parse_roots(&value)))
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

/// True when the account env var or Keychain account is present and non-empty.
fn key_present(account: &str) -> bool {
    lane_truth::secret(account).is_some()
}

fn account_auth_runtime() -> Result<&'static tokio::runtime::Runtime, CsError> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("codescribe-account-auth")
            .build()
            .map_err(|error| format!("account auth runtime initialization failed: {error}"))
    }) {
        Ok(runtime) => Ok(runtime),
        Err(msg) => Err(CsError::Config { msg: msg.clone() }),
    }
}

fn active_account_login() -> &'static Mutex<Option<account_auth::LoginServer>> {
    static ACTIVE: OnceLock<Mutex<Option<account_auth::LoginServer>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

fn account_auth_to_cs(error: account_auth::AccountAuthError) -> CsError {
    CsError::Config {
        msg: error.to_string(),
    }
}

/// Terminal (non-"started") login result from the live account status —
/// `signed_in` / `client_id_configured` always re-read, never assumed. Lives
/// outside the exported impl: it takes a core `ProviderKind`, which must not
/// cross the FFI boundary.
fn account_login_result(
    provider: ProviderKind,
    status: &str,
    message: &str,
) -> CsAccountLoginResult {
    let account_status = account_auth::account_status(provider);
    CsAccountLoginResult {
        provider_id: provider.as_str().to_string(),
        status: status.to_string(),
        message: message.to_string(),
        auth_url: None,
        signed_in: account_status.signed_in,
        client_id_configured: account_status.client_id_configured,
    }
}

impl From<ApiKeyLivenessStatus> for CsApiKeyProbeStatus {
    fn from(status: ApiKeyLivenessStatus) -> Self {
        match status {
            ApiKeyLivenessStatus::Ok => Self::Ok,
            ApiKeyLivenessStatus::Invalid => Self::Invalid,
            ApiKeyLivenessStatus::NoQuota => Self::NoQuota,
            ApiKeyLivenessStatus::Network => Self::Network,
            ApiKeyLivenessStatus::Missing => Self::Missing,
            ApiKeyLivenessStatus::Unsupported => Self::Unsupported,
        }
    }
}

impl From<ApiKeyLivenessResult> for CsApiKeyProbeResult {
    fn from(result: ApiKeyLivenessResult) -> Self {
        Self {
            account: result.account,
            status: result.status.into(),
            message: result.message,
            probed_endpoint: result.probed_endpoint,
        }
    }
}

#[cfg(test)]
mod api_key_probe_tests {
    use super::{ApiKeyLivenessResult, ApiKeyLivenessStatus, CsApiKeyProbeResult};

    #[test]
    fn bridge_probe_result_preserves_the_endpoint_used_by_core() {
        let result = CsApiKeyProbeResult::from(ApiKeyLivenessResult {
            account: "LLM_ASSISTIVE_API_KEY".to_string(),
            status: ApiKeyLivenessStatus::Invalid,
            message: "provider rejected this key".to_string(),
            probed_endpoint: Some("https://api.libraxis.cloud/v1/responses".to_string()),
        });

        assert_eq!(
            result.probed_endpoint.as_deref(),
            Some("https://api.libraxis.cloud/v1/responses")
        );
    }
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

#[cfg(test)]
mod reset_tests {
    use super::{
        CsResetPreview, ResetAuditEvent, app_data_dirs, append_reset_audit, capture_base_prompts,
        clear_mcp_configuration_to, create_reset_destination, move_path_recoverably_with,
        move_reset_dirs_to_destination, remove_path_without_following_symlinks,
        reset_preview_for_dirs, restore_base_prompts,
    };
    use chrono::{DateTime, Utc};
    use serial_test::serial;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique scratch directory under the OS temp dir (never the real home).
    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("cs_reset_{}_{tag}_{nanos}", std::process::id()))
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: reset tests that mutate process env are serialized.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restores the serialized test's prior process environment.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(path, bytes).expect("write fixture");
    }

    fn fixed_timestamp() -> DateTime<Utc> {
        "2026-07-16T16:15:00Z"
            .parse()
            .expect("valid fixed reset timestamp")
    }

    /// The reset scope follows `CODESCRIBE_DATA_DIR`, previews live counts, and
    /// moves the complete source into a recoverable Trash destination.
    #[test]
    #[serial]
    fn reset_scope_follows_data_dir_and_moves_live_data_to_trash() {
        let sandbox = scratch("scope");
        let root = sandbox.join("source");
        let trash = sandbox.join("trash");
        let audit = sandbox.join("logs/reset-audit.log");
        write(
            &root.join("transcriptions/2026-07-15/one.m4a"),
            b"audio-one",
        );
        write(&root.join("transcriptions/2026-07-15/one.txt"), b"hello");
        write(
            &root.join("transcriptions/2026-07-16/two.wav"),
            b"audio-two",
        );
        write(
            &root.join("threads/index.json"),
            br#"{"version":3,"threads":[{"id":"one"},{"id":"two"}]}"#,
        );
        write(&root.join("settings.json"), b"{}");
        write(
            &root.join("prompts/assistive.txt"),
            b"sacred assistive bytes\n",
        );
        write(
            &root.join("prompts/formatting.txt"),
            b"sacred formatting bytes\n",
        );
        write(
            &root.join("prompts/formatting-smart.txt"),
            b"sacred smart bytes\n",
        );
        write(
            &root.join("prompts/formatting-max.txt"),
            b"sacred max bytes\n",
        );
        let root_canon = root.canonicalize().expect("canonical reset root");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);

        let dirs = app_data_dirs();
        assert!(!dirs.is_empty(), "reset must target at least one dir");
        for dir in &dirs {
            let resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            assert_eq!(
                resolved, root_canon,
                "reset target escaped the data dir: {dir:?}"
            );
        }

        let preview = reset_preview_for_dirs(&dirs);
        assert_eq!(preview.audio_files, 2);
        assert_eq!(preview.transcript_days, 2);
        assert_eq!(preview.threads, 2);
        assert!(preview.total_bytes >= 25);

        let timestamp = fixed_timestamp();
        let destination =
            create_reset_destination(&trash, &timestamp).expect("create test Trash destination");
        let preserved_prompts = capture_base_prompts().expect("capture sacred prompts");
        let moved_paths = move_reset_dirs_to_destination(&dirs, &destination, &trash)
            .expect("move reset scope to test Trash");
        restore_base_prompts(&preserved_prompts).expect("restore sacred prompts");
        assert!(!root.join("settings.json").exists());
        assert_eq!(
            std::fs::read(root.join("prompts/assistive.txt")).expect("read restored assistive"),
            b"sacred assistive bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting.txt")).expect("read restored formatting"),
            b"sacred formatting bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting-smart.txt")).expect("read restored smart"),
            b"sacred smart bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting-max.txt")).expect("read restored max"),
            b"sacred max bytes\n"
        );
        assert!(destination.join("codescribe-data/settings.json").is_file());
        assert!(
            destination
                .join("codescribe-data/transcriptions/2026-07-15/one.m4a")
                .is_file()
        );

        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "completed",
            source_paths: &dirs,
            moved_paths: &moved_paths,
            trash_path: &destination,
            preview: &preview,
            include_keys: false,
            include_prompts: false,
            preserved_prompt_files: preserved_prompts.len(),
        })
        .expect("append reset audit");
        let line = std::fs::read_to_string(&audit).expect("read reset audit");
        let entry: serde_json::Value = serde_json::from_str(line.trim()).expect("parse audit JSON");
        assert_eq!(entry["timestamp"], "2026-07-16T16:15:00.000Z");
        assert_eq!(entry["action"], "reset_app_data");
        assert_eq!(entry["status"], "completed");
        assert_eq!(entry["audio_files"], 2);
        assert_eq!(entry["transcript_days"], 2);
        assert_eq!(entry["threads"], 2);
        assert_eq!(entry["include_keys"], false);
        assert_eq!(entry["include_prompts"], false);
        assert_eq!(entry["preserved_prompt_files"], 4);
        assert_eq!(entry["trash_path"], destination.to_string_lossy().as_ref());
        assert!(
            entry["source_paths"]
                .as_array()
                .is_some_and(|paths| paths.len() == 1)
        );
        assert_eq!(
            entry["moved_paths"][0]["source"],
            root_canon.to_string_lossy().as_ref()
        );
        assert_eq!(
            entry["moved_paths"][0]["destination"],
            destination
                .join("codescribe-data")
                .to_string_lossy()
                .as_ref()
        );

        remove_path_without_following_symlinks(&sandbox).expect("clean reset fixture");
    }

    #[test]
    fn reset_audit_log_is_append_only_and_keeps_each_entry() {
        let sandbox = scratch("audit");
        let audit = sandbox.join("reset-audit.log");
        let timestamp = fixed_timestamp();
        let preview = CsResetPreview {
            audio_files: 7,
            transcript_days: 3,
            threads: 4,
            total_bytes: 42,
        };
        let sources = vec![sandbox.join("source")];
        let destination = sandbox.join("trash/reset");

        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "started",
            source_paths: &sources,
            moved_paths: &[],
            trash_path: &destination,
            preview: &preview,
            include_keys: true,
            include_prompts: false,
            preserved_prompt_files: 2,
        })
        .expect("append first audit line");
        let moved_paths = vec![(sources[0].clone(), destination.join("codescribe-data"))];
        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "completed",
            source_paths: &sources,
            moved_paths: &moved_paths,
            trash_path: &destination,
            preview: &preview,
            include_keys: true,
            include_prompts: false,
            preserved_prompt_files: 2,
        })
        .expect("append second audit line");

        let content = std::fs::read_to_string(&audit).expect("read append-only audit");
        assert_eq!(content.lines().count(), 2);
        assert!(
            content
                .lines()
                .all(|line| line.contains("\"include_keys\":true"))
        );
        assert!(
            content
                .lines()
                .next()
                .is_some_and(|line| line.contains("\"status\":\"started\""))
        );
        assert!(
            content
                .lines()
                .nth(1)
                .is_some_and(|line| line.contains("\"status\":\"completed\""))
        );
        remove_path_without_following_symlinks(&sandbox).expect("clean audit fixture");
    }

    #[test]
    fn reset_cross_volume_fallback_copies_before_source_removal() {
        let sandbox = scratch("fallback");
        let source = sandbox.join("source");
        let destination = sandbox.join("destination");
        write(&source.join("nested/audio.m4a"), b"recoverable-audio");

        move_path_recoverably_with(&source, &destination, |_source, _destination| {
            Err(std::io::Error::other("forced cross-volume rename failure"))
        })
        .expect("complete copy-and-remove fallback after rename failure");
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(destination.join("nested/audio.m4a")).expect("read copied audio"),
            b"recoverable-audio"
        );
        assert!(destination.join("nested/audio.m4a").is_file());
        remove_path_without_following_symlinks(&sandbox).expect("clean fallback fixture");
    }

    #[test]
    fn reset_clear_mcp_configuration_moves_only_mcp_json() {
        let sandbox = scratch("mcp");
        let data = sandbox.join("data");
        let trash = sandbox.join("trash");
        let mcp = data.join("mcp.json");
        let settings = data.join("settings.json");
        write(&mcp, br#"{"mcpServers":{"loctree":{}}}"#);
        write(&settings, br#"{"speech":{"language":"pl"}}"#);

        let destination = clear_mcp_configuration_to(&mcp, &trash)
            .expect("move MCP config to Trash")
            .expect("existing MCP config destination");

        assert!(!mcp.exists());
        assert_eq!(
            std::fs::read(&settings).expect("settings must survive MCP clear"),
            br#"{"speech":{"language":"pl"}}"#
        );
        assert_eq!(
            std::fs::read(destination).expect("read trashed MCP config"),
            br#"{"mcpServers":{"loctree":{}}}"#
        );
        remove_path_without_following_symlinks(&sandbox).expect("clean MCP fixture");
    }
}

#[cfg(test)]
mod settings_snapshot_tests {
    use super::{CodescribeConfig, remove_path_without_following_symlinks};
    use codescribe_core::config::{Config, UserSettings};
    use serial_test::serial;
    use std::ffi::{OsStr, OsString};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "cs_settings_snapshot_{}_{tag}_{nanos}",
            std::process::id()
        ))
    }

    #[test]
    #[serial]
    fn assistive_provider_ui_write_is_promoted_to_settings_json() {
        let root = scratch("assistive_provider");
        std::fs::create_dir_all(&root).unwrap();

        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _provider = EnvGuard::remove("LLM_ASSISTIVE_PROVIDER");

        let config = CodescribeConfig::new();
        config
            .update_config(
                "LLM_ASSISTIVE_PROVIDER".to_string(),
                "anthropic-messages".to_string(),
            )
            .expect("persist provider through the UI bridge");

        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(UserSettings::settings_path())
                .expect("read promoted settings.json"),
        )
        .expect("parse promoted settings.json");
        assert_eq!(
            persisted
                .get("speech")
                .and_then(|value| value.get("assistive"))
                .and_then(|value| value.get("provider"))
                .and_then(serde_json::Value::as_str),
            Some("anthropic-messages")
        );

        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional .env");
            assert!(
                !env.contains_key("LLM_ASSISTIVE_PROVIDER"),
                "promoted provider must not be written to .env"
            );
        }
        assert_eq!(
            config.load_settings().llm_assistive_provider.as_deref(),
            Some("anthropic-messages")
        );

        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn tray_toggles_roundtrip_auto_paste_and_format_truth_after_write_failure() {
        let root = scratch("tray_delivery_truth");
        std::fs::create_dir_all(&root).expect("create bridge scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _auto_paste = EnvGuard::remove("AUTO_PASTE_ENABLED");
        let _formatting = EnvGuard::remove("FORMATTING_LEVEL");
        let config = CodescribeConfig::new();

        let after_paste = config
            .set_auto_paste_enabled(false)
            .expect("persist auto paste false");
        assert!(!after_paste.auto_paste_enabled);
        assert_eq!(after_paste.formatting_level, "correction");

        let after_format = config
            .set_auto_format_level("smart".to_string())
            .expect("persist normalized format level");
        assert!(!after_format.auto_paste_enabled);
        assert_eq!(after_format.formatting_level, "smart");

        // Block the atomic temp-file write while leaving settings.json readable.
        // The write must fail and a fresh prompt-free snapshot must recover the
        // last persisted truth rather than an optimistic requested value.
        std::fs::create_dir_all(UserSettings::settings_path().with_extension("json.tmp"))
            .expect("block atomic settings temp path");
        assert!(config.set_auto_paste_enabled(true).is_err());
        let reread = config.tray_toggles();
        assert!(!reread.auto_paste_enabled);
        assert_eq!(reread.formatting_level, "smart");

        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional env");
            assert!(!env.contains_key("AUTO_PASTE_ENABLED"));
            assert!(!env.contains_key("FORMATTING_LEVEL"));
        }
        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn audio_input_reset_persists_absence_not_an_empty_override() {
        let root = scratch("audio_input_reset");
        std::fs::create_dir_all(&root).expect("create audio reset scratch dir");

        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        // Mirror the already-running recorder: its process selector remains
        // pinned until restart even after the saved preference is unset.
        let _device = EnvGuard::set("AUDIO_INPUT_DEVICE", "USB Studio Mic");

        let mut settings = UserSettings {
            audio_input_device: Some("USB Studio Mic".to_string()),
            ..Default::default()
        };
        settings.save().expect("seed audio device preference");

        CodescribeConfig::new()
            .reset_audio_input_device()
            .expect("reset audio device through the bridge");

        settings = UserSettings::load();
        assert_eq!(settings.audio_input_device, None);
        assert_eq!(
            CodescribeConfig::new().load_settings().audio_input_device,
            None
        );
        let persisted = std::fs::read_to_string(UserSettings::settings_path())
            .expect("read settings after audio reset");
        assert!(!persisted.contains("USB Studio Mic"));
        assert!(!persisted.contains("\"input_device_id\": \"\""));

        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn load_settings_prefers_persisted_model_over_stale_process_env() {
        let root = scratch("llm_model");
        std::fs::create_dir_all(&root).unwrap();

        let previous_data_dir = std::env::var("CODESCRIBE_DATA_DIR").ok();
        let previous_model = std::env::var("LLM_MODEL").ok();
        // SAFETY: serialized test body; no background workers are started.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &root);
            std::env::set_var("LLM_MODEL", "stale-bootstrap-model");
        }

        let settings = UserSettings {
            llm_model: Some("fresh-runtime-model".to_string()),
            ..Default::default()
        };
        settings.save().unwrap();

        let snapshot = CodescribeConfig::new().load_settings();
        assert_eq!(snapshot.llm_model.as_deref(), Some("fresh-runtime-model"));

        // SAFETY: restore prior env, same serialized single-thread context.
        unsafe {
            match previous_data_dir {
                Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
            }
            match previous_model {
                Some(value) => std::env::set_var("LLM_MODEL", value),
                None => std::env::remove_var("LLM_MODEL"),
            }
        }
        let _ = remove_path_without_following_symlinks(&root);
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: this module serializes every process-env test.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: this module serializes every process-env test.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: this module serializes every process-env test.
            unsafe {
                match self.previous.as_ref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
