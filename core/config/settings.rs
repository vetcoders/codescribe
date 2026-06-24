//! User-facing settings stored as JSON (GUI-managed).
//!
//! These are the "regular user" tier. Power users override via ~/.codescribe/.env.

use super::types::{ModeBinding, ShortcutBinding, WorkMode, default_mode_bindings};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Regular-user settings (JSON, GUI-managed).
/// All fields are Option — None means "use default or .env override".
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct UserSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_exclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_bindings: Option<Vec<ModeBinding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_start_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_tap_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_silence_sec: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_formatting_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_tagging_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_tag_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beep_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formatting_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_assistive_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_assistive_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_zoom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcription_overlay_enabled: Option<bool>,

    // ── Promoted from .env (settings.json is now source of truth) ──
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_formatting_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_formatting_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_local_stt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_send_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_input_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_notes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_notes_save_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qube_daemon_autostart: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_enter_sends: Option<bool>,

    // ── Voice Lab survivors (user-facing UX knobs) ──
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing_cps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emit_words_max: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffered_interim_sec: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_max_upload_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SettingsV2 {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    interaction: Option<InteractionV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speech: Option<SpeechV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<AudioV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ui: Option<UiV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<FeaturesV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<SystemV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct InteractionV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<TriggerV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hold: Option<HoldV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode_bindings: Option<Vec<ModeBinding>>,
    // De-ghosted (2026-05-30): these user-facing knobs existed in UserSettings + were
    // promoted, but the V2 schema dropped them on every round-trip — settings.json could
    // not actually express them. settings.json must support ALL non-secret parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    send_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_enter_sends: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct TriggerV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    double_tap_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    toggle_silence_timeout_sec: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct HoldV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    exclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SpeechV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<SpeechEngineV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    formatting: Option<FormattingV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assistive: Option<AssistiveV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emission: Option<EmissionV2>,
    // De-ghosted (2026-05-30): base/default LLM endpoint + model (distinct from the
    // formatting/assistive overrides). Previously dropped on V2 round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SpeechEngineV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_transcription_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_max_upload_mb: Option<u64>,
    // De-ghosted (2026-05-30): Whisper model id (distinct from local_model_id path).
    #[serde(skip_serializing_if = "Option::is_none")]
    whisper_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct FormattingV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_tagging_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_tag_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct AssistiveV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct EmissionV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    buffer_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    typing_cps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emit_words_max: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interim_cadence_sec: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct AudioV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    input_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<FeedbackV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct FeedbackV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    beep_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sound_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct UiV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_zoom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcription_overlay_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct FeaturesV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quick_notes_enabled: Option<bool>,
    // De-ghosted (2026-05-30): previously dropped on V2 round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    quick_notes_save_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SystemV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    start_at_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qube_daemon_autostart: Option<bool>,
}

/// Canonical list of env keys that route to `settings.json` (not `.env`).
///
/// Used by `Config::save_to_env`, `Config::save_to_env_many`, and IPC
/// `persist_config` to decide whether a key is "promoted" (GUI-managed)
/// or power-user (.env-managed).
///
/// **Single source of truth** — add new promoted keys here only.
pub const PROMOTED_SETTINGS_KEYS: &[&str] = &[
    // Hotkeys
    "WHISPER_LANGUAGE",
    "HOLD_START_DELAY_MS",
    "DOUBLE_TAP_INTERVAL_MS",
    "TOGGLE_SILENCE_SEC",
    "HOLD_EXCLUSIVE",
    // AI / Formatting
    "AI_FORMATTING_ENABLED",
    "TRANSCRIPT_TAGGING_ENABLED",
    "TRANSCRIPT_TAG_TEMPLATE",
    "FORMATTING_LEVEL",
    // Sound
    "BEEP_ON_START",
    "SOUND_VOLUME",
    "SOUND_NAME",
    // App visibility
    "SHOW_DOCK_ICON",
    "TRANSCRIPTION_OVERLAY_ENABLED",
    // LLM endpoints
    "LLM_ENDPOINT",
    "LLM_MODEL",
    "LLM_ASSISTIVE_ENDPOINT",
    "LLM_ASSISTIVE_MODEL",
    "LLM_FORMATTING_ENDPOINT",
    "LLM_FORMATTING_MODEL",
    // Promoted from .env
    "USE_LOCAL_STT",
    "LOCAL_MODEL",
    "STT_ENDPOINT",
    "TRANSCRIPT_SEND_MODE",
    "AUDIO_INPUT_DEVICE",
    "HISTORY_ENABLED",
    "QUICK_NOTES_ENABLED",
    "QUICK_NOTES_SAVE_ONLY",
    "START_AT_LOGIN",
    "QUBE_DAEMON_AUTOSTART",
    "AGENT_ENTER_SENDS",
    // Voice Lab survivors
    "CODESCRIBE_BUFFER_DELAY_MS",
    "CODESCRIBE_TYPING_CPS",
    "CODESCRIBE_EMIT_WORDS_MAX",
    "CODESCRIBE_BUFFERED_INTERIM_SEC",
    "WHISPER_MODEL",
    "BACKEND_MAX_UPLOAD_MB",
];

/// Check if a key is a promoted (settings.json) setting.
pub fn is_promoted_key(key: &str) -> bool {
    PROMOTED_SETTINGS_KEYS.contains(&key)
}

impl UserSettings {
    fn to_v2(&self) -> SettingsV2 {
        let normalized_mode_bindings = self.mode_bindings_normalized();
        SettingsV2 {
            schema_version: 3,
            interaction: Some(InteractionV2 {
                trigger: Some(TriggerV2 {
                    double_tap_interval_ms: self.double_tap_interval_ms,
                    toggle_silence_timeout_sec: self.toggle_silence_sec,
                }),
                hold: Some(HoldV2 {
                    exclusive: self.hold_exclusive,
                    start_delay_ms: self.hold_start_delay_ms,
                }),
                mode_bindings: Some(normalized_mode_bindings),
                send_mode: self.transcript_send_mode.clone(),
                agent_enter_sends: self.agent_enter_sends,
            }),
            speech: Some(SpeechV2 {
                language: self.whisper_language.clone(),
                engine: Some(SpeechEngineV2 {
                    mode: self
                        .use_local_stt
                        .map(|v| if v { "local_whisper" } else { "cloud_whisper" }.to_string()),
                    local_model_id: self.local_model.clone(),
                    cloud_transcription_endpoint: self.stt_endpoint.clone(),
                    cloud_max_upload_mb: self.backend_max_upload_mb,
                    whisper_model: self.whisper_model.clone(),
                }),
                formatting: Some(FormattingV2 {
                    enabled: self.ai_formatting_enabled,
                    transcript_tagging_enabled: self.transcript_tagging_enabled,
                    transcript_tag_template: self.transcript_tag_template.clone(),
                    level: self.formatting_level.clone(),
                    llm_endpoint: self.llm_formatting_endpoint.clone(),
                    llm_model: self.llm_formatting_model.clone(),
                }),
                assistive: Some(AssistiveV2 {
                    llm_endpoint: self.llm_assistive_endpoint.clone(),
                    llm_model: self.llm_assistive_model.clone(),
                }),
                emission: Some(EmissionV2 {
                    buffer_delay_ms: self.buffer_delay_ms,
                    typing_cps: self.typing_cps,
                    emit_words_max: self.emit_words_max,
                    interim_cadence_sec: self.buffered_interim_sec,
                }),
                llm_endpoint: self.llm_endpoint.clone(),
                llm_model: self.llm_model.clone(),
            }),
            audio: Some(AudioV2 {
                input_device_id: self.audio_input_device.clone(),
                feedback: Some(FeedbackV2 {
                    beep_on_start: self.beep_on_start,
                    sound_name: self.sound_name.clone(),
                    volume: self.sound_volume,
                }),
            }),
            ui: Some(UiV2 {
                chat_zoom: self.chat_zoom,
                show_dock_icon: self.show_dock_icon,
                transcription_overlay_enabled: self.transcription_overlay_enabled,
            }),
            features: Some(FeaturesV2 {
                history_enabled: self.history_enabled,
                quick_notes_enabled: self.quick_notes_enabled,
                quick_notes_save_only: self.quick_notes_save_only,
            }),
            system: Some(SystemV2 {
                start_at_login: self.start_at_login,
                qube_daemon_autostart: self.qube_daemon_autostart,
            }),
        }
    }

    fn from_v2(v2: SettingsV2) -> Self {
        Self {
            whisper_language: v2.speech.as_ref().and_then(|s| s.language.clone()),
            hold_exclusive: v2
                .interaction
                .as_ref()
                .and_then(|i| i.hold.as_ref())
                .and_then(|h| h.exclusive),
            mode_bindings: v2
                .interaction
                .as_ref()
                .and_then(|i| i.mode_bindings.clone()),
            hold_start_delay_ms: v2
                .interaction
                .as_ref()
                .and_then(|i| i.hold.as_ref())
                .and_then(|h| h.start_delay_ms),
            double_tap_interval_ms: v2
                .interaction
                .as_ref()
                .and_then(|i| i.trigger.as_ref())
                .and_then(|t| t.double_tap_interval_ms),
            toggle_silence_sec: v2
                .interaction
                .as_ref()
                .and_then(|i| i.trigger.as_ref())
                .and_then(|t| t.toggle_silence_timeout_sec),
            ai_formatting_enabled: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.enabled),
            transcript_tagging_enabled: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.transcript_tagging_enabled),
            transcript_tag_template: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.transcript_tag_template.clone()),
            beep_on_start: v2
                .audio
                .as_ref()
                .and_then(|a| a.feedback.as_ref())
                .and_then(|f| f.beep_on_start),
            sound_volume: v2
                .audio
                .as_ref()
                .and_then(|a| a.feedback.as_ref())
                .and_then(|f| f.volume),
            formatting_level: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.level.clone()),
            llm_endpoint: v2.speech.as_ref().and_then(|s| s.llm_endpoint.clone()),
            llm_model: v2.speech.as_ref().and_then(|s| s.llm_model.clone()),
            llm_assistive_endpoint: v2
                .speech
                .as_ref()
                .and_then(|s| s.assistive.as_ref())
                .and_then(|a| a.llm_endpoint.clone()),
            llm_assistive_model: v2
                .speech
                .as_ref()
                .and_then(|s| s.assistive.as_ref())
                .and_then(|a| a.llm_model.clone()),
            chat_zoom: v2.ui.as_ref().and_then(|ui| ui.chat_zoom),
            show_dock_icon: v2.ui.as_ref().and_then(|ui| ui.show_dock_icon),
            transcription_overlay_enabled: v2
                .ui
                .as_ref()
                .and_then(|ui| ui.transcription_overlay_enabled),
            llm_formatting_endpoint: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.llm_endpoint.clone()),
            llm_formatting_model: v2
                .speech
                .as_ref()
                .and_then(|s| s.formatting.as_ref())
                .and_then(|f| f.llm_model.clone()),
            use_local_stt: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.mode.as_ref())
                .map(|mode| mode == "local_whisper"),
            local_model: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.local_model_id.clone()),
            stt_endpoint: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.cloud_transcription_endpoint.clone()),
            transcript_send_mode: v2.interaction.as_ref().and_then(|i| i.send_mode.clone()),
            audio_input_device: v2.audio.as_ref().and_then(|a| a.input_device_id.clone()),
            sound_name: v2
                .audio
                .as_ref()
                .and_then(|a| a.feedback.as_ref())
                .and_then(|f| f.sound_name.clone()),
            history_enabled: v2.features.as_ref().and_then(|f| f.history_enabled),
            quick_notes_enabled: v2.features.as_ref().and_then(|f| f.quick_notes_enabled),
            quick_notes_save_only: v2.features.as_ref().and_then(|f| f.quick_notes_save_only),
            start_at_login: v2.system.as_ref().and_then(|s| s.start_at_login),
            qube_daemon_autostart: v2.system.as_ref().and_then(|s| s.qube_daemon_autostart),
            agent_enter_sends: v2.interaction.as_ref().and_then(|i| i.agent_enter_sends),
            buffer_delay_ms: v2
                .speech
                .as_ref()
                .and_then(|s| s.emission.as_ref())
                .and_then(|e| e.buffer_delay_ms),
            typing_cps: v2
                .speech
                .as_ref()
                .and_then(|s| s.emission.as_ref())
                .and_then(|e| e.typing_cps),
            emit_words_max: v2
                .speech
                .as_ref()
                .and_then(|s| s.emission.as_ref())
                .and_then(|e| e.emit_words_max),
            buffered_interim_sec: v2
                .speech
                .as_ref()
                .and_then(|s| s.emission.as_ref())
                .and_then(|e| e.interim_cadence_sec),
            whisper_model: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.whisper_model.clone()),
            backend_max_upload_mb: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.cloud_max_upload_mb),
        }
    }

    fn validate_v2(v2: &SettingsV2) -> anyhow::Result<()> {
        if v2.schema_version != 2 && v2.schema_version != 3 {
            anyhow::bail!("settings schema_version must be 2 or 3")
        }
        if let Some(chat_zoom) = v2.ui.as_ref().and_then(|ui| ui.chat_zoom)
            && !(0.75..=2.0).contains(&chat_zoom)
        {
            anyhow::bail!("ui.chat_zoom must be within [0.75, 2.0]")
        }
        Ok(())
    }

    fn write_json_atomic(path: &PathBuf, json: &str) -> anyhow::Result<()> {
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Returns the settings directory.
    ///
    /// Respects `CODESCRIBE_DATA_DIR` for test isolation; otherwise uses
    /// `~/Library/Application Support/CodeScribe/`.
    pub fn settings_dir() -> PathBuf {
        let dir = if let Ok(test_dir) = std::env::var("CODESCRIBE_DATA_DIR") {
            PathBuf::from(test_dir)
        } else {
            BaseDirs::new()
                .map(|b| b.data_dir().join("CodeScribe"))
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    PathBuf::from(home).join("Library/Application Support/CodeScribe")
                })
        };

        if !dir.exists()
            && let Err(e) = fs::create_dir_all(&dir)
        {
            warn!("Failed to create settings dir {}: {e}", dir.display());
        }
        dir
    }

    /// Returns the path to `settings.json`.
    pub fn settings_path() -> PathBuf {
        Self::settings_dir().join("settings.json")
    }

    /// Loads settings from disk. Returns `Default` on any error.
    pub fn load() -> Self {
        let path = Self::settings_path();
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(value) => {
                    if value.get("schema_version").is_some() {
                        match serde_json::from_value::<SettingsV2>(value) {
                            Ok(v2) => {
                                if let Err(e) = Self::validate_v2(&v2) {
                                    warn!("Invalid settings V2 at {}: {e}", path.display());
                                    return Self::default();
                                }
                                debug!("Loaded settings V2 from {}", path.display());
                                Self::from_v2(v2)
                            }
                            Err(e) => {
                                warn!("Failed to parse settings V2 at {}: {e}", path.display());
                                Self::default()
                            }
                        }
                    } else {
                        match serde_json::from_str::<Self>(&contents) {
                            Ok(v1) => {
                                let backup_path = Self::settings_dir().join("settings.v1.bak.json");
                                if let Err(e) = fs::write(&backup_path, &contents) {
                                    warn!(
                                        "Failed to write V1 backup {}: {e}",
                                        backup_path.display()
                                    );
                                }
                                if let Err(e) = v1.save() {
                                    warn!("Failed hard-migrating settings V1 -> V2: {e}");
                                } else {
                                    info!(
                                        "Migrated settings V1 to V2 and wrote backup {}",
                                        backup_path.display()
                                    );
                                }
                                Self::from_v2(v1.to_v2())
                            }
                            Err(e) => {
                                debug!("Failed to parse {}: {e}, using defaults", path.display());
                                Self::default()
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to parse {}: {e}, using defaults", path.display());
                    Self::default()
                }
            },
            Err(e) => {
                debug!(
                    "No settings file at {} ({e}), using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Persists current settings to disk as pretty-printed JSON.
    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::settings_dir();
        fs::create_dir_all(&dir)?;
        let path = Self::settings_path();
        let v2 = self.to_v2();
        Self::validate_v2(&v2)?;
        let json = serde_json::to_string_pretty(&v2)?;

        if let Ok(existing) = fs::read_to_string(&path)
            && existing == json
        {
            debug!("Settings unchanged; skipping save to {}", path.display());
            return Ok(());
        }

        Self::write_json_atomic(&path, &json)?;
        info!("Saved settings to {}", path.display());
        Ok(())
    }

    fn save_if_changed(&self, before: &Self, setter: &str, key: &str) {
        if self == before {
            debug!("{setter}({key}) ignored; value unchanged");
            return;
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after {setter}({key}): {e}");
        }
    }

    fn mode_bindings_normalized(&self) -> Vec<ModeBinding> {
        let mut normalized = default_mode_bindings();

        if let Some(bindings) = self.mode_bindings.as_ref() {
            for candidate in bindings {
                if let Some(existing) = normalized
                    .iter_mut()
                    .find(|entry| entry.mode == candidate.mode)
                {
                    existing.binding = candidate.binding;
                } else {
                    normalized.push(candidate.clone());
                }
            }
        }

        normalized
    }

    pub fn mode_binding_for(&self, mode: WorkMode) -> ShortcutBinding {
        self.mode_bindings_normalized()
            .into_iter()
            .find(|binding| binding.mode == mode)
            .map(|binding| binding.binding)
            .unwrap_or(ShortcutBinding::Disabled)
    }

    pub fn set_mode_binding(&mut self, mode: WorkMode, binding: ShortcutBinding) {
        let before = self.clone();
        let mut mode_bindings = self.mode_bindings_normalized();
        if let Some(existing) = mode_bindings.iter_mut().find(|entry| entry.mode == mode) {
            existing.binding = binding;
        } else {
            mode_bindings.push(ModeBinding { mode, binding });
        }
        self.mode_bindings = Some(mode_bindings);
        self.save_if_changed(&before, "set_mode_binding", mode.as_str());
    }

    /// Normalize zoom value into persisted representation.
    ///
    /// - Clamps to [0.75, 2.0]
    /// - Rounds to 2 decimals (prevents float jitter rewrite spam)
    /// - Stores `None` for effective default zoom (1.0)
    pub fn normalized_chat_zoom(zoom: f64) -> Option<f64> {
        let clamped = zoom.clamp(0.75, 2.0);
        let rounded = (clamped * 100.0).round() / 100.0;
        if (rounded - 1.0).abs() < 0.01 {
            None
        } else {
            Some(rounded)
        }
    }

    /// Set persisted chat zoom, saving only on effective value change.
    ///
    /// Returns `true` when a real setting change was applied.
    pub fn set_chat_zoom(&mut self, zoom: f64) -> bool {
        let normalized = Self::normalized_chat_zoom(zoom);
        if self.chat_zoom == normalized {
            debug!("set_chat_zoom ignored; value unchanged");
            return false;
        }

        self.chat_zoom = normalized;
        if let Err(e) = self.save() {
            warn!("Failed to save after set_chat_zoom: {e}");
        }
        true
    }

    /// Sets a string-valued setting by its .env key name and saves.
    pub fn set_string(&mut self, key: &str, value: &str) {
        let before = self.clone();
        match key {
            "WHISPER_LANGUAGE" => self.whisper_language = Some(value.to_owned()),
            "LLM_ENDPOINT" => self.llm_endpoint = Some(value.to_owned()),
            "LLM_MODEL" => self.llm_model = Some(value.to_owned()),
            "LLM_ASSISTIVE_ENDPOINT" => self.llm_assistive_endpoint = Some(value.to_owned()),
            "LLM_ASSISTIVE_MODEL" => self.llm_assistive_model = Some(value.to_owned()),
            "FORMATTING_LEVEL" => self.formatting_level = Some(value.to_owned()),
            "TRANSCRIPT_TAG_TEMPLATE" => self.transcript_tag_template = Some(value.to_owned()),
            "LLM_FORMATTING_ENDPOINT" => self.llm_formatting_endpoint = Some(value.to_owned()),
            "LLM_FORMATTING_MODEL" => self.llm_formatting_model = Some(value.to_owned()),
            "LOCAL_MODEL" => self.local_model = Some(value.to_owned()),
            "STT_ENDPOINT" => self.stt_endpoint = Some(value.to_owned()),
            "TRANSCRIPT_SEND_MODE" => self.transcript_send_mode = Some(value.to_owned()),
            "AUDIO_INPUT_DEVICE" => self.audio_input_device = Some(value.to_owned()),
            "SOUND_NAME" => self.sound_name = Some(value.to_owned()),
            "WHISPER_MODEL" => self.whisper_model = Some(value.to_owned()),
            other => {
                warn!("Unknown string setting key: {other}");
                return;
            }
        }
        self.save_if_changed(&before, "set_string", key);
    }

    /// Sets a boolean-valued setting by its .env key name and saves.
    pub fn set_bool(&mut self, key: &str, value: bool) {
        let before = self.clone();
        match key {
            "AI_FORMATTING_ENABLED" => self.ai_formatting_enabled = Some(value),
            "TRANSCRIPT_TAGGING_ENABLED" => self.transcript_tagging_enabled = Some(value),
            "BEEP_ON_START" => self.beep_on_start = Some(value),
            "SHOW_DOCK_ICON" => self.show_dock_icon = Some(value),
            "TRANSCRIPTION_OVERLAY_ENABLED" => self.transcription_overlay_enabled = Some(value),
            "HOLD_EXCLUSIVE" => self.hold_exclusive = Some(value),
            "USE_LOCAL_STT" => self.use_local_stt = Some(value),
            "HISTORY_ENABLED" => self.history_enabled = Some(value),
            "QUICK_NOTES_ENABLED" => self.quick_notes_enabled = Some(value),
            "QUICK_NOTES_SAVE_ONLY" => self.quick_notes_save_only = Some(value),
            "START_AT_LOGIN" => self.start_at_login = Some(value),
            "QUBE_DAEMON_AUTOSTART" => self.qube_daemon_autostart = Some(value),
            "AGENT_ENTER_SENDS" => self.agent_enter_sends = Some(value),
            other => {
                warn!("Unknown bool setting key: {other}");
                return;
            }
        }
        self.save_if_changed(&before, "set_bool", key);
    }

    /// Sets a u64-valued setting by its .env key name and saves.
    pub fn set_u64(&mut self, key: &str, value: u64) {
        let before = self.clone();
        match key {
            "HOLD_START_DELAY_MS" => self.hold_start_delay_ms = Some(value),
            "DOUBLE_TAP_INTERVAL_MS" => self.double_tap_interval_ms = Some(value),
            "CODESCRIBE_BUFFER_DELAY_MS" => self.buffer_delay_ms = Some(value),
            "CODESCRIBE_EMIT_WORDS_MAX" => self.emit_words_max = Some(value),
            "BACKEND_MAX_UPLOAD_MB" => self.backend_max_upload_mb = Some(value),
            other => {
                warn!("Unknown u64 setting key: {other}");
                return;
            }
        }
        self.save_if_changed(&before, "set_u64", key);
    }

    /// Sets an f32-valued setting by its .env key name and saves.
    pub fn set_f32(&mut self, key: &str, value: f32) {
        let before = self.clone();
        match key {
            "SOUND_VOLUME" => self.sound_volume = Some(value),
            "TOGGLE_SILENCE_SEC" => self.toggle_silence_sec = Some(value),
            "CODESCRIBE_TYPING_CPS" => self.typing_cps = Some(value),
            "CODESCRIBE_BUFFERED_INTERIM_SEC" => self.buffered_interim_sec = Some(value),
            other => {
                warn!("Unknown f32 setting key: {other}");
                return;
            }
        }
        self.save_if_changed(&before, "set_f32", key);
    }
}

#[cfg(test)]
mod tests {
    use super::UserSettings;
    use crate::config::{ShortcutBinding, WorkMode};
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn setup_isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        // SAFETY: tests are serial and intentionally override process env.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
            std::env::remove_var("HOLD_MODS");
            std::env::remove_var("TOGGLE_TRIGGER");
        }
        tmp
    }

    #[test]
    fn test_normalized_chat_zoom_rules() {
        assert_eq!(UserSettings::normalized_chat_zoom(1.0), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.004), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.125), Some(1.13));
        assert_eq!(UserSettings::normalized_chat_zoom(0.1), Some(0.75));
        assert_eq!(UserSettings::normalized_chat_zoom(4.0), Some(2.0));
    }

    #[test]
    #[serial]
    fn test_set_chat_zoom_writes_only_on_effective_change() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        let path = UserSettings::settings_path();

        // Default zoom is encoded as None, so this should be a no-op (no file write).
        assert!(!settings.set_chat_zoom(1.0));
        assert!(
            !path.exists(),
            "no-op zoom update should not create settings file"
        );

        assert!(settings.set_chat_zoom(1.125));
        let first_contents = fs::read_to_string(&path).expect("read settings after first write");

        // 1.129 rounds to the same persisted value (1.13), so no write.
        assert!(!settings.set_chat_zoom(1.129));
        let second_contents = fs::read_to_string(&path).expect("read settings after no-op write");
        assert_eq!(first_contents, second_contents);
    }

    #[test]
    #[serial]
    fn test_v1_settings_hard_migrate_to_v2_with_backup() {
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        fs::write(
            &path,
            r#"{
  "chat_zoom": 1.2
}"#,
        )
        .expect("write v1 settings");

        let loaded = UserSettings::load();
        assert_eq!(
            loaded.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::HoldFn
        );

        let backup = UserSettings::settings_dir().join("settings.v1.bak.json");
        assert!(backup.exists(), "expected v1 backup file");

        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read migrated settings"))
                .expect("parse migrated settings");
        assert_eq!(
            migrated.get("schema_version").and_then(|v| v.as_u64()),
            Some(3)
        );
        assert!(
            migrated
                .get("interaction")
                .and_then(|v| v.get("mode_bindings"))
                .and_then(|v| v.as_array())
                .is_some_and(|bindings| !bindings.is_empty()),
            "mode bindings must be persisted as canonical hotkey contract"
        );
    }

    #[test]
    #[serial]
    fn test_show_dock_icon_bool_persists_and_roundtrips() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_bool("SHOW_DOCK_ICON", false);

        assert_eq!(settings.show_dock_icon, Some(false));

        let loaded = UserSettings::load();
        assert_eq!(loaded.show_dock_icon, Some(false));
    }

    #[test]
    #[serial]
    fn test_deghosted_keys_survive_settings_json_roundtrip() {
        // Regression guard (2026-05-30): these keys existed in UserSettings and were
        // promoted, but to_v2/from_v2 dropped them — save→load silently reverted them to
        // default. settings.json must support ALL non-secret parameters (operator's
        // "settings musi obsługiwać wszystkie parametry"). Exercises the real on-disk path.
        let _tmp = setup_isolated_data_dir();
        let settings = UserSettings {
            transcript_send_mode: Some("paste".to_string()),
            quick_notes_save_only: Some(true),
            agent_enter_sends: Some(false),
            whisper_model: Some("whisper-large-v3-turbo".to_string()),
            llm_endpoint: Some("https://api.example/v1/responses".to_string()),
            llm_model: Some("gpt-4.1".to_string()),
            ..Default::default()
        };
        settings.save().expect("save settings");

        let loaded = UserSettings::load();
        assert_eq!(loaded.transcript_send_mode.as_deref(), Some("paste"));
        assert_eq!(loaded.quick_notes_save_only, Some(true));
        assert_eq!(loaded.agent_enter_sends, Some(false));
        assert_eq!(
            loaded.whisper_model.as_deref(),
            Some("whisper-large-v3-turbo")
        );
        assert_eq!(
            loaded.llm_endpoint.as_deref(),
            Some("https://api.example/v1/responses")
        );
        assert_eq!(loaded.llm_model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    #[serial]
    fn test_transcription_overlay_enabled_persists_in_v2_ui_section() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_bool("TRANSCRIPTION_OVERLAY_ENABLED", false);

        let loaded = UserSettings::load();
        assert_eq!(loaded.transcription_overlay_enabled, Some(false));

        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read persisted settings"))
                .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("ui")
                .and_then(|v| v.get("transcription_overlay_enabled"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    #[serial]
    fn test_qube_daemon_autostart_persists_in_v2_system_section() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_bool("QUBE_DAEMON_AUTOSTART", true);

        let loaded = UserSettings::load();
        assert_eq!(loaded.qube_daemon_autostart, Some(true));

        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read persisted settings"))
                .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("system")
                .and_then(|v| v.get("qube_daemon_autostart"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    #[serial]
    fn test_mode_binding_updates_canonical_contract_only() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();

        settings.set_mode_binding(WorkMode::Dictation, ShortcutBinding::DoubleCtrl);
        assert_eq!(
            settings.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::DoubleCtrl
        );
    }

    #[test]
    #[serial]
    fn test_mode_binding_partial_update_preserves_other_modes() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();

        settings.set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
        settings.set_mode_binding(WorkMode::Assistive, ShortcutBinding::DoubleRightOption);

        assert_eq!(
            settings.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::HoldFn
        );
        assert_eq!(
            settings.mode_binding_for(WorkMode::Formatting),
            ShortcutBinding::Disabled
        );
        assert_eq!(
            settings.mode_binding_for(WorkMode::Assistive),
            ShortcutBinding::DoubleRightOption
        );
    }

    #[test]
    #[serial]
    fn test_load_ignores_legacy_hotkey_env_imports() {
        let _tmp = setup_isolated_data_dir();

        unsafe {
            std::env::set_var("HOLD_MODS", "ctrl_alt");
            std::env::set_var("TOGGLE_TRIGGER", "double_ralt");
        }

        let settings = UserSettings::load();
        assert_eq!(
            settings.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::HoldFn
        );
        assert_eq!(
            settings.mode_binding_for(WorkMode::Formatting),
            ShortcutBinding::DoubleLeftOption
        );
        assert_eq!(
            settings.mode_binding_for(WorkMode::Assistive),
            ShortcutBinding::DoubleRightOption
        );
        assert!(
            !UserSettings::settings_path().exists(),
            "loading legacy hotkey envs should not synthesize settings.json"
        );
    }

    #[test]
    #[serial]
    fn test_saved_mode_bindings_outrank_legacy_hotkey_env_noise() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_mode_binding(WorkMode::Dictation, ShortcutBinding::HoldCtrlCmd);

        unsafe {
            std::env::set_var("HOLD_MODS", "ctrl_alt");
            std::env::set_var("TOGGLE_TRIGGER", "double_ctrl");
        }

        let loaded = UserSettings::load();
        assert_eq!(
            loaded.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::HoldCtrlCmd
        );
        assert_eq!(
            loaded.mode_binding_for(WorkMode::Formatting),
            ShortcutBinding::DoubleLeftOption
        );
        assert_eq!(
            loaded.mode_binding_for(WorkMode::Assistive),
            ShortcutBinding::DoubleRightOption
        );
    }
}
