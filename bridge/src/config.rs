//! Configuration surface — thin UniFFI wrapper over the live codescribe
//! `config` engine (settings.json / .env tiering, Keychain-backed API keys,
//! prompt files, onboarding state). Split out of `lib.rs` in W3 cut #0 so each
//! bridge slice owns a disjoint file.
//!
//! Sync-only (NOT tokio): every call here is cheap disk / Keychain / env I/O.
//! Secrets NEVER cross the FFI boundary — only `CsKeyStatus` booleans report
//! whether a key is present.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, SecondsFormat, Utc};
use codescribe_core::config::keychain::{self, KEYCHAIN_ACCOUNTS, delete_key, save_key};
use codescribe_core::config::settings::normalize_agent_workspace_roots;
use codescribe_core::config::{
    AppDataResetGuard, Config, DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT,
    FormattingPolicy, PasteMode, PromptKind, PromptSnapshot, PromptWriteReason, RuntimeLlmLane,
    RuntimeLlmLaneKind, RuntimeLlmLaneUnavailable, RuntimeSettingsSnapshot, UserSettings,
    begin_app_data_reset, prompt_snapshot, prompts, reset_to_defaults, restore_prompt_to_default,
    write_prompt, write_prompt_bytes_during_reset,
};
use codescribe_core::llm::account_auth;
use codescribe_core::llm::key_liveness::{
    ApiKeyLivenessResult, ApiKeyLivenessStatus, probe_api_key_liveness,
};
use codescribe_core::llm::model_discovery::{
    ModelDiscoveryStatus, discover_models as discover_provider_models,
};
use codescribe_core::llm::provider::{
    CustomProvider, ProviderKind, ProviderRef, ProviderRegistry, ResolvedProvider, WireFamily,
};
use codescribe_core::stt::lanes::{SttLane, validate_stt_endpoint};
use directories::BaseDirs;

use crate::{CsError, CsLanguage, application_runtime};

/// Read the launch repair receipt without loading settings again.
#[uniffi::export]
pub fn config_repair_summary() -> Option<String> {
    codescribe_core::config::repair::launch_receipt().summary()
}

/// Typed core actions and refusals serialized without env values or secrets.
#[uniffi::export]
pub fn config_repair_receipt_json() -> String {
    serde_json::to_string(&codescribe_core::config::repair::launch_receipt())
        .unwrap_or_else(|_| "{\"error\":\"receipt serialization failed\"}".into())
}

/// Stable cross-FFI marker: Swift must relaunch even though reset returned an
/// error, because at least one app-data root has already moved and the Rust
/// process fence is permanently latched.
const RESET_RELAUNCH_REQUIRED_MARKER: &str = "CODESCRIBE_RESET_RELAUNCH_REQUIRED";
const AGENT_RESET_RELAUNCH_REQUIRED_MARKER: &str = "CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED";

/// Preserve the ordinary error contract before the destructive boundary, but
/// mark every post-boundary error so the host cannot leave a half-reset,
/// permanently fenced process running.
fn reset_error(reset: &AppDataResetGuard, message: impl Into<String>) -> CsError {
    let message = message.into();
    let msg = if reset.relaunch_required() {
        format!("{RESET_RELAUNCH_REQUIRED_MARKER}: {message}")
    } else {
        message
    };
    CsError::Config { msg }
}

fn agent_reset_error(mutation_started: bool, message: impl Into<String>) -> CsError {
    let message = message.into();
    let msg = if mutation_started {
        format!("{AGENT_RESET_RELAUNCH_REQUIRED_MARKER}: {message}")
    } else {
        message
    };
    CsError::Config { msg }
}

/// Full settings snapshot pushed to the Swift Settings UI.
///
/// Constructed only via [`CsSettings::from_runtime_snapshot`] from one sealed
/// `RuntimeSettingsSnapshot`. API keys are intentionally absent — they live
/// only in `CsKeyStatus` as booleans. Write back through `update_config` /
/// `update_config_many` using the router env keys.
#[derive(uniffi::Record)]
pub struct CsSettings {
    // ── Hotkeys ──
    pub hold_exclusive: bool,
    /// Assistive-arm modifier on hold base: `"shift"` (default) or `"cmd"` (W10-B).
    pub hold_arm_modifier: String,
    pub hold_start_delay_ms: u64,
    pub double_tap_interval_ms: u64,
    /// `TAP_MAX_MS`: longest Option press that still counts as a tap.
    pub tap_max_ms: u64,
    /// `TYPING_QUIET_MS`: Option taps count for nothing this long after typing.
    pub typing_quiet_ms: u64,
    pub toggle_silence_sec: f32,
    /// `WHISPER_CONTEXT_WINDOW_SEC`. Seconds of PCM each Layer 1 window covers.
    pub whisper_context_window_sec: f32,
    pub whisper_adaptive_buffer: bool,
    /// `CODESCRIBE_FORMAT_ON_DEVICE`: Apple system model formats first.
    pub format_on_device: bool,
    /// Voice Lab: the DictationTranscriber witness re-checks unresolved
    /// recovery debt after each sealed utterance. Effective value after the
    /// settings key plus the power-user env override.
    pub dictation_witness_armed: bool,
    /// Independent next-take publication and Apple producer choices.
    pub seal_ledger_enabled: bool,
    pub apple_live_enabled: bool,
    pub light_plus_sentence_pause_sec: f32,
    /// Lab ledger slider, -100 (strict) ..= 100 (loose); 0 is today's engine.
    pub ledger_sensitivity: i32,
    /// Deferred-insert chord (`DeferredInsertShortcut::wire_id()`), sourced
    /// from the canonical merged config snapshot. `"disabled"` is the
    /// product default when no persisted choice exists.
    pub deferred_insert_shortcut: String,
    /// Agent-channel modifier in effect (`ChannelModifier::as_str()`): `"ctrl"`
    /// or `"fn"`, or `"off"` when both keys record dictation. A key that
    /// records is never the channel modifier (one key, one role), so this can
    /// differ from the stored choice. Command is not a value.
    pub channel_modifier: String,
    /// Fn records dictation (Hold Fn/Globe or the Fn tap), so Settings offers
    /// no Fn channel modifier.
    pub fn_records_dictation: bool,
    /// A binding (Hold Fn/Globe) records with Fn, so freeing Fn takes a
    /// rebinding and not only turning the Fn tap off.
    pub fn_binding_records_dictation: bool,
    /// A Ctrl gesture records dictation, so Settings offers no Ctrl channel
    /// modifier.
    pub ctrl_records_dictation: bool,
    /// The widget chord (`WidgetShortcut::wire_id()`), e.g. `"ctrl+shift+b"`.
    pub widget_shortcut: String,
    /// The same chord with macOS glyphs (`WidgetShortcut::label()`), e.g. `"⌃⇧B"`.
    pub widget_shortcut_label: String,
    /// Quick Fn press below the hold delay toggles dictation. Default off.
    pub fn_tap_toggles_dictation: bool,
    /// Middle mouse button follows Fn press/release. Default off.
    /// The tap stays listen-only, so the click still reaches the frontmost app.
    pub middle_mouse_acts_as_fn: bool,
    /// A short middle click opens the overlay's agent chooser. Default off.
    /// While it is on, the middle button is Codescribe's, not the frontmost app's.
    pub middle_mouse_agent_gesture: bool,
    // ── Language ──
    pub whisper_language: CsLanguage,
    // ── AI / formatting ──
    /// Automatic paste policy (`PASTE_MODE`). Written back via `update_config`.
    pub paste_mode: CsPasteMode,
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
    /// Effective settings-only storage policy for the next capture.
    pub audio_retention: String,
    // ── History / quick notes ──
    pub history_enabled: bool,
    pub quick_notes_enabled: bool,
    pub quick_notes_save_only: bool,
    // ── STT backend ──
    pub use_local_stt: bool,
    pub local_model: String,
    pub stt_file_endpoint: Option<String>,
    pub stt_live_endpoint: Option<String>,
    // ── Clipboard ──
    pub restore_clipboard: bool,
    pub restore_clipboard_delay_ms: u64,
    // ── System / agent ──
    pub start_at_login: bool,
    pub agent_enter_sends: bool,
    pub agent_auto_send: bool,
    /// Agent-channel send hold in seconds; zero disables it. UI range is 0...30.
    pub agent_send_hold_sec: u32,
    /// Built-in Agent auto-send countdown, `1...30` seconds.
    pub agent_auto_send_delay_sec: u64,
    /// A built-in Agent take without voice ends after this, `10...120` seconds.
    pub agent_take_voiceless_end_sec: u64,
    /// `continuous` | `end_after_message`.
    pub agent_channel_session_policy: String,
    /// Effective channel send pause; `0` only from the env override (off).
    pub agent_channel_send_pause_sec: f32,
    /// `settings` | `env_override`: where that pause came from.
    pub agent_channel_send_pause_source: String,
    /// `0` = never, else `15...600` seconds.
    pub agent_channel_voiceless_close_sec: u64,
    /// `off` | `light_plus` | `formatter`.
    pub agent_channel_segment_finish: String,
    /// Formatter budget for one channel message, `1...10` seconds.
    pub agent_channel_format_wait_sec: u64,
    pub dump_audio_logs: bool,
    // ── Persisted lane selection and engine settings ──
    /// Lane = full ProviderRef (vendor ID or `custom:<slug>`) + model; provider first.
    pub llm_formatting_provider: Option<String>,
    pub llm_formatting_model: Option<String>,
    /// Formatting reasoning level: `none` (default), `low`, `medium`, `high`.
    pub llm_formatting_reasoning: String,
    pub llm_assistive_provider: Option<String>,
    pub llm_assistive_model: Option<String>,
    /// Agent reasoning level: `default` (the model's own) or a chosen level.
    pub llm_assistive_reasoning: String,
    /// Automatic formatting policy: off, correction or smart. A stored earlier
    /// `max`/`creative` reads as smart here; Max is invoked through Format.
    pub formatting_level: Option<String>,
    /// Formatter Keywords: preferred vocabulary given as context to
    /// Correction, Smart and Max. Normalized terms from the same seal; empty
    /// means no extra context. Written only through `set_formatter_keywords`.
    pub formatter_keywords: Vec<String>,
    pub whisper_model: Option<String>,
    /// Read-only diagnostic env override captured by the runtime snapshot.
    /// ASR mode owns the default; Settings never writes this value.
    pub layered_transcription: Option<String>,
    /// Workspace root directories the agent scans (`list_projects` tool) to
    /// resolve project names to paths (`AGENT_WORKSPACE_ROOTS`, colon-joined on
    /// the wire). Effective value: never empty here — defaults to
    /// `["~/.codescribe"]` so the Settings UI always shows the root the tool
    /// will actually scan. Written back via `update_config` with the same key
    /// (env-managed, NOT promoted).
    pub agent_workspace_roots: Vec<String>,
    pub buffer_delay_ms: Option<u64>,
    pub typing_cps: Option<f32>,
    pub emit_words_max: Option<u64>,
    pub buffered_interim_sec: Option<f32>,
    pub backend_max_upload_mb: Option<u64>,
    /// Product ASR lane (`CODESCRIBE_ASR_MODE`): `"apple_only"` |
    /// `"local_power"` | `"cloud"`. Written back via `update_config`.
    pub asr_mode: Option<String>,
    /// Lane this snapshot actually runs, `AsrProductMode::as_str()`:
    /// `UserSettings::resolved_asr_mode()` over the persisted mode, the consent
    /// record and the saved `use_local_stt` choice of the same seal. Read-only;
    /// Swift shows and gates on it and never re-derives it from `asr_mode`.
    pub resolved_asr_mode: String,
    /// Audio-egress consent (`CODESCRIBE_CLOUD_CONSENT`): `"granted"` |
    /// `"denied"`. Cloud without `granted` resolves to Apple-only.
    pub cloud_consent: Option<String>,
    /// Libraxis gateway session-mint URL (`CODESCRIBE_ASR_GATEWAY_URL`).
    /// Session mint, not the live WSS socket (`STT_ENDPOINT`).
    pub asr_gateway_url: Option<String>,
}

impl CsSettings {
    /// Secret-free DTO projection from one exact immutable runtime snapshot.
    ///
    /// Resolved `Config` values, persisted `UserSettings` intent, sealed LLM
    /// lanes, and sealed formatting policy all come from the same loader pass.
    /// No process-env / `.env` re-merge is performed here.
    fn from_runtime_snapshot(runtime: &RuntimeSettingsSnapshot) -> Self {
        let config = runtime.values();
        let settings = runtime.user_settings();
        let recording_keys = Config::recording_keys(settings, config.fn_tap_toggles_dictation);
        let formatting = runtime.llm_lanes().formatting();
        let assistive = runtime.llm_lanes().assistive();
        let mut agent_workspace_roots = normalize_agent_workspace_roots(
            settings.agent_workspace_roots.clone().unwrap_or_default(),
        );
        if agent_workspace_roots.is_empty() {
            agent_workspace_roots.push("~/.codescribe".to_string());
        }
        Self {
            hold_exclusive: config.hold_exclusive,
            hold_arm_modifier: config.hold_arm_modifier.as_str().to_string(),
            hold_start_delay_ms: config.hold_start_delay_ms,
            double_tap_interval_ms: config.double_tap_interval_ms,
            tap_max_ms: config.tap_max_ms,
            typing_quiet_ms: config.typing_quiet_ms,
            toggle_silence_sec: config.toggle_silence_sec,
            whisper_context_window_sec: config.whisper_context_window_sec,
            whisper_adaptive_buffer: config.whisper_adaptive_buffer,
            format_on_device: config.format_on_device,
            dictation_witness_armed: runtime.dictation_witness_armed(),
            seal_ledger_enabled: runtime.seal_ledger_enabled(),
            apple_live_enabled: runtime.apple_live_enabled(),
            light_plus_sentence_pause_sec: config.light_plus_sentence_pause_sec,
            ledger_sensitivity: config.ledger_sensitivity,
            deferred_insert_shortcut: config.deferred_insert_shortcut.wire_id().to_string(),
            channel_modifier: config.channel_modifier.as_str().to_string(),
            fn_records_dictation: recording_keys.fn_key,
            fn_binding_records_dictation: recording_keys.fn_binding,
            ctrl_records_dictation: recording_keys.ctrl,
            widget_shortcut: config.widget_shortcut.wire_id(),
            widget_shortcut_label: config.widget_shortcut.label(),
            fn_tap_toggles_dictation: config.fn_tap_toggles_dictation,
            middle_mouse_acts_as_fn: config.middle_mouse_acts_as_fn,
            middle_mouse_agent_gesture: config.middle_mouse_agent_gesture,
            whisper_language: CsLanguage::from(config.whisper_language),
            paste_mode: config.paste_mode.into(),
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
            // Saved user choice from the same seal; live device truth stays on
            // `CsAudioInputSnapshot`.
            audio_input_device: setting_string(settings.audio_input_device.clone()),
            audio_retention: config.audio_retention.as_str().to_string(),
            history_enabled: config.history_enabled,
            quick_notes_enabled: config.quick_notes_enabled,
            quick_notes_save_only: config.quick_notes_save_only,
            use_local_stt: config.use_local_stt,
            local_model: config.local_model.clone(),
            stt_file_endpoint: config.stt_file_endpoint.clone(),
            stt_live_endpoint: config.stt_live_endpoint.clone(),
            restore_clipboard: config.restore_clipboard,
            restore_clipboard_delay_ms: config.restore_clipboard_delay_ms,
            start_at_login: config.start_at_login,
            agent_enter_sends: config.agent_enter_sends,
            agent_auto_send: config.agent_auto_send,
            agent_send_hold_sec: runtime.agent_send_hold_sec(),
            agent_auto_send_delay_sec: config.agent_auto_send_delay_sec,
            agent_take_voiceless_end_sec: config.agent_take_voiceless_end_sec,
            agent_channel_session_policy: config.agent_channel.session_policy.as_str().to_string(),
            agent_channel_send_pause_sec: config.agent_channel.send_pause_sec,
            agent_channel_send_pause_source: config
                .agent_channel
                .send_pause_source
                .as_str()
                .to_string(),
            agent_channel_voiceless_close_sec: config.agent_channel.voiceless_close_sec,
            agent_channel_segment_finish: config.agent_channel.segment_finish.as_str().to_string(),
            agent_channel_format_wait_sec: config.agent_channel.format_wait_sec,
            dump_audio_logs: config.dump_audio_logs,
            // Editable Settings fields prefer persisted intent from the same
            // seal so a fresh UI write is visible; sealed lanes fill gaps when
            // the operator has no persisted row yet.
            llm_formatting_provider: setting_string(settings.llm_formatting_provider.clone())
                .or_else(|| Some(formatting.provider().as_string())),
            llm_formatting_model: setting_string(settings.llm_formatting_model.clone())
                .or_else(|| Some(formatting.model().to_string())),
            llm_formatting_reasoning: settings
                .llm_formatting_reasoning
                .as_deref()
                .and_then(codescribe_core::llm::provider::ReasoningEffort::parse)
                .unwrap_or_default()
                .as_str()
                .to_string(),
            llm_assistive_model: setting_string(settings.llm_assistive_model.clone())
                .or_else(|| Some(assistive.model().to_string())),
            llm_assistive_reasoning: settings
                .llm_assistive_reasoning
                .as_deref()
                .and_then(codescribe_core::llm::provider::ReasoningEffort::parse)
                .map_or("default", |effort| effort.as_str())
                .to_string(),
            llm_assistive_provider: setting_string(settings.llm_assistive_provider.clone())
                .or_else(|| Some(assistive.provider().as_string())),
            formatting_level: Some(runtime.automatic_formatting_policy().as_str().to_string()),
            formatter_keywords: runtime.formatter_keywords().to_vec(),
            whisper_model: setting_string(settings.whisper_model.clone()),
            layered_transcription: runtime.layered_transcription_override().map(str::to_owned),
            agent_workspace_roots,
            buffer_delay_ms: settings.buffer_delay_ms,
            typing_cps: settings.typing_cps,
            emit_words_max: settings.emit_words_max,
            buffered_interim_sec: settings.buffered_interim_sec,
            backend_max_upload_mb: settings.backend_max_upload_mb,
            asr_mode: setting_string(settings.asr_mode.clone()),
            // The session factory reads the same resolver on the same seal.
            resolved_asr_mode: settings.resolved_asr_mode().mode.as_str().to_string(),
            cloud_consent: setting_string(settings.cloud_consent.clone()),
            asr_gateway_url: setting_string(settings.asr_gateway_url.clone()),
        }
    }
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

/// Non-secret impact summary for the narrowly-scoped Agent reset.  Unlike the
/// app-data reset, this never counts or touches recordings, transcripts,
/// prompts, lexicon data, license state, or dictation preferences.
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct CsAgentResetPreview {
    pub threads: u64,
    pub files: u64,
    pub total_bytes: u64,
    pub secrets_present: bool,
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
    /// Flatten the core snapshot for the FFI record: the prompt path becomes a
    /// lossy UTF-8 string (Swift has no `PathBuf`) and the provenance enum
    /// collapses to its stable `as_str` tag.
    fn from(snapshot: PromptSnapshot) -> Self {
        Self {
            content: snapshot.content,
            path: snapshot.path.to_string_lossy().into_owned(),
            source: snapshot.source.as_str().to_string(),
            read_error: snapshot.read_error,
        }
    }
}

/// Resolve a formatting-policy id to the prompt file that policy edits.
/// `"off"` is rejected rather than silently mapped: the Off policy runs no LLM
/// pass, so it owns no prompt for the editor to open or restore.
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
    pub llm_libraxis_api_key_set: bool,
    pub llm_openai_api_key_set: bool,
    pub llm_xai_api_key_set: bool,
    pub llm_anthropic_api_key_set: bool,
    pub stt_file_api_key_set: bool,
    pub stt_live_api_key_set: bool,
    pub github_token_set: bool,
}

/// Automatic paste policy (`PASTE_MODE`), one-to-one with core [`PasteMode`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsPasteMode {
    /// Paste only into a focused, editable, non-password field; terminals
    /// only past the executable-content guard. Otherwise held on the clipboard.
    Safe,
    /// Paste wherever the caret is, terminals included; password fields and
    /// the terminal guard still hold.
    Comfort,
    /// Never paste automatically.
    Off,
}

impl From<PasteMode> for CsPasteMode {
    fn from(mode: PasteMode) -> Self {
        match mode {
            PasteMode::Safe => Self::Safe,
            PasteMode::Comfort => Self::Comfort,
            PasteMode::Off => Self::Off,
        }
    }
}

impl From<CsPasteMode> for PasteMode {
    fn from(mode: CsPasteMode) -> Self {
        match mode {
            CsPasteMode::Safe => Self::Safe,
            CsPasteMode::Comfort => Self::Comfort,
            CsPasteMode::Off => Self::Off,
        }
    }
}

/// UI-safe API-key liveness bucket. No variant carries secret material.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsApiKeyProbeStatus {
    /// The provider accepted this probe. This does not verify another model,
    /// OAuth account or a later request.
    Ok,
    /// The provider rejected the credential itself (401 / 403).
    Invalid,
    /// A usage, quota or billing limit (429 / 402 or an explicit marker).
    NoQuota,
    /// No verdict was reachable — transport failure or a provider 5xx.
    Network,
    /// Nothing is stored for this account, so no request was made.
    Missing,
    /// This account has no cheap liveness probe or belongs
    /// to no registered provider.
    Unsupported,
    /// The request was rejected without establishing credential acceptance.
    Rejected,
}

/// Result of one Settings "Test" action for a Keychain account.
#[derive(uniffi::Record)]
pub struct CsApiKeyProbeResult {
    pub account: String,
    pub status: CsApiKeyProbeStatus,
    pub message: String,
    pub probed_endpoint: Option<String>,
}

/// One Settings › Formatting › Test request, measured. `status` is one of
/// `applied`, `unchanged`, `skipped`, `failed`, `timed_out`. Never a secret.
#[derive(uniffi::Record)]
pub struct CsFormattingProbe {
    pub status: String,
    pub input: String,
    pub output: String,
    pub elapsed_ms: u64,
    /// The live formatter job's deadline the request had to meet.
    pub deadline_ms: u64,
    pub provider: String,
    pub model: String,
    /// The reasoning effort sent; `None` when the request carried no field.
    pub reasoning: Option<String>,
    /// Fixed diagnostic code, never a raw provider error or response body.
    pub failure_code: Option<String>,
    pub http_status: Option<u16>,
    /// Configured cloud route; excludes path, query and user information.
    pub endpoint_host: Option<String>,
    pub credential_source: String,
    pub checked_at_unix_ms: u64,
}

impl From<codescribe_core::llm::ai_formatting::FormattingProbe> for CsFormattingProbe {
    fn from(probe: codescribe_core::llm::ai_formatting::FormattingProbe) -> Self {
        use codescribe_core::llm::ai_formatting::FormattingProbeStatus as Status;
        Self {
            status: match probe.status {
                Status::Applied => "applied",
                Status::Unchanged => "unchanged",
                Status::Skipped => "skipped",
                Status::Failed => "failed",
                Status::TimedOut => "timed_out",
            }
            .to_string(),
            input: probe.input,
            output: probe.output,
            elapsed_ms: u64::try_from(probe.elapsed.as_millis()).unwrap_or(u64::MAX),
            deadline_ms: u64::try_from(probe.deadline.as_millis()).unwrap_or(u64::MAX),
            provider: probe.provider,
            model: probe.model,
            reasoning: probe.reasoning.map(str::to_string),
            failure_code: probe.failure.map(|failure| failure.code().to_string()),
            http_status: probe.failure.and_then(|failure| failure.http_status()),
            endpoint_host: probe.endpoint_host,
            credential_source: probe.credential_source.to_string(),
            checked_at_unix_ms: probe.checked_at_unix_ms,
        }
    }
}

/// One selectable model for a provider (id sent on the wire + display label).
#[derive(uniffi::Record)]
pub struct CsModelOption {
    pub id: String,
    pub display_name: String,
}

/// Live model discovery result for one provider. `status` is one of:
/// `"fresh"`, `"cached"`, `"no_key"`, `"key_rejected"` (the provider refused
/// the stored API key), `"error"`. Errors never carry secrets.
#[derive(uniffi::Record)]
pub struct CsModelDiscovery {
    pub provider_id: String,
    pub status: String,
    pub message: Option<String>,
    pub models: Vec<CsModelOption>,
}

/// Provider identity and credential presence; never a returned secret.
/// `account_identity` is who the stored id token says is signed in (email,
/// else subject) — `None` while signed out or without such a claim.
#[derive(uniffi::Record)]
pub struct CsProviderOption {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub wire: String,
    pub endpoint: String,
    pub api_key_account: String,
    pub api_key_set: bool,
    pub key_required: bool,
    pub account_signed_in: bool,
    pub account_login_enabled: bool,
    pub account_status_message: String,
    pub account_identity: Option<String>,
    pub oauth_client_id: Option<String>,
}

/// Secret-free credential projection captured at one cache revision.
#[derive(uniffi::Record)]
pub struct CsProviderAccessSnapshot {
    pub providers: Vec<CsProviderOption>,
    pub account_errors: std::collections::HashMap<String, String>,
    pub key_status: CsKeyStatus,
    pub stt_lanes: Vec<CsSttLane>,
    pub revision: u64,
}

/// One STT endpoint and its credential presence; secrets never leave Keychain.
#[derive(uniffi::Record)]
pub struct CsSttLane {
    pub id: String,
    pub title: String,
    pub accepts: String,
    pub placeholder: String,
    pub endpoint: Option<String>,
    pub endpoint_wire_key: String,
    pub key_account: String,
    pub api_key_set: bool,
}

/// Input-only secret: consumed into the Keychain bundle, never returned.
#[derive(uniffi::Record)]
pub struct CsCustomProviderDraft {
    pub name: String,
    pub wire: String,
    pub endpoint: String,
    pub api_key: Option<String>,
}

#[derive(uniffi::Record)]
pub struct CsCustomProviderRemoval {
    pub id: String,
    pub lanes_reset: Vec<CsLlmLane>,
}

/// Stable lane identity used by the secret-free runtime lane projection.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsLlmLane {
    /// Automatic post-dictation formatting lane.
    Formatting,
    /// Assistive/agent lane (act-on-selection and voice chat).
    Assistive,
}

impl From<CsLlmLane> for RuntimeLlmLaneKind {
    /// Cross the FFI boundary inward: the bridge enum exists only so
    /// `RuntimeLlmLaneKind` itself never has to be exported to Swift.
    fn from(value: CsLlmLane) -> Self {
        match value {
            CsLlmLane::Formatting => Self::Formatting,
            CsLlmLane::Assistive => Self::Assistive,
        }
    }
}

/// Why a lane cannot send a request, as data: Swift composes the sentence
/// (`LLMLaneUnavailablePresentation.swift`). Provider names, endpoints and
/// Keychain account names travel verbatim.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum CsLlmLaneUnavailable {
    /// The lane points at a Custom provider id that no longer exists.
    CustomProviderMissing { custom_id: String },
    /// The provider has no model selected.
    ModelMissing { provider: String },
    /// No Keychain credential and no supported signed-in provider account.
    CredentialMissing {
        lane: CsLlmLane,
        provider: String,
        endpoint: String,
        key_account: String,
    },
    /// Cloud Code Assist needs Google account credentials or authorized-user ADC.
    CloudCodeAssistAccountRequired { lane: CsLlmLane },
    /// ACP program/cwd not absolute, or argv not literal.
    AcpLaunchInvalid,
    /// ACP needs explicit absolute `system.agent_workspace_roots`.
    AcpWorkspaceRootsMissing,
    /// ACP cwd is outside every configured Agent workspace root.
    AcpCwdOutsideWorkspace,
}

impl From<RuntimeLlmLaneKind> for CsLlmLane {
    fn from(value: RuntimeLlmLaneKind) -> Self {
        match value {
            RuntimeLlmLaneKind::Formatting => Self::Formatting,
            RuntimeLlmLaneKind::Assistive => Self::Assistive,
        }
    }
}

impl From<&RuntimeLlmLaneUnavailable> for CsLlmLaneUnavailable {
    fn from(value: &RuntimeLlmLaneUnavailable) -> Self {
        match value {
            RuntimeLlmLaneUnavailable::CustomProviderMissing { custom_id } => {
                Self::CustomProviderMissing {
                    custom_id: custom_id.clone(),
                }
            }
            RuntimeLlmLaneUnavailable::ModelMissing { provider } => Self::ModelMissing {
                provider: provider.clone(),
            },
            RuntimeLlmLaneUnavailable::CredentialMissing {
                lane,
                provider,
                endpoint,
                key_account,
            } => Self::CredentialMissing {
                lane: (*lane).into(),
                provider: provider.clone(),
                endpoint: endpoint.clone(),
                key_account: key_account.clone(),
            },
            RuntimeLlmLaneUnavailable::CloudCodeAssistAccountRequired { lane } => {
                Self::CloudCodeAssistAccountRequired {
                    lane: (*lane).into(),
                }
            }
            RuntimeLlmLaneUnavailable::AcpLaunchInvalid => Self::AcpLaunchInvalid,
            RuntimeLlmLaneUnavailable::AcpWorkspaceRootsMissing => Self::AcpWorkspaceRootsMissing,
            RuntimeLlmLaneUnavailable::AcpCwdOutsideWorkspace => Self::AcpCwdOutsideWorkspace,
        }
    }
}

/// Complete canonical projection for one sealed LLM lane. Credentials never cross the
/// bridge: only the owning account name and presence/auth booleans are exposed.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsRuntimeLlmLane {
    pub lane: CsLlmLane,
    pub provider_id: String,
    pub provider_display_name: String,
    pub wire: String,
    pub endpoint: String,
    pub model: String,
    pub key_account: String,
    pub key_present: bool,
    pub account_auth: bool,
    pub available: bool,
    pub unavailable: Option<CsLlmLaneUnavailable>,
}

impl From<&RuntimeLlmLane> for CsRuntimeLlmLane {
    /// Secret-free projection outward from the loader-owned lane.
    fn from(value: &RuntimeLlmLane) -> Self {
        Self {
            lane: value.lane().into(),
            provider_id: value.provider().as_string(),
            provider_display_name: value.provider_display_name().to_string(),
            wire: value.wire_family().as_str().to_string(),
            endpoint: value.endpoint().to_string(),
            model: value.model().to_string(),
            key_account: value.credential().key_account().to_string(),
            key_present: value.credential().api_key().is_some(),
            account_auth: value.credential().account_auth(),
            available: value.available(),
            unavailable: value.unavailable().map(CsLlmLaneUnavailable::from),
        }
    }
}

#[derive(Default)]
struct CachedRuntimeSnapshot {
    value: Option<(Option<std::time::SystemTime>, RuntimeSettingsSnapshot)>,
    generation: u64,
}

/// Metadata-only fingerprint shared by the runtime lane cache and its UI consumers.
fn runtime_settings_modified_at() -> Option<std::time::SystemTime> {
    fs::metadata(UserSettings::settings_path())
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn last_good_runtime_snapshot() -> &'static Mutex<CachedRuntimeSnapshot> {
    static LAST_GOOD: OnceLock<Mutex<CachedRuntimeSnapshot>> = OnceLock::new();
    LAST_GOOD.get_or_init(|| Mutex::new(CachedRuntimeSnapshot::default()))
}

#[cfg(test)]
static RUNTIME_SNAPSHOT_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static INVALIDATE_AFTER_RUNTIME_LOAD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Drop the last-good lane snapshot. Credential mutations (Keychain / OAuth)
/// never touch `settings.json`, so mtime is not a sufficient cache key.
pub(crate) fn invalidate_runtime_snapshot_cache() {
    let mut guard = last_good_runtime_snapshot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.value = None;
    guard.generation = guard.generation.wrapping_add(1);
}

/// One loader snapshot for lane projection: reuse the last good value when the
/// file mtime is unchanged, and never panic the UI if a transient read fails.
fn load_runtime_snapshot_for_lane() -> RuntimeSettingsSnapshot {
    let mtime = runtime_settings_modified_at();
    let generation = {
        let guard = last_good_runtime_snapshot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_mtime, snapshot)) = guard.value.as_ref()
            && *cached_mtime == mtime
        {
            return snapshot.clone();
        }
        guard.generation
    };
    #[cfg(test)]
    RUNTIME_SNAPSHOT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let snapshot = match Config::load_runtime_snapshot_without_keychain() {
        Ok(snapshot) => snapshot,
        Err(_) => last_good_runtime_snapshot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .value
            .as_ref()
            .map(|(_, snapshot)| snapshot.clone())
            .unwrap_or_else(|| Config::load_startup_runtime_snapshot(false)),
    };
    #[cfg(test)]
    if INVALIDATE_AFTER_RUNTIME_LOAD.swap(false, std::sync::atomic::Ordering::SeqCst) {
        invalidate_runtime_snapshot_cache();
    }
    let mut guard = last_good_runtime_snapshot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // A credential mutation may invalidate while this loader is working.
    // Its earlier projection must not repopulate the cleared cache.
    if guard.generation == generation {
        guard.value = Some((mtime, snapshot.clone()));
    }
    snapshot
}

/// Project one lane from a single loader snapshot without exposing secrets.
#[uniffi::export]
pub fn runtime_llm_lane(lane: CsLlmLane) -> CsRuntimeLlmLane {
    load_runtime_snapshot_for_lane()
        .llm_lanes()
        .lane(lane.into())
        .into()
}

/// Last stop-path serving verdict from the controller runtime owner.
/// The Settings "Active STT" row consumes this — never configured preference.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsLastServingVerdict {
    /// Actual engine label (`local_apple`, `local_whisper`, `streaming_whisper`, `cloud_stt`).
    pub engine: String,
    /// Final-pass routing mode that governed the stop (`smart` / `always` / `off`).
    pub routing_mode: String,
    /// Final-pass disposition when one ran (`skipped`, `changed`, …).
    pub disposition: Option<String>,
    /// True when the serving engine was a runtime fallback (e.g. Apple→Whisper).
    pub fallback_used: bool,
}

/// Snapshot the last serving verdict, if any stop completed in this process.
/// `None` renders as "Not yet served" Swift-side.
#[uniffi::export]
pub fn current_serving_verdict() -> Option<CsLastServingVerdict> {
    codescribe::controller::serving_status::current_last_serving().map(|verdict| {
        CsLastServingVerdict {
            engine: verdict.engine,
            routing_mode: verdict.routing_mode,
            disposition: verdict.disposition,
            fallback_used: verdict.fallback_used,
        }
    })
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
    /// User-owned automatic paste policy. Assistive and controller safety
    /// vetoes can still prevent or hold a paste for a particular recording.
    pub paste_mode: CsPasteMode,
    /// Read-only projection `paste_mode != off` for binary on/off surfaces.
    /// Never persisted; `paste_mode` is the one stored choice.
    pub auto_paste_enabled: bool,
    /// Normalized automatic formatting policy: off, correction or smart.
    /// A stored earlier `max`/`creative` value reads as smart.
    pub formatting_level: String,
    /// UI-initiated recording starts in the assistive lane when enabled.
    pub start_assistive: bool,
    /// Notes Mode: voice → daily note (no paste). Backed by the core
    /// `quick_notes_enabled` + `quick_notes_save_only` pair, flipped together.
    pub notes_mode_enabled: bool,
    /// Cursor-following recording indicator visibility.
    pub hold_indicator: bool,
    /// Base indicator diameter. Assistive mode keeps its existing multiplier.
    pub hold_badge_size: u32,
}

/// Thin handle to the codescribe config engine. Stateless: each method reloads
/// or writes through the live `Config` / `UserSettings` / Keychain so Swift
/// always sees on-disk truth.
#[derive(uniffi::Object)]
pub struct CodescribeConfig {}

#[uniffi::export]
impl CodescribeConfig {
    /// Construct the bridge handle. Not a pure allocation: this is the app's
    /// first-touch entry point, so it also initializes logging and resumes run
    /// monitors persisted by a previous process. Later handles
    /// are cheap — the side effects are guarded to run only once, and are
    /// skipped entirely under `cfg!(test)`.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        // Resume any run monitors persisted by a previous app process so a
        // watched Vibecrafted run keeps heartbeating after restart.
        if !cfg!(test) {
            codescribe::agent::monitor::resume_monitors_on_startup();
        }
        Self {}
    }

    /// Full settings snapshot for the Settings UI. Loads exactly one canonical
    /// `RuntimeSettingsSnapshot` and projects a secret-free `CsSettings` from
    /// that instance — never a second `Config::load` + `UserSettings::load` +
    /// env-file reconstruct.
    pub fn load_settings(&self) -> CsSettings {
        let runtime = Config::load_runtime_snapshot_without_keychain()
            .expect("canonical runtime settings must load for Settings UI projection");
        CsSettings::from_runtime_snapshot(&runtime)
    }

    /// Diagnostic projection without credential imports. The canonical loader
    /// may repair settings; a recorded refusal must not become apparent success.
    pub fn load_diagnostic_settings(&self) -> Result<CsSettings, CsError> {
        let runtime = Config::load_startup_runtime_snapshot(false);
        if !runtime.repair_receipt().unrepairable.is_empty() {
            return Err(CsError::Config {
                msg: "Configuration refusal recorded in this process".into(),
            });
        }
        Ok(CsSettings::from_runtime_snapshot(&runtime))
    }

    /// Read the presentation preference from the canonical settings snapshot.
    pub fn overlay_expanded_by_default(&self) -> bool {
        // Show the transcript at take start unless the user explicitly opted
        // out. The doc line above is part of the UniFFI checksum: keep it.
        match Config::load_runtime_snapshot_without_keychain() {
            Ok(snapshot) => snapshot
                .user_settings()
                .show_transcript_at_take_start
                .unwrap_or(true),
            Err(error) => {
                tracing::warn!(%error, "overlay take-start preference could not be loaded; showing transcript");
                true
            }
        }
    }

    /// Persist only the preferred presentation; never change live capture.
    pub fn set_overlay_expanded_by_default(&self, enabled: bool) -> bool {
        let mut settings = UserSettings::load();
        settings.show_transcript_at_take_start = Some(enabled);
        if let Err(error) = settings.save() {
            tracing::warn!(%error, "overlay expansion preference could not be saved");
            return false;
        }
        true
    }

    /// Read the user-visible pin from the canonical settings snapshot.
    pub fn overlay_keep_visible_between_takes(&self) -> bool {
        Config::load_runtime_snapshot_without_keychain()
            .expect("canonical runtime settings must load for overlay pin")
            .user_settings()
            .overlay_keep_visible_between_takes
            .unwrap_or(false)
    }

    /// Persist the pin only on a changed user choice.
    pub fn set_overlay_keep_visible_between_takes(&self, enabled: bool) -> bool {
        let mut settings = UserSettings::load();
        if settings.overlay_keep_visible_between_takes == Some(enabled) {
            return true;
        }
        settings.overlay_keep_visible_between_takes = Some(enabled);
        if let Err(error) = settings.save() {
            tracing::warn!(%error, "overlay pin preference could not be saved");
            return false;
        }
        true
    }

    /// Read the auto-fold interval. `0` means never. Unset, or a stored value
    /// outside {0, 5, 10, 30, 60}, reads as 10 seconds.
    pub fn overlay_auto_fold_seconds(&self) -> u32 {
        // The doc lines above are part of the UniFFI checksum: keep them. A
        // settings read that fails keeps the default instead of panicking
        // across the FFI boundary at overlay attach.
        let stored = match Config::load_runtime_snapshot_without_keychain() {
            Ok(snapshot) => snapshot.user_settings().overlay_auto_fold_seconds,
            Err(error) => {
                tracing::warn!(%error, "overlay auto-fold preference could not be loaded; folding after 10 s");
                None
            }
        };
        match stored {
            Some(seconds) if matches!(seconds, 0 | 5 | 10 | 30 | 60) => seconds,
            _ => 10,
        }
    }

    /// Persist the auto-fold interval. `0` means never. A value outside
    /// {0, 5, 10, 30, 60} is refused and nothing is written. An unchanged
    /// value does not rewrite settings.
    pub fn set_overlay_auto_fold_seconds(&self, seconds: u32) -> bool {
        if !matches!(seconds, 0 | 5 | 10 | 30 | 60) {
            return false;
        }
        let mut settings = UserSettings::load();
        if settings.overlay_auto_fold_seconds == Some(seconds) {
            return true;
        }
        settings.overlay_auto_fold_seconds = Some(seconds);
        if let Err(error) = settings.save() {
            tracing::warn!(%error, "overlay auto-fold preference could not be saved");
            return false;
        }
        true
    }

    /// Persist the paste mode and return the prompt-free post-write truth in
    /// one result. Callers may re-read `tray_toggles()` after an error; no
    /// optimistic bridge cache is retained.
    pub fn set_paste_mode(&self, mode: CsPasteMode) -> Result<CsTrayToggles, CsError> {
        let mut settings = UserSettings::load();
        settings.paste_mode = Some(mode.into());
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(self.tray_toggles())
    }

    /// On/off switch over the same stored `paste_mode`, for binary surfaces
    /// (the overlay chip). Off stores `off`; on re-arms `safe` only when the
    /// mode is `off`, so an armed `comfort` choice survives a redundant on.
    pub fn set_auto_paste_enabled(&self, enabled: bool) -> Result<CsTrayToggles, CsError> {
        let current = self.tray_toggles().paste_mode;
        let mode = match (enabled, current) {
            (false, _) => CsPasteMode::Off,
            (true, CsPasteMode::Off) => CsPasteMode::Safe,
            (true, armed) => armed,
        };
        self.set_paste_mode(mode)
    }

    /// Persist a normalized Auto Format policy and return prompt-free
    /// post-write truth. Only off, correction and smart are automatic; Max and
    /// unknown policy IDs fail without changing disk state.
    pub fn set_auto_format_level(&self, level: String) -> Result<CsTrayToggles, CsError> {
        let policy = FormattingPolicy::parse(&level).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        // Max is the overlay assistant, invoked for one operation through
        // Format. It is never stored as the automatic policy; the explicit
        // formatter APIs keep selecting it per request.
        if policy == FormattingPolicy::Max {
            return Err(CsError::Config {
                msg: format!(
                    "{level:?} is not an automatic formatting level; Max runs only when \
                     invoked through Format. Expected off, correction, or smart"
                ),
            });
        }
        let mut settings = UserSettings::load();
        settings.formatting_level = Some(policy.as_str().to_string());
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(self.tray_toggles())
    }

    /// Saved formatter Keywords, normalized, from one runtime snapshot. A
    /// failed read serves the last good snapshot instead of an empty list.
    pub fn get_formatter_keywords(&self) -> Vec<String> {
        load_runtime_snapshot_for_lane()
            .formatter_keywords()
            .to_vec()
    }

    /// Replace the formatter Keywords and return the normalized stored list.
    /// Only `formatter_keywords` changes; the automatic level and every other
    /// setting keep their stored values. The settings owner refuses an
    /// oversized list whole; nothing is truncated and the error is returned.
    /// An empty list removes the Keywords.
    pub fn set_formatter_keywords(&self, keywords: Vec<String>) -> Result<Vec<String>, CsError> {
        let mut settings = UserSettings::load();
        settings.formatter_keywords = (!keywords.is_empty()).then_some(keywords);
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        // The returned list is what the next formatting request receives:
        // the snapshot sealed from the document just written.
        let runtime =
            Config::load_runtime_snapshot_without_keychain().map_err(|error| CsError::Config {
                msg: error.to_string(),
            })?;
        reload_hotkey_runtime();
        crate::hotkeys::refresh_live_controller_config();
        Ok(runtime.formatter_keywords().to_vec())
    }

    /// Lightweight tray-only settings read uses files, env and cached credentials.
    /// It never prompts just because the user opened the menu. Projects from one
    /// keychain-free runtime snapshot.
    pub fn tray_toggles(&self) -> CsTrayToggles {
        let runtime = Config::load_runtime_snapshot_without_keychain()
            .expect("canonical runtime settings must load for tray toggles");
        let config = runtime.values();
        CsTrayToggles {
            show_dock_icon: config.show_dock_icon,
            transcription_overlay_enabled: config.transcription_overlay_enabled,
            paste_mode: config.paste_mode.into(),
            auto_paste_enabled: config.paste_mode != PasteMode::Off,
            formatting_level: runtime.automatic_formatting_policy().as_str().to_string(),
            start_assistive: config.tray_start_assistive,
            // Notes Mode is "on" only when BOTH flags are set (dictation → note
            // AND no paste). Reading just quick_notes_enabled could show the toggle
            // ON while dictation still pastes (save_only=false) — an edge desync.
            notes_mode_enabled: config.quick_notes_enabled && config.quick_notes_save_only,
            hold_indicator: config.hold_indicator,
            hold_badge_size: config.hold_badge_size,
        }
    }

    /// Persist one config value, auto-tiered by the core router
    /// (`save_to_env`): API keys → Keychain, promoted keys → settings.json,
    /// power-user keys → `.env`. Runtime readers reload persisted snapshots
    /// instead of mutating the process env.
    pub fn update_config(&self, key: String, value: String) -> Result<(), CsError> {
        validate_provider_setting(&key, &value)?;
        Config::load_without_keychain()
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
        for entry in &entries {
            validate_provider_setting(&entry.key, &entry.value)?;
        }
        let pairs: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.key.as_str(), entry.value.as_str()))
            .collect();
        Config::load_without_keychain()
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

    /// Absolute path to the high-churn data folder (`<config dir>/data.noindex`),
    /// resolved by `Config::churn_dir`. Swift places its diagnostic log and
    /// overlay caches here instead of rebuilding the path.
    pub fn churn_dir(&self) -> String {
        Config::churn_dir().to_string_lossy().to_string()
    }

    /// Settings JSON belongs to the settings loader, not the app-data directory.
    /// Resolves the path only; does not load credentials or create a file.
    pub fn settings_file_path(&self) -> String {
        UserSettings::settings_path().to_string_lossy().to_string()
    }

    pub fn key_status(&self) -> CsKeyStatus {
        CsKeyStatus {
            llm_libraxis_api_key_set: keychain::key_present("LLM_LIBRAXIS_API_KEY"),
            llm_openai_api_key_set: keychain::key_present("LLM_OPENAI_API_KEY"),
            llm_xai_api_key_set: keychain::key_present("LLM_XAI_API_KEY"),
            llm_anthropic_api_key_set: keychain::key_present("LLM_ANTHROPIC_API_KEY"),
            stt_file_api_key_set: keychain::key_present("STT_FILE_API_KEY"),
            stt_live_api_key_set: keychain::key_present("STT_LIVE_API_KEY"),
            github_token_set: keychain::key_present("GITHUB_TOKEN"),
        }
    }

    pub fn test_api_key(&self, account: String) -> Result<CsApiKeyProbeResult, CsError> {
        ensure_known_account(&account)?;
        let provider = ProviderRegistry::from_settings(&UserSettings::load_projection())
            .all()
            .into_iter()
            .find(|provider| provider.key_account == account);
        Ok(probe_api_key_liveness(&account, provider.as_ref()).into())
    }

    /// Settings › Formatting › Test: format `sample` once through the live
    /// formatter path (empty `sample` sends the built-in raw transcript).
    /// Blocks for up to the live deadline; Swift calls it off MainActor.
    pub fn test_formatting(&self, sample: String) -> Result<CsFormattingProbe, CsError> {
        let runtime = load_runtime_snapshot_for_lane();
        let language = runtime
            .values()
            .whisper_language
            .whisper_hint()
            .map(str::to_string);
        let probe = application_runtime::block_on(async move {
            codescribe_core::llm::ai_formatting::probe_formatting(
                Some(&sample),
                language.as_deref(),
                &runtime,
            )
            .await
        })?;
        Ok(probe.into())
    }

    /// Explicit credential acquisition. Swift executes this off MainActor.
    ///
    /// This is the only authorized path that fills the process credential
    /// cache, so it also re-seals the live controller through the one
    /// settings-refresh path. Launch creates the controller before Swift
    /// restores provider access, and a Keychain-only secret is therefore absent
    /// from that first sealed generation: without this re-seal the Cloud lane
    /// reports `live_key_missing` on every take until an unrelated settings
    /// write happens to re-seal it.
    pub fn provider_access_snapshot(&self) -> Result<CsProviderAccessSnapshot, CsError> {
        let previous_revision = keychain::bundle_revision();
        keychain::refresh_bundle().map_err(provider_error)?;
        // Authorized background refresh also completes pending credential imports
        // through the existing loader; passive UI projections leave them pending.
        let _ = Config::load_runtime_snapshot().map_err(provider_error)?;
        let revision = keychain::bundle_revision();
        if revision != previous_revision {
            invalidate_runtime_snapshot_cache();
        }
        // Unconditional, not gated on `revision`: a coalesced bundle read keeps
        // the revision while still being the moment the controller's sealed
        // generation is known to be credential-complete. The refresh is a no-op
        // before the controller exists and defers to the next take while one is
        // active, so it can never interrupt capture.
        crate::hotkeys::refresh_live_controller_config();
        let registry = ProviderRegistry::from_settings(&UserSettings::load());
        let mut providers = Vec::new();
        let mut account_errors = std::collections::HashMap::new();
        for provider in registry.all() {
            let account_unavailable = if provider.wire == WireFamily::GoogleGenerateContent {
                account_auth::google_credential_status().is_err()
            } else {
                provider
                    .oauth_vendor
                    .is_some_and(|vendor| account_auth::account_status_snapshot(vendor).is_err())
            };
            let option = provider_option(provider);
            if account_unavailable {
                // Never expose token JSON or its decoder details in public UI metadata.
                account_errors.insert(option.id.clone(), option.account_status_message.clone());
            }
            providers.push(option);
        }
        Ok(CsProviderAccessSnapshot {
            providers,
            account_errors,
            key_status: self.key_status(),
            stt_lanes: self.stt_lanes(),
            revision,
        })
    }

    /// Cache-only revision check; it never waits for credential I/O.
    pub fn provider_access_revision(&self) -> u64 {
        keychain::bundle_revision()
    }

    /// Opaque palette cache stamp over the runtime cache's canonical settings
    /// mtime, invalidation generation and the existing credential revision.
    /// One metadata lookup and cache-only locks, no
    /// settings parsing, lane projection, provider registry or credential I/O.
    /// Missing/unreadable metadata refuses cache reuse rather than certifying
    /// that a previous projection is still current.
    pub fn composer_model_cache_stamp(&self) -> Option<String> {
        runtime_settings_modified_at().map(|mtime| {
            let generation = last_good_runtime_snapshot()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .generation;
            format!("{}:{generation}:{mtime:?}", keychain::bundle_revision())
        })
    }

    pub fn available_providers(&self) -> Vec<CsProviderOption> {
        ProviderRegistry::from_settings(&UserSettings::load_projection())
            .all()
            .into_iter()
            .map(provider_option)
            .collect()
    }

    pub fn add_custom_provider(
        &self,
        draft: CsCustomProviderDraft,
    ) -> Result<CsProviderOption, CsError> {
        let wire = parse_wire(&draft.wire)?;
        let row =
            CustomProvider::new(&draft.name, wire, &draft.endpoint).map_err(provider_error)?;
        let mut settings = UserSettings::load();
        settings
            .add_custom_provider(row.clone())
            .map_err(provider_error)?;
        persist_custom_provider(&settings, &row, draft.api_key)
    }

    pub fn update_custom_provider(
        &self,
        id: String,
        draft: CsCustomProviderDraft,
    ) -> Result<CsProviderOption, CsError> {
        let mut settings = UserSettings::load();
        let row = settings
            .update_custom_provider(&id, &draft.name, parse_wire(&draft.wire)?, &draft.endpoint)
            .map_err(provider_error)?;
        persist_custom_provider(&settings, &row, draft.api_key)
    }

    pub fn remove_custom_provider(&self, id: String) -> Result<CsCustomProviderRemoval, CsError> {
        let mut settings = UserSettings::load();
        let removed = settings
            .remove_custom_provider(&id)
            .map_err(provider_error)?;
        let account = removed.provider.key_account();
        let removed = UserSettings::with_credential_edit(&account, |latest| {
            let removed = latest.remove_custom_provider(&id)?;
            delete_key(&account)?;
            Ok(removed)
        })
        .map_err(provider_error)?;
        keychain::advance_bundle_revision();
        crate::hotkeys::refresh_live_controller_config();
        Ok(CsCustomProviderRemoval {
            id: removed.provider.id,
            lanes_reset: removed
                .lanes_reset
                .into_iter()
                .map(|lane| match lane {
                    RuntimeLlmLaneKind::Formatting => CsLlmLane::Formatting,
                    RuntimeLlmLaneKind::Assistive => CsLlmLane::Assistive,
                })
                .collect(),
        })
    }

    pub fn set_lane_provider(&self, lane: CsLlmLane, provider_id: String) -> Result<(), CsError> {
        let key = match lane {
            CsLlmLane::Formatting => "LLM_FORMATTING_PROVIDER",
            CsLlmLane::Assistive => "LLM_ASSISTIVE_PROVIDER",
        };
        self.update_config(key.to_string(), provider_id)
    }

    /// Start provider-account login for the selected provider.
    ///
    /// Flow is chosen from the provider's OAuth registry row:
    /// - **Loopback** (OpenAI): bind local callback, open authorize URL.
    /// - **Device code** (xAI / OpenCode SuperGrok): request RFC 8628 device
    ///   code, open verification URL (no `127.0.0.1` listener — consent cannot
    ///   hang waiting on a dead loopback port).
    /// - **Paste code** (Anthropic): refused here until a paste UI lands.
    ///
    /// Gated by the provider's client id (settings / env / shipped default).
    pub fn start_account_login(
        &self,
        provider_id: String,
    ) -> Result<CsAccountLoginResult, CsError> {
        let provider = ProviderKind::from_str(&provider_id).map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        let client_id =
            account_auth::client_id_for_provider(provider).map_err(account_auth_to_cs)?;
        let oauth = account_auth::provider_oauth_config(provider).map_err(account_auth_to_cs)?;

        // Supersede any prior attempt (loopback or device) so fixed ports and
        // poll loops never race two logins.
        {
            let mut guard = active_account_login().lock().map_err(|_| CsError::Config {
                msg: "account login state lock poisoned".to_string(),
            })?;
            if let Some(previous) = guard.take() {
                previous.cancel();
            }
        }

        match oauth.login_flow {
            account_auth::LoginFlow::DeviceCode => {
                let mut config = account_auth::DeviceAuthConfig::new(provider, client_id);
                // Await will re-clamp; start uses the full default budget.
                config.max_wait = std::time::Duration::from_secs(15 * 60);
                let request_config = config.clone();
                let device = application_runtime::block_on(async move {
                    account_auth::request_device_code(&request_config).await
                })?
                .map_err(account_auth_to_cs)?;
                let auth_url = device.verification_url.clone();
                let user_code = device.user_code.clone();
                let mut guard = active_account_login().lock().map_err(|_| CsError::Config {
                    msg: "account login state lock poisoned".to_string(),
                })?;
                *guard = Some(PendingAccountLogin::Device { config, device });
                Ok(CsAccountLoginResult {
                    provider_id: provider.as_str().to_string(),
                    status: "started".to_string(),
                    message: format!(
                        "open the browser and approve access (code {user_code} if asked)"
                    ),
                    auth_url: Some(auth_url),
                    signed_in: false,
                    client_id_configured: true,
                })
            }
            account_auth::LoginFlow::Loopback => {
                let opts = account_auth::ServerOptions::new(provider, client_id)
                    .map_err(account_auth_to_cs)?;
                let login = application_runtime::block_on(account_auth::run_login_server(opts))?
                    .map_err(account_auth_to_cs)?;
                let auth_url = login.auth_url.clone();
                let mut guard = active_account_login().lock().map_err(|_| CsError::Config {
                    msg: "account login state lock poisoned".to_string(),
                })?;
                *guard = Some(PendingAccountLogin::Loopback(login));
                Ok(CsAccountLoginResult {
                    provider_id: provider.as_str().to_string(),
                    status: "started".to_string(),
                    message: "open the browser to finish sign-in".to_string(),
                    auth_url: Some(auth_url),
                    signed_in: false,
                    client_id_configured: true,
                })
            }
            account_auth::LoginFlow::PasteCode => Err(CsError::Config {
                msg: format!(
                    "{} signs in by pasting a code; that UI is not wired yet",
                    provider.display_name()
                ),
            }),
        }
    }

    /// Block until the in-flight provider-account login completes, fails, or
    /// times out. Swift calls this from a background queue right after
    /// `start_account_login` opened the browser. On timeout the pending
    /// loopback server is shut down (or the device poll is abandoned). A
    /// second `start_account_login` while pending cancels the first.
    ///
    /// P2-09: 300s default (from caller) is intentional; OAuth human steps can
    /// exceed short timeouts. No configurability knobs.
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

        let timeout = std::time::Duration::from_secs(timeout_seconds.max(1));
        match login {
            PendingAccountLogin::Loopback(login) => {
                let cancel = login.cancel_handle();
                let outcome = application_runtime::block_on(async move {
                    tokio::time::timeout(timeout, login.block_until_done()).await
                })?;
                match outcome {
                    Ok(Ok(())) => {
                        invalidate_runtime_snapshot_cache();
                        let message = account_auth::account_status(provider).message;
                        Ok(account_login_result(provider, "signed_in", &message))
                    }
                    Ok(Err(error)) => {
                        Ok(account_login_result(provider, "failed", &error.to_string()))
                    }
                    Err(_elapsed) => {
                        cancel.shutdown();
                        let message = format!(
                            "sign-in was not completed within {timeout_seconds}s; the local login server was shut down"
                        );
                        Ok(account_login_result(provider, "timeout", &message))
                    }
                }
            }
            PendingAccountLogin::Device { mut config, device } => {
                config.max_wait = timeout;
                let outcome = application_runtime::block_on(async move {
                    tokio::time::timeout(
                        timeout,
                        account_auth::complete_device_code_login(&config, &device),
                    )
                    .await
                })?;
                match outcome {
                    Ok(Ok(())) => {
                        invalidate_runtime_snapshot_cache();
                        let message = account_auth::account_status(provider).message;
                        Ok(account_login_result(provider, "signed_in", &message))
                    }
                    Ok(Err(error)) => {
                        Ok(account_login_result(provider, "failed", &error.to_string()))
                    }
                    Err(_elapsed) => {
                        let message = format!(
                            "sign-in was not completed within {timeout_seconds}s; device authorization abandoned"
                        );
                        Ok(account_login_result(provider, "timeout", &message))
                    }
                }
            }
        }
    }

    /// Cancel any in-flight provider-account login (loopback port or device poll).
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
        account_auth::clear_account_tokens(provider).map_err(account_auth_to_cs)?;
        invalidate_runtime_snapshot_cache();
        Ok(())
    }

    /// Discover model options from the selected provider using the live provider
    /// `/models` API plus the existing config/Keychain/env key resolution path.
    /// Missing key is returned as a typed status, not as a thrown bridge error,
    /// so Settings can render "Add API key to discover models" inline.
    pub fn discover_models(&self, provider_id: String) -> CsModelDiscovery {
        let provider = match resolve_catalog_provider(&UserSettings::load(), &provider_id) {
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
        let api_key = keychain::cached_runtime_key(&provider.key_account);
        match discover_provider_models(&provider, api_key.as_deref()) {
            Ok(result) => {
                let (status, message) = match result.status {
                    ModelDiscoveryStatus::Fresh => ("fresh".to_string(), None),
                    ModelDiscoveryStatus::Cached { reason } => ("cached".to_string(), Some(reason)),
                };
                CsModelDiscovery {
                    provider_id: result.provider.as_string(),
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
                let status = if provider.key_required && error.code() == "no_key" {
                    "no_key"
                } else if error.rejects_credential() {
                    "key_rejected"
                } else {
                    "error"
                };
                CsModelDiscovery {
                    provider_id: error.provider().as_string(),
                    status: status.to_string(),
                    message: Some(error.message()),
                    models: Vec::new(),
                }
            }
        }
    }

    /// Availability of the same file lane used by explicit cloud retranscription.
    pub fn cloud_file_retranscription_available(&self) -> bool {
        crate::recording::cloud_file_lane(&Config::load_without_keychain()).is_ok()
    }

    pub fn stt_lanes(&self) -> Vec<CsSttLane> {
        let settings = UserSettings::load_projection();
        SttLane::ALL
            .into_iter()
            .map(|lane| CsSttLane {
                id: lane.id().into(),
                title: lane.title().into(),
                accepts: lane.accepts().into(),
                placeholder: lane.placeholder().into(),
                endpoint: match lane {
                    SttLane::File => settings.stt_file_endpoint.clone(),
                    SttLane::Live => settings.stt_live_endpoint.clone(),
                },
                endpoint_wire_key: lane.wire_key().into(),
                key_account: lane.key_account().into(),
                api_key_set: keychain::key_present(lane.key_account()),
            })
            .collect()
    }

    pub fn service_key_accounts(&self) -> Vec<String> {
        vec!["GITHUB_TOKEN".into()]
    }

    /// Store an API key in the Keychain. `account` must be a known
    /// `KEYCHAIN_ACCOUNTS` entry. The secret is never echoed back.
    pub fn set_api_key(&self, account: String, secret: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        UserSettings::with_credential_edit(&account, |_| save_key(&account, &secret)).map_err(
            |error| CsError::Config {
                msg: error.to_string(),
            },
        )?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Delete an API key from the Keychain. `account` must be a known
    /// `KEYCHAIN_ACCOUNTS` entry.
    pub fn clear_api_key(&self, account: String) -> Result<(), CsError> {
        ensure_known_account(&account)?;
        UserSettings::with_credential_edit(&account, |_| delete_key(&account)).map_err(
            |error| CsError::Config {
                msg: error.to_string(),
            },
        )?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
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
        .map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
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
        .map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Overwrite the BASE assistive prompt file (`assistive.txt`).
    pub fn set_assistive_prompt(&self, content: String) -> Result<(), CsError> {
        write_prompt(
            PromptKind::Assistive,
            &content,
            PromptWriteReason::SettingsSave,
        )
        .map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Restore only the formatting base prompt after explicit UI confirmation:
    /// the override is backed up and removed, so the built-in text is in use.
    pub fn restore_formatting_prompt_to_default(&self) -> Result<(), CsError> {
        restore_prompt_to_default(PromptKind::Formatting).map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Restore one explicit formatting policy prompt after UI confirmation:
    /// the override is backed up and removed, so the built-in text is in use.
    pub fn restore_formatting_prompt_for_level_to_default(
        &self,
        level: String,
    ) -> Result<(), CsError> {
        restore_prompt_to_default(formatting_prompt_kind(&level)?).map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Restore only the assistive base prompt after explicit UI confirmation:
    /// the override is backed up and removed, so the built-in text is in use.
    pub fn restore_assistive_prompt_to_default(&self) -> Result<(), CsError> {
        restore_prompt_to_default(PromptKind::Assistive).map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
    }

    /// Reset both BASE prompt files to their built-in defaults
    /// (`reset_to_defaults`). Does not touch `*_tuning.txt`.
    pub fn reset_prompts_to_defaults(&self) -> Result<(), CsError> {
        reset_to_defaults().map_err(CsError::from)?;
        invalidate_runtime_snapshot_cache();
        crate::hotkeys::refresh_live_controller_config();
        Ok(())
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
    /// canonical 13-step flow). Returns `0` (Welcome) when no progress marker
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
        UserSettings::load_projection().onboarding_mode
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

    /// Preview the Agent-only reset without changing disk or Keychain state.
    pub fn reset_agent_preview(&self) -> CsAgentResetPreview {
        agent_reset_preview_for_paths(&agent_reset_paths())
    }

    /// Reset only durable Agent state. Conversations and tool/MCP files are
    /// moved to Trash; provider credentials are deleted from Keychain and are
    /// intentionally not recoverable. This deliberately does not use the
    /// full-app reset fence or its broad root move.
    pub fn reset_agent_data(&self) -> Result<(), CsError> {
        // Resolve connector-owned Keychain accounts before `mcp.json` is moved.
        // A malformed config fails closed while no destructive mutation has
        // started; otherwise its bearer tokens would become invisible orphans.
        let mut secret_accounts: Vec<String> = agent_secret_accounts()
            .iter()
            .map(|account| (*account).to_string())
            .collect();
        secret_accounts.extend(
            agent_connector_secret_accounts()
                .map_err(|error| agent_reset_error(false, error.to_string()))?,
        );
        let trash = codescribe_trash_dir()?;
        let destination = create_agent_reset_destination(&trash)?;
        let paths = agent_reset_paths();
        let mut mutation_started = false;

        for source in &paths {
            if source.exists() {
                let name = source.file_name().ok_or_else(|| {
                    agent_reset_error(mutation_started, "Agent reset path has no filename")
                })?;
                let target = unique_destination(&destination, &name.to_string_lossy(), None);
                move_path_recoverably(source, &target).map_err(|error| {
                    agent_reset_error(
                        mutation_started,
                        format!("failed to move Agent data to Trash: {error}"),
                    )
                })?;
                mutation_started = true;
            }
        }

        clear_agent_settings().map_err(|error| {
            agent_reset_error(
                mutation_started,
                format!("failed to clear Agent settings: {error}"),
            )
        })?;
        mutation_started = true;
        Config::remove_env_keys(agent_env_keys()).map_err(|error| {
            agent_reset_error(
                mutation_started,
                format!("failed to clear Agent .env settings: {error}"),
            )
        })?;

        for account in &secret_accounts {
            mutation_started = true;
            UserSettings::with_credential_edit(account, |_| delete_key(account)).map_err(
                |error| {
                    agent_reset_error(
                        mutation_started,
                        format!("failed to remove Agent secret {account}: {error}"),
                    )
                },
            )?;
        }
        Ok(())
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
        let mut reset_guard = begin_app_data_reset().map_err(|error| CsError::Config {
            msg: format!("cannot reset app data: {error}"),
        })?;
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
            error: None,
            failed_source: None,
            failed_destination: None,
        })
        .map_err(|error| CsError::Config {
            msg: format!("failed to append reset audit log: {error}"),
        })?;

        let moved_paths = match move_reset_dirs_to_destination(
            &mut reset_guard,
            &dirs,
            &reset_destination,
            &trash_root,
        ) {
            Ok(moved_paths) => moved_paths,
            Err(failure) => {
                let error = failure.to_string();
                let _ = append_reset_audit(&ResetAuditEvent {
                    audit_path: &audit_path,
                    timestamp: &now,
                    status: "move_failed",
                    source_paths: &dirs,
                    moved_paths: &failure.moved_paths,
                    trash_path: &reset_destination,
                    preview: &preview,
                    include_keys,
                    include_prompts,
                    preserved_prompt_files: preserved_prompts.len(),
                    error: Some(&error),
                    failed_source: Some(&failure.failed_source),
                    failed_destination: Some(&failure.failed_destination),
                });
                return Err(reset_error(
                    &reset_guard,
                    format!("failed to move app data to Trash: {error}"),
                ));
            }
        };

        if let Err(error) = restore_base_prompts(&reset_guard, &preserved_prompts) {
            let error_message = error.to_string();
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
                error: Some(&error_message),
                failed_source: None,
                failed_destination: None,
            });
            return Err(reset_error(
                &reset_guard,
                format!("app data moved to Trash but base prompt restoration failed: {error}"),
            ));
        }

        let key_error = if include_keys {
            let mut failure = None;
            for account in KEYCHAIN_ACCOUNTS {
                if let Err(error) = delete_key(account) {
                    failure = Some(reset_error(
                        &reset_guard,
                        format!("failed to remove keychain key {account}: {error}"),
                    ));
                    break;
                }
            }
            failure
        } else {
            None
        };
        let key_error_message = key_error.as_ref().map(ToString::to_string);

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
            error: key_error_message.as_deref(),
            failed_source: None,
            failed_destination: None,
        })
        .map_err(|error| {
            reset_error(
                &reset_guard,
                format!("failed to append reset audit log: {error}"),
            )
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

/// One user-owned prompt file held in memory across a reset, so a reset that
/// keeps prompts can write the exact original bytes back after the data root
/// has already been moved to Trash.
#[derive(Debug)]
struct PreservedPrompt {
    kind: PromptKind,
    bytes: Vec<u8>,
}

/// Read every user-owned prompt file before the reset moves its directory away.
/// A prompt that does not exist yet is simply absent from the result — only a
/// real read failure aborts, because losing bytes we could not read would make
/// the later restore silently lossy.
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

/// Write captured prompt bytes back through the core's atomic prompt writer,
/// recreating the prompts directory the reset just removed. Tagged with
/// `AppResetPreservation` so the prompt audit distinguishes a restore from a
/// user edit.
fn restore_base_prompts(
    reset: &AppDataResetGuard,
    prompts: &[PreservedPrompt],
) -> std::io::Result<()> {
    for prompt in prompts {
        write_prompt_bytes_during_reset(
            reset,
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

/// The complete durable Agent-owned file surface. Keep this explicit: a reset
/// is allowed to move only these paths, never a parent data root.
fn agent_reset_paths() -> Vec<PathBuf> {
    let data = UserSettings::settings_dir();
    let config = Config::config_dir();
    vec![
        // `ThreadStore` keeps attachments at `threads/blobs/`; moving the
        // parent once avoids a second overlapping move and preserves recovery.
        data.join("threads"),
        config.join("mcp.json"),
        config.join("tool_grants.json"),
    ]
}

fn agent_secret_accounts() -> &'static [&'static str] {
    &[
        "LLM_LIBRAXIS_API_KEY",
        "LLM_OPENAI_API_KEY",
        "LLM_ANTHROPIC_API_KEY",
        "LLM_XAI_API_KEY",
        "LLM_GOOGLE_API_KEY",
        account_auth::OPENAI_ACCOUNT_TOKENS_ACCOUNT,
        account_auth::ANTHROPIC_ACCOUNT_TOKENS_ACCOUNT,
        account_auth::XAI_ACCOUNT_TOKENS_ACCOUNT,
        account_auth::GOOGLE_ACCOUNT_TOKENS_ACCOUNT,
    ]
}

/// Connector tokens created by the MCP Settings UI use a private account
/// namespace and are referenced from `mcp.json`. Delete only those app-owned
/// accounts: a hand-written auth_ref may deliberately point at a shared secret
/// such as GITHUB_TOKEN and Reset Agent must not infer ownership of it.
fn agent_connector_secret_accounts() -> anyhow::Result<Vec<String>> {
    let mut accounts: Vec<String> =
        codescribe_core::mcp::list_servers_at(&Config::config_dir().join("mcp.json"))?
            .into_iter()
            .filter_map(|server| server.auth_ref)
            .filter(|account| account.starts_with("MCP_CONNECTOR_") && account.ends_with("_TOKEN"))
            .collect();
    accounts.sort();
    accounts.dedup();
    Ok(accounts)
}

/// Legacy/power-user Agent rows that can otherwise outlive settings.json and
/// become the effective provider again after relaunch. Keep this list narrow:
/// dictation, formatting, audio and hotkey rows are intentionally absent.
fn agent_env_keys() -> &'static [&'static str] {
    &[
        "LLM_ASSISTIVE_MODEL",
        "LLM_ASSISTIVE_PROVIDER",
        "LLM_LIBRAXIS_API_KEY",
        "LLM_OPENAI_API_KEY",
        "LLM_ANTHROPIC_API_KEY",
        "LLM_XAI_API_KEY",
        "LLM_GOOGLE_API_KEY",
        "LLM_OPENAI_ACCOUNT_TOKENS",
        "LLM_ANTHROPIC_ACCOUNT_TOKENS",
        "LLM_XAI_ACCOUNT_TOKENS",
        "LLM_GOOGLE_ACCOUNT_TOKENS",
        "LLM_OPENAI_OAUTH_CLIENT_ID",
        "LLM_ANTHROPIC_OAUTH_CLIENT_ID",
        "LLM_XAI_OAUTH_CLIENT_ID",
        "LLM_GOOGLE_OAUTH_CLIENT_ID",
        "AGENT_WORKSPACE_ROOTS",
        "AGENT_ENTER_SENDS",
        "AGENT_AUTO_SEND",
        "CODESCRIBE_AGENT_SEND_HOLD_SEC",
        "AGENT_AUTO_SEND_DELAY_SEC",
        "AGENT_TAKE_VOICELESS_END_SEC",
    ]
}

fn agent_reset_preview_for_paths(paths: &[PathBuf]) -> CsAgentResetPreview {
    let (connector_accounts, connector_scan_failed) = match agent_connector_secret_accounts() {
        Ok(accounts) => (accounts, false),
        Err(_) => (Vec::new(), true),
    };
    let mut preview = CsAgentResetPreview {
        secrets_present: connector_scan_failed
            || agent_secret_accounts()
                .iter()
                .map(|account| (*account).to_string())
                .chain(connector_accounts)
                .any(|account| {
                    codescribe_core::config::keychain::load_key(&account)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                }),
        ..CsAgentResetPreview::default()
    };

    for path in paths {
        if path.ends_with("threads") {
            preview.threads = preview
                .threads
                .saturating_add(thread_index_count(&path.join("index.json")));
        }
        preview.files = preview.files.saturating_add(path_file_count(path));
        preview.total_bytes = preview.total_bytes.saturating_add(path_size(path));
    }
    preview
}

fn path_file_count(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return 1;
    }
    if !metadata.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| path_file_count(&entry.path()))
        .sum()
}

/// Remove only settings that define an Agent identity, its provider, its
/// workspace and its tool policy. All transcription, audio, hotkey, lexicon,
/// license and other ordinary app settings remain in the same JSON document.
fn clear_agent_settings() -> anyhow::Result<()> {
    UserSettings::remove_agent_owned_state()
}

fn create_agent_reset_destination(trash: &Path) -> anyhow::Result<PathBuf> {
    codescribe_core::test_isolation::assert_test_write_allowed(trash);
    fs::create_dir_all(trash)?;
    let destination = unique_destination(
        trash,
        &format!("codescribe-agent-reset-{}", timestamp_slug(&Utc::now())),
        None,
    );
    fs::create_dir_all(&destination)?;
    Ok(destination)
}

/// The user's `~/.Trash`, which is where a reset parks data so it stays
/// recoverable. Fails loudly when the home directory cannot be resolved rather
/// than inventing a fallback destination.
fn codescribe_trash_dir() -> Result<PathBuf, CsError> {
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".Trash"))
        .ok_or_else(|| CsError::Config {
            msg: "could not resolve the user's Trash directory".to_string(),
        })
}

/// External audit log for resets, deliberately outside the app data roots
/// (`~/Library/Logs/Codescribe/`) so the record of a reset survives the reset
/// that produced it. `validate_reset_source` refuses any source that would
/// swallow this path.
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

/// Sum the live reset impact across the data roots. Byte and file counts add up
/// across roots; `threads` does not — the thread index is a single store, so the
/// first root that carries one wins instead of double-counting a shared index.
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

/// Recursive on-disk size of a path. Symlinks count as zero and are never
/// followed, so a link into a huge external tree cannot inflate the preview (or
/// walk out of the reset scope). Unreadable entries contribute zero rather than
/// failing the whole preview.
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

/// Count `(audio_files, transcript_days)` under the transcriptions root. A "day"
/// is a direct child directory that contains at least one file at any depth, so
/// an empty leftover day folder is not reported as user data.
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

/// Recursively count `(audio_files, all_files)` inside one transcription day.
/// Symlinks are skipped entirely so the walk cannot escape the day folder or
/// count the same bytes twice.
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

/// Extension-based recording test for the reset preview. Case-insensitive, and
/// deliberately extension-only: the preview must stay cheap, so no file is
/// opened to sniff its container.
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

/// Number of conversations recorded in the thread index. A missing, unreadable,
/// or malformed index counts as zero: the preview is advisory and must never
/// block a reset on a corrupt artifact.
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

/// Filename-safe UTC timestamp for Trash destinations: RFC3339 shape but with
/// dashes instead of colons in the time part. Milliseconds are kept so two
/// resets inside the same second still produce distinct names.
fn timestamp_slug(now: &DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H-%M-%S%.3fZ").to_string()
}

/// First non-colliding `root/stem[-N][.ext]`. A reset must never overwrite an
/// earlier recoverable copy sitting in Trash, so collisions get a numeric
/// suffix; after 10 000 attempts it falls back to the process id rather than
/// looping or returning a path that already exists.
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

/// Last-line guard before a directory is moved out from under the user. Refuses
/// a filesystem root, the home directory itself, any source that contains the
/// Trash destination (which would move the destination into itself), and any
/// source that contains the external audit log (which would erase the record of
/// the reset). Operates on canonicalized paths, so a symlinked data dir cannot
/// disguise an unsafe target.
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

/// Create the timestamped `codescribe-reset-<ts>` folder in Trash that will
/// receive this reset's data roots. Uses `create_dir` (not `create_dir_all`) for
/// the leaf so an already-existing destination is an error rather than a silent
/// merge into someone else's reset.
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

/// Structured partial-progress failure from a multi-root reset. The external
/// audit must retain every root that already moved plus the exact root whose
/// move failed; reporting `moved_paths=[]` after a partial move is false
/// recovery guidance.
#[derive(Debug)]
struct ResetMoveFailure {
    error: std::io::Error,
    moved_paths: Vec<(PathBuf, PathBuf)>,
    failed_source: PathBuf,
    failed_destination: PathBuf,
}

impl std::fmt::Display for ResetMoveFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} -> {} failed after {} completed move(s): {}",
            self.failed_source.display(),
            self.failed_destination.display(),
            self.moved_paths.len(),
            self.error
        )
    }
}

/// Stable destination label for one selected data root.
fn reset_root_destination(reset_destination: &Path, index: usize) -> PathBuf {
    let label = if index == 0 {
        "codescribe-data".to_string()
    } else {
        format!("application-support-{index}")
    };
    reset_destination.join(label)
}

/// Move each existing data root into the prepared Trash destination, validating
/// every source first. The first root lands as `codescribe-data` and the rest as
/// `application-support-<n>`, so the recovered folder is self-describing.
/// Returns the `(source, destination)` pairs for the audit record.
fn move_reset_dirs_to_destination(
    reset: &mut AppDataResetGuard,
    dirs: &[PathBuf],
    reset_destination: &Path,
    trash_root: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, ResetMoveFailure> {
    let existing_sources: Vec<&PathBuf> = dirs.iter().filter(|source| source.exists()).collect();
    // Validate the complete cut before the first irreversible move. Otherwise a
    // bad second root could be discovered only after the first was already in
    // Trash, turning a preflight error into a partial reset.
    for (index, source) in existing_sources.iter().enumerate() {
        if let Err(error) = validate_reset_source(source, trash_root) {
            return Err(ResetMoveFailure {
                error,
                moved_paths: Vec::new(),
                failed_source: (*source).clone(),
                failed_destination: reset_root_destination(reset_destination, index),
            });
        }
    }

    let mut moved_paths = Vec::new();
    for (index, source) in existing_sources.into_iter().enumerate() {
        let destination = reset_root_destination(reset_destination, index);
        if let Err(error) = move_reset_path_recoverably(reset, source, &destination) {
            return Err(ResetMoveFailure {
                error,
                moved_paths,
                failed_source: source.clone(),
                failed_destination: destination,
            });
        }
        moved_paths.push((source.clone(), destination));
    }
    Ok(moved_paths)
}

/// Move one reset root while coupling the first irreversible filesystem change
/// to the process-lifetime latch. A failed same-volume rename is still wholly
/// reversible, so the gate remains in Resetting during the copy fallback and
/// only latches immediately before the copied source is removed.
fn move_reset_path_recoverably(
    reset: &mut AppDataResetGuard,
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    match reset.rename_destructively(source, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            if let Err(copy_error) = copy_path_without_following_symlinks(source, destination) {
                return Err(std::io::Error::new(
                    copy_error.kind(),
                    format!("rename failed ({rename_error}); fallback copy failed ({copy_error})"),
                ));
            }
            reset.mark_destructive_started();
            remove_path_without_following_symlinks(source)
        }
    }
}

/// Prefer same-volume rename. If the source is on another volume, create and
/// sync the complete destination before removing the source tree.
fn move_path_recoverably(source: &Path, destination: &Path) -> std::io::Result<()> {
    move_path_recoverably_with(source, destination, |source, destination| {
        fs::rename(source, destination)
    })
}

/// `move_path_recoverably` with an injectable rename, so tests can force the
/// cross-volume branch without a second volume. The ordering is the safety
/// property: the copy must fully succeed before the source is removed, and a
/// failed copy reports both errors instead of leaving a half-move behind.
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

/// Recursively copy a tree, recreating symlinks as links instead of copying
/// through them — following a link would both duplicate foreign data into Trash
/// and break the user's ability to restore the tree as it was. Regular files are
/// `sync_all`'d before the caller removes the source, so a crash mid-reset
/// cannot leave the data neither copied nor kept.
fn copy_path_without_following_symlinks(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        // WHY: `destination` is an app-created child of ~/.Trash and `target`
        // is whatever the user's own link already pointed at; this call
        // recreates the link itself without following or writing through its
        // target. Preserving the link is required for a recoverable reset;
        // regular files go through `fs::copy`.
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

/// Delete a tree without ever descending through a symlink — a link is unlinked,
/// never traversed, so removing the source after a cross-volume copy cannot
/// reach outside the reset scope. An already-absent path is success, which keeps
/// the copy-then-remove path idempotent on retry.
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

/// Move a single `mcp.json` to Trash, leaving every sibling in the config
/// directory untouched. `Ok(None)` means there was nothing to clear — an absent
/// file is not an error, so the Settings action is idempotent.
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

/// One line of the reset audit record. Borrowed throughout because a reset
/// writes several of these (`started`, then a terminal status) from the same
/// live values, and the record must describe scope and counts only — never file
/// contents, prompts, or key material.
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
    error: Option<&'a str>,
    failed_source: Option<&'a Path>,
    failed_destination: Option<&'a Path>,
}

/// Append one JSON line to the external reset audit log and fsync it. Strictly
/// append-only and flushed per entry: the `started` line has to survive even if
/// the reset that follows crashes, which is the only way to tell an interrupted
/// reset from one that never began.
fn append_reset_audit(event: &ResetAuditEvent<'_>) -> std::io::Result<()> {
    codescribe_core::test_isolation::assert_test_write_allowed(event.audit_path);
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
        "error": event.error,
        "failed_source": event.failed_source.map(|path| path.to_string_lossy()),
        "failed_destination": event.failed_destination.map(|path| path.to_string_lossy()),
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(event.audit_path)?;
    writeln!(file, "{entry}")?;
    file.sync_data()
}

/// Trim and collapse a blank string to `None`. The tier lookups below treat
/// "present but empty" as "not configured", so a stray `KEY=` never shadows a
/// real value from a lower tier.
fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// A persisted settings value, normalized to `None` when blank.
fn setting_string(value: Option<String>) -> Option<String> {
    value.and_then(non_empty)
}

fn provider_error(error: impl std::fmt::Display) -> CsError {
    CsError::Config {
        msg: error.to_string(),
    }
}

fn parse_wire(wire: &str) -> Result<WireFamily, CsError> {
    WireFamily::parse(wire).ok_or_else(|| provider_error("unknown provider wire"))
}

fn resolve_catalog_provider(
    settings: &UserSettings,
    id: &str,
) -> Result<ResolvedProvider, CsError> {
    ProviderRef::parse(id)
        .and_then(|reference| ProviderRegistry::from_settings(settings).resolve(&reference))
        .ok_or_else(|| provider_error(format!("unknown provider: {id}")))
}

fn provider_option(provider: ResolvedProvider) -> CsProviderOption {
    let google = provider.wire == WireFamily::GoogleGenerateContent;
    let google_status = google.then(account_auth::google_credential_status);
    let google_account_available = google_status
        .as_ref()
        .is_some_and(|status| status.as_ref().is_ok_and(|status| status.available));
    let google_message = google_status.as_ref().map(|status| match status {
        Ok(status) => status.message.clone(),
        Err(_) => "Google account credentials are unavailable or unsupported. Use an API key or authorized-user ADC; service-account and external-account credentials are not supported.".into(),
    }).map(|message| {
        if codescribe_core::llm::vendors::google::is_cloud_code_assist(&provider.endpoint) {
            format!("Cloud Code Assist uses account credentials; API keys cannot authenticate this endpoint. {message}")
        } else {
            message
        }
    });
    let status = provider
        .oauth_vendor
        .map(account_auth::cached_account_status);
    CsProviderOption {
        id: provider.reference.as_string(),
        kind: if provider.reference.custom_id().is_some() {
            "custom"
        } else {
            "vendor"
        }
        .to_string(),
        display_name: provider.display_name,
        wire: provider.wire.as_str().to_string(),
        endpoint: provider.endpoint,
        api_key_set: keychain::key_present(&provider.key_account)
            || (google && account_auth::google_api_key().is_some()),
        api_key_account: provider.key_account,
        key_required: provider.key_required && !google_account_available,
        account_signed_in: if google {
            // Saved storage is not a token-acceptance receipt. ADC remains
            // externally managed and is not shown as a removable account.
            google_status
                .as_ref()
                .and_then(|status| status.as_ref().ok())
                .map_or_else(
                    || keychain::key_present(account_auth::GOOGLE_ACCOUNT_TOKENS_ACCOUNT),
                    |status| status.stored_account,
                )
        } else {
            status.as_ref().is_some_and(|status| status.signed_in)
        },
        account_login_enabled: status
            .as_ref()
            .is_some_and(|status| status.client_id_configured),
        account_identity: status.as_ref().and_then(|status| status.identity.clone()),
        account_status_message: google_message
            .unwrap_or_else(|| status.map(|status| status.message).unwrap_or_default()),
        oauth_client_id: provider
            .oauth_vendor
            .and_then(|vendor| account_auth::client_id_for_provider(vendor).ok()),
    }
}

fn persist_custom_provider(
    settings: &UserSettings,
    row: &CustomProvider,
    secret: Option<String>,
) -> Result<CsProviderOption, CsError> {
    // Persist the addressable row first; failed key writes can be retried through set_api_key.
    settings.save().map_err(provider_error)?;
    keychain::advance_bundle_revision();
    let result = secret
        .map(|secret| {
            UserSettings::with_credential_edit(&row.key_account(), |_| {
                save_key(&row.key_account(), &secret)
            })
        })
        .transpose();
    crate::hotkeys::refresh_live_controller_config();
    result.map_err(provider_error)?;
    Ok(provider_option(resolve_catalog_provider(
        settings,
        &ProviderRef::Custom(row.id.clone()).as_string(),
    )?))
}

fn validate_provider_setting(key: &str, value: &str) -> Result<(), CsError> {
    if let Some(lane) = SttLane::ALL.into_iter().find(|lane| lane.wire_key() == key)
        && !value.trim().is_empty()
    {
        validate_stt_endpoint(lane, value).map_err(provider_error)?;
    }
    if key.starts_with("LLM_") && key.ends_with("_ENDPOINT") {
        return Err(provider_error("removed: endpoints live on providers"));
    }
    if key == "LLM_MODEL" {
        return Err(provider_error("removed: models live on lanes"));
    }
    if matches!(key, "LLM_FORMATTING_PROVIDER" | "LLM_ASSISTIVE_PROVIDER") {
        resolve_catalog_provider(&UserSettings::load_projection(), value)?;
    }
    Ok(())
}

/// One in-flight account login — either a loopback callback server (OpenAI)
/// or an RFC 8628 device-code poll handle (xAI SuperGrok / OpenCode path).
enum PendingAccountLogin {
    Loopback(account_auth::LoginServer),
    Device {
        config: account_auth::DeviceAuthConfig,
        device: account_auth::DeviceCode,
    },
}

impl PendingAccountLogin {
    /// Abandon this attempt. Loopback shuts the listener; device just drops
    /// (the awaiter times out or is superseded by a new start).
    fn cancel(self) {
        match self {
            Self::Loopback(login) => login.cancel(),
            Self::Device { .. } => {}
        }
    }
}

/// The single in-flight login, if any. At most one sign-in can be pending:
/// starting a second supersedes and cancels the first.
fn active_account_login() -> &'static Mutex<Option<PendingAccountLogin>> {
    /// At most one in-flight login; a new start cancels the previous.
    static ACTIVE: OnceLock<Mutex<Option<PendingAccountLogin>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Cancel the content-free OAuth/device-code lifecycle before application
/// runtime shutdown. The shared app runtime then owns the only reactor teardown.
pub(crate) fn cancel_pending_account_login_for_shutdown() {
    let pending = active_account_login()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(pending) = pending {
        pending.cancel();
    }
}

/// Flatten an account-auth error into the bridge's config error. The core error
/// type stays out of the FFI surface; only its rendered message crosses, and
/// that message is already written to be safe to show a user.
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
    /// One-to-one re-homing of the core verdict. The arms are written out rather
    /// than derived so adding a core status is a compile error here instead of a
    /// silently mismapped UI bucket.
    fn from(status: ApiKeyLivenessStatus) -> Self {
        match status {
            ApiKeyLivenessStatus::Ok => Self::Ok,
            ApiKeyLivenessStatus::Invalid => Self::Invalid,
            ApiKeyLivenessStatus::NoQuota => Self::NoQuota,
            ApiKeyLivenessStatus::Network => Self::Network,
            ApiKeyLivenessStatus::Missing => Self::Missing,
            ApiKeyLivenessStatus::Unsupported => Self::Unsupported,
            ApiKeyLivenessStatus::Rejected => Self::Rejected,
        }
    }
}

impl From<ApiKeyLivenessResult> for CsApiKeyProbeResult {
    /// Carry the whole probe result across, including `probed_endpoint` — the
    /// Keys panel shows which endpoint answered, so dropping it here would make
    /// a self-hosted misconfiguration indistinguishable from a bad key.
    fn from(result: ApiKeyLivenessResult) -> Self {
        Self {
            account: result.account,
            status: result.status.into(),
            message: result.message,
            probed_endpoint: result.probed_endpoint,
        }
    }
}

/// Reject unknown Keychain accounts before touching the Keychain.
fn ensure_known_account(account: &str) -> Result<(), CsError> {
    if keychain::is_known_account(account) {
        Ok(())
    } else {
        Err(CsError::Config {
            msg: format!("unknown keychain account: {account}"),
        })
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
