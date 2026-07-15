//! Configuration surface — thin UniFFI wrapper over the live codescribe
//! `config` engine (settings.json / .env tiering, Keychain-backed API keys,
//! prompt files, onboarding state). Split out of `lib.rs` in W3 cut #0 so each
//! bridge slice owns a disjoint file.
//!
//! Sync-only (NOT tokio): every call here is cheap disk / Keychain / env I/O.
//! Secrets NEVER cross the FFI boundary — only `CsKeyStatus` booleans report
//! whether a key is present.

use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use codescribe_core::config::keychain::{KEYCHAIN_ACCOUNTS, delete_key, save_key};
use codescribe_core::config::prompts::{get_assistive_prompt_path, get_formatting_prompt_path};
use codescribe_core::config::{
    Config, DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, UserSettings, reset_to_defaults,
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
            audio_input_device: config.audio_input_device.clone(),
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
            formatting_level: effective_settings_string(
                "FORMATTING_LEVEL",
                settings.formatting_level.clone(),
                &env_file,
            ),
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

    /// Wipe all local codescribe data for a privacy "Reset app data" action.
    ///
    /// Removes the two app-owned data trees — the config / logs / transcription
    /// directory (`~/.codescribe`) and the Application Support store
    /// (`~/Library/Application Support/Codescribe`, i.e. settings.json + thread
    /// history) — sourced from the same runtime path helpers the app reads, so a
    /// `CODESCRIBE_DATA_DIR` override is honored and no `~` is ever hardcoded.
    ///
    /// When `include_keys` is true, also removes every Keychain-backed API key
    /// (`KEYCHAIN_ACCOUNTS`). Deleting the data trees clears the `setup_done` sentinel, so the next launch replays the
    /// first-run wizard. TCC / permission grants are deliberately NOT touched —
    /// those are the user's to manage in System Settings.
    ///
    /// UserDefaults (window frames / SwiftUI scene restoration) is a CFPreferences
    /// domain with no core helper; the Swift caller clears it before relaunch.
    pub fn reset_app_data(&self, include_keys: bool) -> Result<(), CsError> {
        for dir in app_data_dirs() {
            remove_dir_all_tolerant(&dir).map_err(|error| CsError::Config {
                msg: format!("failed to remove {}: {error}", dir.display()),
            })?;
        }
        if include_keys {
            for account in KEYCHAIN_ACCOUNTS {
                delete_key(account).map_err(|error| CsError::Config {
                    msg: format!("failed to remove keychain key {account}: {error}"),
                })?;
            }
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

/// App-owned data directories a full "Reset app data" wipes: the config / logs /
/// transcription tree first, then the Application Support store. Both come from
/// the runtime path helpers (honoring `CODESCRIBE_DATA_DIR`); when an override
/// collapses them onto one path the duplicate is dropped so we never remove twice.
fn app_data_dirs() -> Vec<std::path::PathBuf> {
    let config_dir = Config::config_dir();
    let settings_dir = codescribe_core::config::UserSettings::settings_dir();
    if settings_dir == config_dir {
        vec![config_dir]
    } else {
        vec![config_dir, settings_dir]
    }
}

/// `remove_dir_all` that treats an already-absent directory as success: a reset
/// must not fail just because a tree (e.g. an Application Support store that was
/// never created) does not exist yet.
fn remove_dir_all_tolerant(dir: &std::path::Path) -> std::io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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

#[cfg(test)]
mod reset_tests {
    use super::{app_data_dirs, remove_dir_all_tolerant};
    use serial_test::serial;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique scratch directory under the OS temp dir (never the real home).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("cs_reset_{}_{tag}_{nanos}", std::process::id()))
    }

    /// The reset scope must follow the `CODESCRIBE_DATA_DIR` override and wipe
    /// exactly that tree — never anything outside it — and a repeat wipe of an
    /// already-gone tree must succeed (idempotent). `#[serial]`: mutates the global
    /// `CODESCRIBE_DATA_DIR` env var read by the core path helpers.
    #[test]
    #[serial]
    fn reset_scope_follows_data_dir_and_is_idempotent() {
        let root = scratch("scope");
        std::fs::create_dir_all(root.join("transcriptions")).unwrap();
        std::fs::write(root.join("settings.json"), b"{}").unwrap();
        std::fs::write(root.join("transcriptions/a.txt"), b"hi").unwrap();
        let root_canon = root.canonicalize().unwrap();

        let previous = std::env::var("CODESCRIBE_DATA_DIR").ok();
        // SAFETY: single-threaded test body, serialized via `#[serial]`.
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &root) };

        let dirs = app_data_dirs();
        assert!(!dirs.is_empty(), "reset must target at least one dir");
        // Every target resolves to the override root — nothing escapes it.
        for dir in &dirs {
            let resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            assert_eq!(
                resolved, root_canon,
                "reset target escaped the data dir: {dir:?}"
            );
        }

        for dir in &dirs {
            remove_dir_all_tolerant(dir).unwrap();
        }
        assert!(!root_canon.exists(), "data tree survived reset");

        // Idempotent: wiping an absent tree is a no-op, not an error.
        for dir in &dirs {
            remove_dir_all_tolerant(dir).unwrap();
        }

        // SAFETY: restore prior env, same serialized single-thread context.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
            }
        }
    }
}

#[cfg(test)]
mod settings_snapshot_tests {
    use super::CodescribeConfig;
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

        let _ = std::fs::remove_dir_all(root);
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
        let _ = std::fs::remove_dir_all(root);
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
