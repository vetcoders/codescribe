//! User-facing settings stored as JSON (GUI-managed).
//!
//! These are the "regular user" tier. Power users override via ~/.codescribe/.env.

use super::types::{ModeBinding, ShortcutBinding, WorkMode, default_mode_bindings};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Canonical formatting policy shared by persistence, runtime selection, and UI.
///
/// Legacy values are accepted only at this boundary and are normalized before
/// any new write. Unknown values are errors; they are never promoted to a more
/// aggressive policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FormattingPolicy {
    Off,
    #[default]
    Correction,
    Smart,
    Max,
}

impl FormattingPolicy {
    /// Every policy in increasing-aggressiveness order, for UI pickers that
    /// must not hand-maintain their own copy of the list.
    pub const ALL: [Self; 4] = [Self::Off, Self::Correction, Self::Smart, Self::Max];

    /// Canonical persisted spelling. Always a modern name — writing back a
    /// legacy alias is what the normalization boundary exists to prevent.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Correction => "correction",
            Self::Smart => "smart",
            Self::Max => "max",
        }
    }

    /// The one place legacy aliases (`raw`, `medium`, `creative`) are accepted.
    /// An unrecognized value is an error rather than a fallback: silently
    /// defaulting would hand the user a *more* aggressive policy than they set.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim() {
            "off" | "raw" => Ok(Self::Off),
            "correction" | "medium" => Ok(Self::Correction),
            "smart" => Ok(Self::Smart),
            "max" | "creative" => Ok(Self::Max),
            value => anyhow::bail!(
                "unknown FORMATTING_LEVEL {value:?}; expected off, correction, smart, or max"
            ),
        }
    }

    /// Precedence for the effective policy: process env beats `settings.json`,
    /// which beats the built-in default. A malformed value at either layer is
    /// surfaced as an error instead of being skipped over.
    pub fn resolve(runtime: Option<&str>, persisted: Option<&str>) -> anyhow::Result<Self> {
        runtime
            .or(persisted)
            .map(Self::parse)
            .unwrap_or_else(|| Ok(Self::default()))
    }
}

/// Built-in workspace root used only when no user-managed roots exist.
///
/// Deliberately the app's own data dir: it always exists (green "exists" dot
/// from first launch) and holds no git checkouts, so a fresh install lists
/// nothing instead of inventing a directory convention. The previous `~/Git`
/// default was an accidental import from another operator's layout, not a
/// chosen contract (removed 2026-08-09).
pub const DEFAULT_AGENT_WORKSPACE_ROOT: &str = "~/.codescribe";

/// Trim workspace-root entries and discard empty rows while preserving the
/// operator's order. This is the canonical normalization boundary shared by
/// persistence, the agent tools, and readiness.
pub fn normalize_agent_workspace_roots<I, S>(roots: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    roots
        .into_iter()
        .map(|root| root.as_ref().trim().to_string())
        .filter(|root| !root.is_empty())
        .collect()
}

/// Parse the colon-joined UniFFI/env wire representation used by Settings.
pub fn parse_agent_workspace_roots(value: &str) -> Vec<String> {
    normalize_agent_workspace_roots(value.split(':'))
}

/// Regular-user settings (JSON, GUI-managed).
/// All fields are Option — None means "use default or .env override".
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct UserSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_exclusive: Option<bool>,
    /// Assistive-arm modifier on hold base: `"shift"` (default) or `"cmd"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_arm_modifier: Option<String>,
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
    pub auto_paste_enabled: Option<bool>,
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
    pub llm_assistive_provider: Option<String>,
    /// Optional override for the OpenAI OAuth client id (non-secret app identity).
    /// `None` falls through to env, then the shipped Codex CLI public app id
    /// (see `NOTICE`). Env `CODESCRIBE_OPENAI_OAUTH_CLIENT_ID` is the dev fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_oauth_client_id: Option<String>,
    /// Same contract as `openai_oauth_client_id`, for Anthropic account login.
    /// No shipped default — `None` ⇒ "awaiting app registration"; env
    /// `CODESCRIBE_ANTHROPIC_OAUTH_CLIENT_ID` is the dev-only fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_oauth_client_id: Option<String>,
    /// Optional override for xAI OAuth client id. `None` falls through to env,
    /// then the shipped Grok CLI public client id (`NOTICE`). Env
    /// `CODESCRIBE_XAI_OAUTH_CLIENT_ID` is the dev-only mid-tier fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xai_oauth_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_zoom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcription_overlay_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tray_start_assistive: Option<bool>,
    // Promoted 2026-08-11: these lived only in `.env`, so the tray/settings
    // writers died silently once the file became unwritable (uchg lock) and
    // the 2026-08-08 wipe erased the user's values outright.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_indicator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_badge_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_clipboard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_clipboard_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_insert_shortcut: Option<String>,

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
    /// Opt-in qube donor (`on` | `off`). Seeds `CODESCRIBE_QUBE_DONOR`. Default off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qube_donor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_enter_sends: Option<bool>,
    /// First-run operating lane chosen during onboarding ("basic" | "agentic").
    /// `None` means "not yet chosen" — callers treat that as the safe Basic lane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onboarding_mode: Option<String>,

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

    // ── STT engine / layered transcription (F1) ──
    /// STT engine selection ("auto" | "apple" | "whisper").
    /// Seeds `CODESCRIBE_STT_ENGINE`; string on purpose (1:1 env mapping, like
    /// `onboarding_mode`). `None`/absent means the built-in auto policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_engine: Option<String>,
    /// Final-pass routing mode (`always` | `smart` | `off`).
    /// Seeds `FINAL_PASS_MODE` (alias `CODESCRIBE_FINAL_PASS_MODE`). Default
    /// Smart when absent. Distinct from lexicon `FinalPassMode` in contracts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_pass_mode: Option<String>,
    /// Layered incremental transcription phase ("off" | "phase1").
    /// Seeds `CODESCRIBE_LAYERED_TRANSCRIPTION`; anything other than
    /// "phase1".."phase4" (or bare "1".."4") is treated as OFF by the core.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layered_transcription: Option<String>,
    /// Opt-in Whisper `initial_prompt` vocabulary hint.
    /// Seeds `CODESCRIBE_STT_INITIAL_PROMPT_ENABLED`; absent means default OFF.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_initial_prompt_enabled: Option<bool>,

    // ── Layer 1 ASR product mode + audio-egress consent (C2) ──
    /// Layer 1 product mode (`cloud` | `local_power` | `apple_only`).
    /// `None` means "not yet chosen": the resolver derives the mode from the
    /// legacy `use_local_stt` choice (upgrades) or lands on Apple-only (fresh).
    /// Writes are validated through [`crate::config::cloud_asr::AsrProductMode`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_mode: Option<String>,
    /// Audio-egress consent record (`granted` | `denied`). `None` means never
    /// asked; anything non-canonical reads as unanswered (fail closed). Cloud
    /// mode without a granted record resolves to Apple-only — see
    /// [`UserSettings::resolved_asr_mode`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_consent: Option<String>,
    /// RFC 3339 timestamp of the last explicit consent answer. Informational
    /// provenance only — never an input to the resolver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_consent_at: Option<String>,
    /// Libraxis gateway session-mint endpoint. Endpoint only, never a vendor
    /// key: writes are validated through
    /// [`crate::config::cloud_asr::GatewaySessionMint`], which refuses
    /// user-info and query material. `None` means "not configured".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_gateway_url: Option<String>,

    // ── Agent workspace ──
    /// Workspace root directories the agent scans (`list_projects`) to resolve a
    /// project name to an absolute path. The Settings UI sends the
    /// `AGENT_WORKSPACE_ROOTS` wire key, but this field in durable
    /// `settings.json` is the source of truth. `None`/absent means the built-in
    /// default (`~/.codescribe` — exists everywhere, holds no checkouts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_workspace_roots: Option<Vec<String>>,

    // ── Agent tool permissions (B2 gateway) ──
    /// Global allow/ask/deny policy for agent tools. Stored under
    /// `agent.permissions` in settings.json. `None` means migration defaults
    /// (read-only → allow, side-effectful → ask) applied at resolve time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_permissions: Option<crate::agent::permissions::AgentPermissions>,

    // ── Agent capability preferences (provider-neutral broker) ──
    /// Schema-backed capability preferences under `agent.capabilities`.
    /// Not a copied list of every connector tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_capabilities: Option<crate::agent::capabilities::AgentCapabilityPreferences>,
}

/// On-disk shape of `settings.json`: the same state as [`UserSettings`], but
/// grouped by domain instead of flat.
///
/// The two representations must stay in bijection. A field that exists in
/// `UserSettings` but is dropped by [`UserSettings::to_v2`] or
/// [`UserSettings::from_v2`] "ghosts": the user can set it, and `save` → `load`
/// silently reverts it. The `De-ghosted (2026-05-30)` markers below are the
/// scars of exactly that failure.
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
    /// Agent runtime preferences (tool permissions gateway, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<AgentV2>,
}

/// `agent` section. Written only when at least one of its parts is present, so
/// a user who never touched agent settings gets no empty section on disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct AgentV2 {
    /// Tool permission policy: global + per-server + per-tool allow/ask/deny.
    #[serde(skip_serializing_if = "Option::is_none")]
    permissions: Option<crate::agent::permissions::AgentPermissions>,
    /// Provider-neutral capability preferences (not a connector tool dump).
    #[serde(skip_serializing_if = "Option::is_none")]
    capabilities: Option<crate::agent::capabilities::AgentCapabilityPreferences>,
}

/// `interaction` section: how the user starts, stops, and delivers dictation.
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
    /// User-owned automatic delivery policy shared by Hold and hands-free
    /// dictation. Assistive and safety vetoes are enforced by the controller.
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_paste_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deferred_insert_shortcut: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore_clipboard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore_clipboard_delay_ms: Option<u64>,
}

/// Timing of the tap-based triggers: how fast a double tap must be, and how
/// long silence may run before a toggled session closes itself.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct TriggerV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    double_tap_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    toggle_silence_timeout_sec: Option<f32>,
}

/// Hold-to-talk behaviour: whether the hold key is claimed exclusively, which
/// modifier arms the assistive lane, and the delay before recording starts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct HoldV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    exclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arm_modifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_delay_ms: Option<u64>,
}

/// `speech` section: everything between the microphone and the delivered text —
/// recognition engine, formatting, the assistive lane, and emission cadence.
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

/// Which recognizer runs and how. `mode` is the legacy local/cloud switch;
/// `stt_engine` is the newer three-way selector that supersedes it. Both are
/// kept because existing files on disk still carry the former.
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
    // F1 layered transcription: engine selector + phase flag (string, 1:1 env).
    #[serde(skip_serializing_if = "Option::is_none")]
    stt_engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_pass_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layered_transcription: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_prompt_enabled: Option<bool>,
    // C2: Layer 1 product mode (cloud | local_power | apple_only) and the
    // gateway session-mint endpoint it uses when cloud is armed.
    #[serde(skip_serializing_if = "Option::is_none")]
    asr_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gateway_session_url: Option<String>,
}

/// LLM post-processing of the transcript: whether it runs, how aggressively,
/// and against which endpoint. The endpoint/model here override the base
/// `speech.llm_*` pair for the formatting lane only.
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

/// The assistive lane's own provider triple. Separate from formatting so the
/// two lanes can run on different models — and so a key written for one lane
/// cannot quietly serve the other.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct AssistiveV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

/// Pacing of text as it lands in the target app: buffering delay, typing
/// speed, chunk size, and how often interim results are refreshed.
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

/// `audio` section: capture device plus the audible feedback that tells the
/// user recording actually started.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct AudioV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    input_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback: Option<FeedbackV2>,
}

/// Start-of-recording cue: whether it sounds, which sound, and how loudly.
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

/// `ui` section: chrome the user sees — chat zoom, Dock presence, the live
/// transcription overlay, and which lane the tray starts in.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct UiV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_zoom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcription_overlay_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tray_start_assistive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hold_indicator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hold_badge_size: Option<u64>,
}

/// `features` section: optional surfaces the user can switch off entirely,
/// such as transcript history and quick notes.
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

/// `system` section: install-level state and app identity — launch at login,
/// the qube daemon, onboarding lane, agent workspace roots, and the OAuth
/// client ids. The client ids live here rather than in the Keychain because
/// they identify the app, not the user.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SystemV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    start_at_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qube_daemon_autostart: Option<bool>,
    /// Opt-in qube donor (`on` | `off`). Seeds `CODESCRIBE_QUBE_DONOR`.
    #[serde(skip_serializing_if = "Option::is_none")]
    qube_donor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    onboarding_mode: Option<String>,
    // Agent workspace roots (colon-joined into AGENT_WORKSPACE_ROOTS). List on
    // purpose — mirrors mode_bindings' Vec round-trip through the V2 schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_workspace_roots: Option<Vec<String>>,
    // "Sign in with ChatGPT" OAuth client id (non-secret app identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    openai_oauth_client_id: Option<String>,
    // Anthropic account-login OAuth client id (non-secret app identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_oauth_client_id: Option<String>,
    // xAI account-login OAuth client id (non-secret app identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    xai_oauth_client_id: Option<String>,
    // C2: audio-egress consent record — install-level privacy state, kept in
    // `system` so engine-section rewrites can never touch it.
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_audio_egress_consent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_audio_egress_consent_at: Option<String>,
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
    "HOLD_ARM_MODIFIER",
    // AI / Formatting
    "AI_FORMATTING_ENABLED",
    "AUTO_PASTE_ENABLED",
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
    "TRAY_START_ASSISTIVE",
    // Pointer indicator + delivery (promoted 2026-08-11: .env writes died
    // silently under the uchg lock, killing the tray Pointer Indicator row)
    "HOLD_INDICATOR",
    "HOLD_BADGE_SIZE",
    "RESTORE_CLIPBOARD",
    "RESTORE_CLIPBOARD_DELAY_MS",
    "CODESCRIBE_DEFERRED_INSERT_SHORTCUT",
    // LLM endpoints
    "LLM_ENDPOINT",
    "LLM_MODEL",
    "LLM_ASSISTIVE_ENDPOINT",
    "LLM_ASSISTIVE_MODEL",
    "LLM_ASSISTIVE_PROVIDER",
    "LLM_FORMATTING_ENDPOINT",
    "LLM_FORMATTING_MODEL",
    // Account-login OAuth client ids — non-secret app identities, so they live
    // in settings.json (NOT the Keychain); env stays the dev fallback.
    "LLM_OPENAI_OAUTH_CLIENT_ID",
    "LLM_ANTHROPIC_OAUTH_CLIENT_ID",
    "LLM_XAI_OAUTH_CLIENT_ID",
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
    "ONBOARDING_MODE",
    "AGENT_WORKSPACE_ROOTS",
    // Voice Lab survivors
    "CODESCRIBE_BUFFER_DELAY_MS",
    "CODESCRIBE_TYPING_CPS",
    "CODESCRIBE_EMIT_WORDS_MAX",
    "CODESCRIBE_BUFFERED_INTERIM_SEC",
    "WHISPER_MODEL",
    "BACKEND_MAX_UPLOAD_MB",
    // STT contract (2026-07-24): engine + final-pass are product settings.
    // UI writes land in settings.json; process env is reconciled on write so
    // a stale ~/.codescribe/.env line cannot silently lottery the live path.
    "CODESCRIBE_STT_ENGINE",
    "FINAL_PASS_MODE",
    "CODESCRIBE_FINAL_PASS_MODE",
    // Promoted 2026-08-10: the un-promoted toggle wrote .env only, the stale
    // process env won the UI read-back, and the Layered switch snapped OFF.
    "CODESCRIBE_LAYERED_TRANSCRIPTION",
    // C2: Layer 1 product mode, audio-egress consent, gateway mint endpoint.
    // settings.json is the single brain — no .env dual-write for these.
    "CODESCRIBE_ASR_MODE",
    "CODESCRIBE_CLOUD_CONSENT",
    "CODESCRIBE_ASR_GATEWAY_URL",
    // Still env-seedable when unset; not full dual-brain:
    // "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED",
];

/// Check if a key is a promoted (settings.json) setting.
pub fn is_promoted_key(key: &str) -> bool {
    PROMOTED_SETTINGS_KEYS.contains(&key)
}

impl UserSettings {
    /// Project the flat settings onto the nested on-disk schema. Always writes
    /// `schema_version: 3` and normalized values, so re-saving an older file
    /// upgrades it in place. Every field added to `UserSettings` must be routed
    /// here and in [`Self::from_v2`], or it ghosts on the next round-trip.
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
                    arm_modifier: self.hold_arm_modifier.clone(),
                    start_delay_ms: self.hold_start_delay_ms,
                }),
                mode_bindings: Some(normalized_mode_bindings),
                send_mode: self.transcript_send_mode.clone(),
                agent_enter_sends: self.agent_enter_sends,
                auto_paste_enabled: self.auto_paste_enabled,
                deferred_insert_shortcut: self.deferred_insert_shortcut.clone(),
                restore_clipboard: self.restore_clipboard,
                restore_clipboard_delay_ms: self.restore_clipboard_delay_ms,
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
                    stt_engine: self.stt_engine.clone(),
                    final_pass_mode: self.final_pass_mode.clone(),
                    layered_transcription: self.layered_transcription.clone(),
                    initial_prompt_enabled: self.stt_initial_prompt_enabled,
                    asr_mode: self.asr_mode.clone(),
                    gateway_session_url: self.asr_gateway_url.clone(),
                }),
                formatting: Some(FormattingV2 {
                    enabled: self.ai_formatting_enabled,
                    transcript_tagging_enabled: self.transcript_tagging_enabled,
                    transcript_tag_template: self.transcript_tag_template.clone(),
                    level: self
                        .formatting_level
                        .as_deref()
                        .and_then(|value| FormattingPolicy::parse(value).ok())
                        .map(|policy| policy.as_str().to_string()),
                    llm_endpoint: self.llm_formatting_endpoint.clone(),
                    llm_model: self.llm_formatting_model.clone(),
                }),
                assistive: Some(AssistiveV2 {
                    llm_endpoint: self.llm_assistive_endpoint.clone(),
                    llm_model: self.llm_assistive_model.clone(),
                    provider: self.llm_assistive_provider.clone(),
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
                tray_start_assistive: self.tray_start_assistive,
                hold_indicator: self.hold_indicator,
                hold_badge_size: self.hold_badge_size,
            }),
            features: Some(FeaturesV2 {
                history_enabled: self.history_enabled,
                quick_notes_enabled: self.quick_notes_enabled,
                quick_notes_save_only: self.quick_notes_save_only,
            }),
            system: Some(SystemV2 {
                start_at_login: self.start_at_login,
                qube_daemon_autostart: self.qube_daemon_autostart,
                qube_donor: self.qube_donor.clone(),
                onboarding_mode: self.onboarding_mode.clone(),
                agent_workspace_roots: self.agent_workspace_roots.clone(),
                openai_oauth_client_id: self.openai_oauth_client_id.clone(),
                anthropic_oauth_client_id: self.anthropic_oauth_client_id.clone(),
                xai_oauth_client_id: self.xai_oauth_client_id.clone(),
                cloud_audio_egress_consent: self.cloud_consent.clone(),
                cloud_audio_egress_consent_at: self.cloud_consent_at.clone(),
            }),
            agent: match (
                self.agent_permissions.clone(),
                self.agent_capabilities.clone(),
            ) {
                (None, None) => None,
                (permissions, capabilities) => Some(AgentV2 {
                    permissions,
                    capabilities,
                }),
            },
        }
    }

    /// Flatten the on-disk schema back into runtime settings. Missing sections
    /// collapse to `None` rather than failing, which is what lets a partially
    /// written file still load.
    ///
    /// Two fields deliberately do not: `stt_engine` and `final_pass_mode` fall
    /// back to the product defaults (`apple` / `smart`). An empty
    /// `speech.engine: {}` used to leave them unset, handing the decision to
    /// whatever the environment happened to say.
    fn from_v2(v2: SettingsV2) -> Self {
        Self {
            whisper_language: v2.speech.as_ref().and_then(|s| s.language.clone()),
            hold_exclusive: v2
                .interaction
                .as_ref()
                .and_then(|i| i.hold.as_ref())
                .and_then(|h| h.exclusive),
            hold_arm_modifier: v2
                .interaction
                .as_ref()
                .and_then(|i| i.hold.as_ref())
                .and_then(|h| h.arm_modifier.clone()),
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
            auto_paste_enabled: v2
                .interaction
                .as_ref()
                .and_then(|interaction| interaction.auto_paste_enabled),
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
                .and_then(|f| f.level.as_deref())
                .and_then(|value| FormattingPolicy::parse(value).ok())
                .map(|policy| policy.as_str().to_string()),
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
            llm_assistive_provider: v2
                .speech
                .as_ref()
                .and_then(|s| s.assistive.as_ref())
                .and_then(|a| a.provider.clone()),
            chat_zoom: v2.ui.as_ref().and_then(|ui| ui.chat_zoom),
            show_dock_icon: v2.ui.as_ref().and_then(|ui| ui.show_dock_icon),
            transcription_overlay_enabled: v2
                .ui
                .as_ref()
                .and_then(|ui| ui.transcription_overlay_enabled),
            tray_start_assistive: v2.ui.as_ref().and_then(|ui| ui.tray_start_assistive),
            hold_indicator: v2.ui.as_ref().and_then(|ui| ui.hold_indicator),
            hold_badge_size: v2.ui.as_ref().and_then(|ui| ui.hold_badge_size),
            deferred_insert_shortcut: v2
                .interaction
                .as_ref()
                .and_then(|interaction| interaction.deferred_insert_shortcut.clone()),
            restore_clipboard: v2
                .interaction
                .as_ref()
                .and_then(|interaction| interaction.restore_clipboard),
            restore_clipboard_delay_ms: v2
                .interaction
                .as_ref()
                .and_then(|interaction| interaction.restore_clipboard_delay_ms),
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
                .filter(|mode| !mode.trim().is_empty())
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
            qube_donor: v2.system.as_ref().and_then(|s| s.qube_donor.clone()),
            onboarding_mode: v2.system.as_ref().and_then(|s| s.onboarding_mode.clone()),
            agent_workspace_roots: v2
                .system
                .as_ref()
                .and_then(|s| s.agent_workspace_roots.clone()),
            openai_oauth_client_id: v2
                .system
                .as_ref()
                .and_then(|s| s.openai_oauth_client_id.clone()),
            anthropic_oauth_client_id: v2
                .system
                .as_ref()
                .and_then(|s| s.anthropic_oauth_client_id.clone()),
            xai_oauth_client_id: v2
                .system
                .as_ref()
                .and_then(|s| s.xai_oauth_client_id.clone()),
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
            // Product default: Apple live (must-have). Empty `speech.engine: {}`
            // used to leave stt_engine=None → env/auto lottery; pin apple.
            stt_engine: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.stt_engine.clone())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| Some("apple".to_string())),
            final_pass_mode: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.final_pass_mode.clone())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| Some("smart".to_string())),
            layered_transcription: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.layered_transcription.clone()),
            stt_initial_prompt_enabled: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.initial_prompt_enabled),
            asr_mode: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.asr_mode.clone()),
            asr_gateway_url: v2
                .speech
                .as_ref()
                .and_then(|s| s.engine.as_ref())
                .and_then(|e| e.gateway_session_url.clone()),
            cloud_consent: v2
                .system
                .as_ref()
                .and_then(|s| s.cloud_audio_egress_consent.clone()),
            cloud_consent_at: v2
                .system
                .as_ref()
                .and_then(|s| s.cloud_audio_egress_consent_at.clone()),
            agent_permissions: v2.agent.as_ref().and_then(|a| a.permissions.clone()),
            agent_capabilities: v2.agent.as_ref().and_then(|a| a.capabilities.clone()),
        }
    }

    /// Reject a file that would load into nonsense: unsupported schema version,
    /// out-of-range zoom, or an unparseable formatting level. Runs on both read
    /// and write, so a bad value can neither be loaded nor persisted.
    fn validate_v2(v2: &SettingsV2) -> anyhow::Result<()> {
        if v2.schema_version != 2 && v2.schema_version != 3 {
            anyhow::bail!("settings schema_version must be 2 or 3")
        }
        if let Some(chat_zoom) = v2.ui.as_ref().and_then(|ui| ui.chat_zoom)
            && !(0.75..=2.0).contains(&chat_zoom)
        {
            anyhow::bail!("ui.chat_zoom must be within [0.75, 2.0]")
        }
        if let Some(level) = v2
            .speech
            .as_ref()
            .and_then(|speech| speech.formatting.as_ref())
            .and_then(|formatting| formatting.level.as_deref())
        {
            FormattingPolicy::parse(level)?;
        }
        Ok(())
    }

    /// Write via temp file plus rename, so a crash mid-write leaves the previous
    /// `settings.json` intact rather than a truncated one the app would treat
    /// as corrupt and silently replace with defaults.
    fn write_json_atomic(path: &PathBuf, json: &str) -> anyhow::Result<()> {
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Returns the settings directory.
    ///
    /// Respects `CODESCRIBE_DATA_DIR` for test isolation; otherwise uses
    /// `~/Library/Application Support/Codescribe/`.
    pub fn settings_dir() -> PathBuf {
        let dir = if let Ok(test_dir) = std::env::var("CODESCRIBE_DATA_DIR") {
            PathBuf::from(test_dir)
        } else {
            BaseDirs::new()
                .map(|b| b.data_dir().join("Codescribe"))
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    PathBuf::from(home).join("Library/Application Support/Codescribe")
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
        if let Some(level) = self.formatting_level.as_deref() {
            FormattingPolicy::parse(level)?;
        }
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

    /// Persist only when the setter actually changed something. Setters are
    /// called on every UI event, so writing unconditionally would rewrite the
    /// file constantly — and a failed save is logged, not propagated, because
    /// the in-memory value has already been updated.
    fn save_if_changed(&self, before: &Self, setter: &str, key: &str) {
        if self == before {
            debug!("{setter}({key}) ignored; value unchanged");
            return;
        }
        if let Err(e) = self.save() {
            warn!("Failed to save after {setter}({key}): {e}");
        }
    }

    /// Overlay the user's bindings on the built-in defaults, so a settings file
    /// that mentions one mode does not leave the others unbound. Unknown modes
    /// are appended rather than dropped — a downgrade must not destroy bindings
    /// written by a newer build.
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

    /// Effective shortcut for a work mode, defaults included. A mode with no
    /// binding at all reads as `Disabled` rather than panicking.
    pub fn mode_binding_for(&self, mode: WorkMode) -> ShortcutBinding {
        self.mode_bindings_normalized()
            .into_iter()
            .find(|binding| binding.mode == mode)
            .map(|binding| binding.binding)
            .unwrap_or(ShortcutBinding::Disabled)
    }

    /// Rebind one work mode and persist. Writes the *full* normalized set, so
    /// the file always states every mode explicitly instead of relying on
    /// defaults that a later release might change under the user.
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
            "LLM_ASSISTIVE_PROVIDER" => self.llm_assistive_provider = Some(value.to_owned()),
            "LLM_OPENAI_OAUTH_CLIENT_ID" => {
                // Empty clears back to the shipped Codex CLI public app id.
                let trimmed = value.trim();
                self.openai_oauth_client_id = (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
            "LLM_ANTHROPIC_OAUTH_CLIENT_ID" => {
                let trimmed = value.trim();
                self.anthropic_oauth_client_id = (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
            "LLM_XAI_OAUTH_CLIENT_ID" => {
                // Empty clears back to xAI's published desktop client id.
                let trimmed = value.trim();
                self.xai_oauth_client_id = (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
            "FORMATTING_LEVEL" => match FormattingPolicy::parse(value) {
                Ok(policy) => self.formatting_level = Some(policy.as_str().to_string()),
                Err(error) => {
                    warn!("Rejected formatting policy write: {error}");
                    return;
                }
            },
            "CODESCRIBE_DEFERRED_INSERT_SHORTCUT" => {
                match value.parse::<crate::config::DeferredInsertShortcut>() {
                    Ok(shortcut) => {
                        self.deferred_insert_shortcut = Some(shortcut.wire_id().to_string())
                    }
                    Err(error) => {
                        warn!("Rejected deferred-insert shortcut write: {error}");
                        return;
                    }
                }
            }
            "TRANSCRIPT_TAG_TEMPLATE" => self.transcript_tag_template = Some(value.to_owned()),
            "LLM_FORMATTING_ENDPOINT" => self.llm_formatting_endpoint = Some(value.to_owned()),
            "LLM_FORMATTING_MODEL" => self.llm_formatting_model = Some(value.to_owned()),
            "LOCAL_MODEL" => self.local_model = Some(value.to_owned()),
            "STT_ENDPOINT" => self.stt_endpoint = Some(value.to_owned()),
            "TRANSCRIPT_SEND_MODE" => self.transcript_send_mode = Some(value.to_owned()),
            "AUDIO_INPUT_DEVICE" => self.audio_input_device = Some(value.to_owned()),
            "SOUND_NAME" => self.sound_name = Some(value.to_owned()),
            "WHISPER_MODEL" => self.whisper_model = Some(value.to_owned()),
            "ONBOARDING_MODE" => self.onboarding_mode = Some(value.to_owned()),
            "CODESCRIBE_STT_ENGINE" => self.stt_engine = Some(value.to_owned()),
            "FINAL_PASS_MODE" | "CODESCRIBE_FINAL_PASS_MODE" => {
                let normalized = value.trim().to_ascii_lowercase();
                match normalized.as_str() {
                    "always" | "smart" | "off" => {
                        self.final_pass_mode = Some(normalized);
                    }
                    _ => {
                        warn!(
                            "Rejected final_pass_mode write (expected always|smart|off): {value}"
                        );
                        return;
                    }
                }
            }
            "CODESCRIBE_LAYERED_TRANSCRIPTION" => {
                self.layered_transcription = Some(value.to_owned())
            }
            "CODESCRIBE_ASR_MODE" => {
                // Empty clears back to derivation (legacy choice or Apple-only).
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    self.asr_mode = None;
                } else {
                    match trimmed.parse::<crate::config::cloud_asr::AsrProductMode>() {
                        Ok(mode) => self.asr_mode = Some(mode.as_str().to_string()),
                        Err(error) => {
                            warn!("Rejected ASR mode write: {error}");
                            return;
                        }
                    }
                }
            }
            "CODESCRIBE_CLOUD_CONSENT" => {
                // Explicit answers only; empty clears the record back to
                // "never asked". Every answer stamps its provenance timestamp.
                let normalized = value.trim().to_ascii_lowercase();
                match normalized.as_str() {
                    "" => {
                        self.cloud_consent = None;
                        self.cloud_consent_at = None;
                    }
                    crate::config::cloud_asr::CONSENT_WIRE_GRANTED
                    | crate::config::cloud_asr::CONSENT_WIRE_DENIED => {
                        self.cloud_consent = Some(normalized);
                        self.cloud_consent_at = Some(chrono::Utc::now().to_rfc3339());
                    }
                    _ => {
                        warn!("Rejected cloud consent write (expected granted|denied): {value}");
                        return;
                    }
                }
            }
            "CODESCRIBE_ASR_GATEWAY_URL" => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    self.asr_gateway_url = None;
                } else {
                    match crate::config::cloud_asr::GatewaySessionMint::new(trimmed) {
                        Ok(mint) => self.asr_gateway_url = Some(mint.url().to_string()),
                        Err(error) => {
                            warn!("Rejected ASR gateway URL write: {error}");
                            return;
                        }
                    }
                }
            }
            "CODESCRIBE_QUBE_DONOR" => {
                let normalized = value.trim().to_ascii_lowercase();
                match normalized.as_str() {
                    "on" | "off" => self.qube_donor = Some(normalized),
                    _ => {
                        warn!("Rejected qube_donor write (expected on|off): {value}");
                        return;
                    }
                }
            }
            "AGENT_WORKSPACE_ROOTS" => {
                let roots = parse_agent_workspace_roots(value);
                self.agent_workspace_roots = (!roots.is_empty()).then_some(roots);
            }
            "HOLD_ARM_MODIFIER" => match value.parse::<crate::config::HoldArmModifier>() {
                Ok(arm) => self.hold_arm_modifier = Some(arm.as_str().to_string()),
                Err(error) => {
                    warn!("Rejected hold arm modifier write: {error}");
                    return;
                }
            },
            other => {
                warn!("Unknown string setting key: {other}");
                return;
            }
        }
        self.save_if_changed(&before, "set_string", key);
    }

    /// Resolve the effective Layer 1 product mode from this settings snapshot.
    ///
    /// The one sanctioned read path: combines the persisted `asr_mode`, the
    /// consent record, and the legacy `use_local_stt` choice through
    /// [`crate::config::cloud_asr::resolve_asr_product_mode`]. Callers must not
    /// re-derive policy from the raw fields.
    pub fn resolved_asr_mode(&self) -> crate::config::cloud_asr::ResolvedAsrMode {
        crate::config::cloud_asr::resolve_asr_product_mode(
            self.asr_mode.as_deref(),
            self.cloud_consent.as_deref(),
            self.use_local_stt,
        )
    }

    /// Sets a boolean-valued setting by its .env key name and saves.
    pub fn set_bool(&mut self, key: &str, value: bool) {
        let before = self.clone();
        match key {
            "AI_FORMATTING_ENABLED" => self.ai_formatting_enabled = Some(value),
            "AUTO_PASTE_ENABLED" => self.auto_paste_enabled = Some(value),
            "TRANSCRIPT_TAGGING_ENABLED" => self.transcript_tagging_enabled = Some(value),
            "BEEP_ON_START" => self.beep_on_start = Some(value),
            "SHOW_DOCK_ICON" => self.show_dock_icon = Some(value),
            "TRANSCRIPTION_OVERLAY_ENABLED" => self.transcription_overlay_enabled = Some(value),
            "TRAY_START_ASSISTIVE" => self.tray_start_assistive = Some(value),
            "HOLD_INDICATOR" => self.hold_indicator = Some(value),
            "RESTORE_CLIPBOARD" => self.restore_clipboard = Some(value),
            "HOLD_EXCLUSIVE" => self.hold_exclusive = Some(value),
            "USE_LOCAL_STT" => self.use_local_stt = Some(value),
            "HISTORY_ENABLED" => self.history_enabled = Some(value),
            "QUICK_NOTES_ENABLED" => self.quick_notes_enabled = Some(value),
            "QUICK_NOTES_SAVE_ONLY" => self.quick_notes_save_only = Some(value),
            "START_AT_LOGIN" => self.start_at_login = Some(value),
            "QUBE_DAEMON_AUTOSTART" => self.qube_daemon_autostart = Some(value),
            "AGENT_ENTER_SENDS" => self.agent_enter_sends = Some(value),
            "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED" => {
                self.stt_initial_prompt_enabled = Some(value)
            }
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
            "HOLD_BADGE_SIZE" => self.hold_badge_size = Some(value),
            "RESTORE_CLIPBOARD_DELAY_MS" => self.restore_clipboard_delay_ms = Some(value),
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

/// Persistence is exercised against real files in a temp data dir, not against
/// in-memory conversions — the failures these guard against (ghosted fields,
/// migration loss, write amplification) only appear on the round-trip through
/// disk. All tests are `#[serial]`: the data dir is selected by process env.
#[cfg(test)]
mod tests {
    use super::{FormattingPolicy, UserSettings, is_promoted_key};
    use crate::config::{ShortcutBinding, WorkMode};
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    /// Redirect the data dir to a fresh temp directory and clear the legacy
    /// hotkey env vars. The returned guard must stay alive for the whole test:
    /// dropping it deletes the directory the settings file lives in.
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

    /// Zoom normalization: clamped to the supported range, rounded to two
    /// decimals, and the effective default encoded as `None` so it is omitted
    /// from the file entirely.
    #[test]
    fn test_normalized_chat_zoom_rules() {
        assert_eq!(UserSettings::normalized_chat_zoom(1.0), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.004), None);
        assert_eq!(UserSettings::normalized_chat_zoom(1.125), Some(1.13));
        assert_eq!(UserSettings::normalized_chat_zoom(0.1), Some(0.75));
        assert_eq!(UserSettings::normalized_chat_zoom(4.0), Some(2.0));
    }

    /// Zoom is a slider, so it fires continuously: a change that rounds to the
    /// already-persisted value must not touch the file at all. Asserted on the
    /// file's bytes, not on the return flag.
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

    /// A legacy flat file is migrated on first load, the original is kept as a
    /// `.bak`, and the rewritten file states the hotkey contract explicitly
    /// rather than leaving it implied by defaults.
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

    /// Full alias matrix through both schema generations: every legacy spelling
    /// is accepted on read and rewritten in canonical form, while unknown values
    /// are refused rather than rounded up to a more aggressive policy — and a
    /// rejected write leaves no file behind.
    #[test]
    #[serial]
    fn formatting_policy_v1_v2_alias_matrix_normalizes_and_rejects_unknowns() {
        let cases = [
            ("off", FormattingPolicy::Off, "off"),
            ("correction", FormattingPolicy::Correction, "correction"),
            ("smart", FormattingPolicy::Smart, "smart"),
            ("max", FormattingPolicy::Max, "max"),
            ("raw", FormattingPolicy::Off, "off"),
            ("medium", FormattingPolicy::Correction, "correction"),
            ("creative", FormattingPolicy::Max, "max"),
        ];

        for (input, policy, normalized) in cases {
            assert_eq!(
                FormattingPolicy::parse(input).expect("known policy"),
                policy
            );

            let v1_dir = setup_isolated_data_dir();
            let v1_path = UserSettings::settings_path();
            fs::write(&v1_path, format!(r#"{{"formatting_level":"{input}"}}"#)).expect("write V1");
            let v1 = UserSettings::load();
            assert_eq!(v1.formatting_level.as_deref(), Some(normalized));
            let v1_json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&v1_path).expect("read migrated V1"))
                    .expect("parse migrated V1");
            assert_eq!(
                v1_json
                    .pointer("/speech/formatting/level")
                    .and_then(|v| v.as_str()),
                Some(normalized)
            );
            drop(v1_dir);

            let v2_dir = setup_isolated_data_dir();
            let v2_path = UserSettings::settings_path();
            fs::write(
                &v2_path,
                format!(
                    r#"{{"schema_version":3,"speech":{{"formatting":{{"level":"{input}"}}}}}}"#
                ),
            )
            .expect("write V2");
            let v2 = UserSettings::load();
            assert_eq!(v2.formatting_level.as_deref(), Some(normalized));
            v2.save().expect("rewrite normalized V2");
            let v2_json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&v2_path).expect("read rewritten V2"))
                    .expect("parse rewritten V2");
            assert_eq!(
                v2_json
                    .pointer("/speech/formatting/level")
                    .and_then(|v| v.as_str()),
                Some(normalized)
            );
            drop(v2_dir);
        }

        for unknown in ["", "basic", "aggressive", "SMART", "maximum"] {
            assert!(
                FormattingPolicy::parse(unknown).is_err(),
                "accepted {unknown:?}"
            );
        }

        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_string("FORMATTING_LEVEL", "aggressive");
        assert_eq!(settings.formatting_level, None);
        assert!(!UserSettings::settings_path().exists());
    }

    /// A `false` written through the setter survives reload — the case a naive
    /// `skip_serializing_if` on a plain `bool` would silently lose.
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

    /// Auto-paste round-trips through both schemas *and* its declared contract
    /// in `ENV_REGISTRY.toml` still matches (bool, hot-reloadable, default on).
    /// Reading the registry keeps code and documented contract from drifting.
    #[test]
    #[serial]
    fn auto_paste_v1_v2_roundtrips_and_registry_contract_is_promoted_hot() {
        let v1_dir = setup_isolated_data_dir();
        let v1_path = UserSettings::settings_path();
        fs::write(&v1_path, r#"{"auto_paste_enabled":false}"#).expect("write V1");
        let v1 = UserSettings::load();
        assert_eq!(v1.auto_paste_enabled, Some(false));
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&v1_path).expect("read migrated V1"))
                .expect("parse migrated V1");
        assert_eq!(
            migrated
                .pointer("/interaction/auto_paste_enabled")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        drop(v1_dir);

        let _v2_dir = setup_isolated_data_dir();
        let v2_path = UserSettings::settings_path();
        fs::write(
            &v2_path,
            r#"{"schema_version":3,"interaction":{"auto_paste_enabled":true}}"#,
        )
        .expect("write V2");
        let v2 = UserSettings::load();
        assert_eq!(v2.auto_paste_enabled, Some(true));
        v2.save().expect("round-trip V2");
        assert!(is_promoted_key("AUTO_PASTE_ENABLED"));

        let registry = include_str!("../../docs/ENV_REGISTRY.toml");
        let section = registry
            .split("[vars.AUTO_PASTE_ENABLED]")
            .nth(1)
            .and_then(|tail| tail.split("\n[vars.").next())
            .expect("AUTO_PASTE_ENABLED registry section");
        assert!(section.contains("default = \"1\""));
        assert!(section.contains("type = \"bool\""));
        assert!(section.contains("reload = \"hot\""));
    }

    /// The ghosting regression itself: these keys were settable and promoted,
    /// but the V2 conversion dropped them, so `save` → `load` reverted them to
    /// default. Runs the real on-disk path, since an in-memory check would pass.
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

    /// The STT selector keys survive the `speech.engine` round-trip, and the
    /// setters route them to the same place — so `settings.json` remains a
    /// valid seed source instead of being overwritten by env on next load.
    #[test]
    #[serial]
    fn test_stt_engine_and_layered_transcription_survive_roundtrip() {
        // F1 layered transcription: both env-managed keys must round-trip through
        // the V2 speech.engine section, or save→load silently drops the seed value.
        let _tmp = setup_isolated_data_dir();
        let settings = UserSettings {
            stt_engine: Some("apple".to_string()),
            final_pass_mode: Some("smart".to_string()),
            layered_transcription: Some("phase1".to_string()),
            stt_initial_prompt_enabled: Some(true),
            ..Default::default()
        };
        settings.save().expect("save settings");

        let loaded = UserSettings::load();
        assert_eq!(loaded.stt_engine.as_deref(), Some("apple"));
        assert_eq!(loaded.final_pass_mode.as_deref(), Some("smart"));
        assert_eq!(loaded.layered_transcription.as_deref(), Some("phase1"));
        assert_eq!(loaded.stt_initial_prompt_enabled, Some(true));

        // Setters route keys (settings.json stays a valid seed source).
        let mut mutated = loaded;
        mutated.set_string("CODESCRIBE_STT_ENGINE", "whisper");
        mutated.set_string("FINAL_PASS_MODE", "off");
        mutated.set_string("CODESCRIBE_LAYERED_TRANSCRIPTION", "off");
        mutated.set_bool("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED", false);
        let reloaded = UserSettings::load();
        assert_eq!(reloaded.stt_engine.as_deref(), Some("whisper"));
        assert_eq!(reloaded.final_pass_mode.as_deref(), Some("off"));
        assert_eq!(reloaded.layered_transcription.as_deref(), Some("off"));
        assert_eq!(reloaded.stt_initial_prompt_enabled, Some(false));
    }

    /// Layered transcription is a promoted product setting (full single-brain,
    /// same contract as `CODESCRIBE_STT_ENGINE`). Without promotion the toggle
    /// write lands in `.env` only, the stale process env wins the UI read-back,
    /// and the switch visibly snaps OFF (operator repro 2026-08-10).
    #[test]
    fn layered_transcription_is_promoted_single_brain_key() {
        assert!(
            is_promoted_key("CODESCRIBE_LAYERED_TRANSCRIPTION"),
            "CODESCRIBE_LAYERED_TRANSCRIPTION must be a promoted settings.json key"
        );
    }

    /// An empty `speech.engine: {}` must resolve to the product default, not to
    /// "unset". Unset handed the choice to the environment, so which recognizer
    /// ran depended on a stale `.env` line rather than on the product.
    #[test]
    #[serial]
    fn empty_speech_engine_defaults_to_apple_live_product() {
        // MacGyver lottery shape: schema v3 with speech.engine: {} left stt_engine
        // unset and .env=auto won. Product must pin Apple live + smart final.
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        fs::write(
            &path,
            r#"{
  "schema_version": 3,
  "speech": {
    "language": "pl",
    "engine": {}
  }
}"#,
        )
        .expect("write empty engine settings");
        let loaded = UserSettings::load();
        assert_eq!(
            loaded.stt_engine.as_deref(),
            Some("apple"),
            "empty speech.engine must pin Apple live, not leave None/auto lottery"
        );
        assert_eq!(loaded.final_pass_mode.as_deref(), Some("smart"));
        assert!(is_promoted_key("CODESCRIBE_STT_ENGINE"));
        assert!(is_promoted_key("FINAL_PASS_MODE"));
    }

    /// Tool permissions survive persistence *and* land under `agent.permissions`
    /// at the exact JSON paths the gateway reads. A permission that round-trips
    /// in memory but lands elsewhere on disk is a silently unenforced rule.
    #[test]
    #[serial]
    fn agent_permissions_roundtrip_under_agent_section() {
        use crate::agent::permissions::{AgentPermissions, PermissionLevel};
        let _tmp = setup_isolated_data_dir();
        let mut perms = AgentPermissions {
            default: PermissionLevel::Ask,
            read_only_default: PermissionLevel::Allow,
            side_effect_default: PermissionLevel::Deny,
            ..Default::default()
        };
        perms.tools.insert(
            "desktop-commander:write_file".into(),
            PermissionLevel::Allow,
        );
        perms
            .servers
            .insert("desktop-commander".into(), PermissionLevel::Ask);
        let settings = UserSettings {
            agent_permissions: Some(perms.clone()),
            ..Default::default()
        };
        settings.save().expect("save settings");

        let loaded = UserSettings::load();
        assert_eq!(loaded.agent_permissions.as_ref(), Some(&perms));

        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(
            persisted
                .pointer("/agent/permissions/tools/desktop-commander:write_file")
                .and_then(|v| v.as_str()),
            Some("allow")
        );
        assert_eq!(
            persisted
                .pointer("/agent/permissions/side_effect_default")
                .and_then(|v| v.as_str()),
            Some("deny")
        );
    }

    /// Workspace roots persist as a list under `system`, parse from the
    /// colon-joined wire form with surrounding space trimmed, and clear back to
    /// `None` when set to empty — the seed `list_projects` depends on.
    #[test]
    #[serial]
    fn test_agent_workspace_roots_survive_v2_system_roundtrip() {
        // Workspace roots must round-trip through the V2 `system` section (Vec,
        // like mode_bindings) or save→load silently drops the AGENT_WORKSPACE_ROOTS
        // seed the list_projects tool depends on.
        let _tmp = setup_isolated_data_dir();
        let settings = UserSettings {
            agent_workspace_roots: Some(vec!["~/Git".to_string(), "~/dev".to_string()]),
            ..Default::default()
        };
        settings.save().expect("save settings");

        let loaded = UserSettings::load();
        assert_eq!(
            loaded.agent_workspace_roots.as_deref(),
            Some(["~/Git".to_string(), "~/dev".to_string()].as_slice())
        );

        // Land under the V2 `system` section (not a stray top-level key).
        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read persisted settings"))
                .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("system")
                .and_then(|v| v.get("agent_workspace_roots"))
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(2)
        );

        // set_string parses the colon-joined wire form; empty clears back to None.
        let mut mutated = loaded;
        mutated.set_string("AGENT_WORKSPACE_ROOTS", "~/code : ~/work");
        let reloaded = UserSettings::load();
        assert_eq!(
            reloaded.agent_workspace_roots.as_deref(),
            Some(["~/code".to_string(), "~/work".to_string()].as_slice())
        );

        let mut cleared = reloaded;
        cleared.set_string("AGENT_WORKSPACE_ROOTS", "");
        assert_eq!(cleared.agent_workspace_roots, None);
    }

    /// Overlay toggle lands under `ui`, asserted on the JSON path rather than
    /// on the reloaded struct — a stray top-level key would still round-trip.
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

    /// Same section contract for the tray's starting lane.
    #[test]
    #[serial]
    fn test_tray_start_assistive_persists_in_v2_ui_section() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_bool("TRAY_START_ASSISTIVE", true);

        let loaded = UserSettings::load();
        assert_eq!(loaded.tray_start_assistive, Some(true));

        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read persisted settings"))
                .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("ui")
                .and_then(|v| v.get("tray_start_assistive"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    /// Daemon autostart is install-level state, so it belongs under `system`.
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

    /// Onboarding lane must survive the flat → nested → flat trip. Losing it
    /// would re-run first-run onboarding for a user who already finished it.
    #[test]
    #[serial]
    fn test_onboarding_mode_persists_in_v2_system_section() {
        // Ghosting guard (W1-C1): onboarding_mode must survive the flat ->
        // SettingsV2 -> flat round-trip and land in the V2 `system` section.
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_string("ONBOARDING_MODE", "agentic");

        let loaded = UserSettings::load();
        assert_eq!(loaded.onboarding_mode.as_deref(), Some("agentic"));

        let path = UserSettings::settings_path();
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read persisted settings"))
                .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("system")
                .and_then(|v| v.get("onboarding_mode"))
                .and_then(|v| v.as_str()),
            Some("agentic")
        );
    }

    /// The OAuth client id persists under `system`, and a blank or
    /// whitespace-only write clears it back to "awaiting registration" instead
    /// of storing an empty string that would read as a configured identity.
    #[test]
    #[serial]
    fn test_oauth_client_id_persists_in_v2_system_section_and_empty_clears() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_string("LLM_OPENAI_OAUTH_CLIENT_ID", "app_abc123");

        let loaded = UserSettings::load();
        assert_eq!(loaded.openai_oauth_client_id.as_deref(), Some("app_abc123"));

        let persisted: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read persisted settings"),
        )
        .expect("parse persisted settings");
        assert_eq!(
            persisted
                .get("system")
                .and_then(|v| v.get("openai_oauth_client_id"))
                .and_then(|v| v.as_str()),
            Some("app_abc123")
        );

        // Empty (or whitespace) clears back to the registration gate.
        let mut cleared = loaded;
        cleared.set_string("LLM_OPENAI_OAUTH_CLIENT_ID", "   ");
        assert_eq!(UserSettings::load().openai_oauth_client_id, None);
    }

    /// "Not yet chosen" is `None`, distinct from any concrete lane — callers
    /// treat it as the safe Basic path.
    #[test]
    #[serial]
    fn test_onboarding_mode_defaults_to_none_when_unset() {
        let _tmp = setup_isolated_data_dir();
        let settings = UserSettings::default();
        assert_eq!(settings.onboarding_mode, None);
    }

    /// A rebind is readable back through the same accessor the runtime uses.
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

    /// Rebinding one mode leaves the others alone — the normalization overlay
    /// must merge into the defaults, not replace them.
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

    /// Retired hotkey env vars are inert: they neither change the effective
    /// bindings nor cause a `settings.json` to be synthesized on load.
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

    /// With both present, the saved binding wins over the legacy env var — a
    /// leftover shell export cannot take the user's hotkey away.
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

    // ── C2: Layer 1 ASR mode + audio-egress consent ──

    /// The three C2 keys are promoted: writes route to settings.json, never
    /// to `.env`, and never to the Keychain.
    #[test]
    fn c2_keys_are_promoted() {
        assert!(is_promoted_key("CODESCRIBE_ASR_MODE"));
        assert!(is_promoted_key("CODESCRIBE_CLOUD_CONSENT"));
        assert!(is_promoted_key("CODESCRIBE_ASR_GATEWAY_URL"));
    }

    /// Mode, consent (with timestamp), and gateway URL survive the full disk
    /// round-trip through the V2 schema — no ghosting.
    #[test]
    #[serial]
    fn c2_fields_round_trip_through_v2_schema() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();
        settings.set_string("CODESCRIBE_ASR_MODE", "cloud");
        settings.set_string("CODESCRIBE_CLOUD_CONSENT", "granted");
        settings.set_string(
            "CODESCRIBE_ASR_GATEWAY_URL",
            "https://gateway.libraxis.cloud/v1/asr/sessions",
        );

        let loaded = UserSettings::load();
        assert_eq!(loaded.asr_mode.as_deref(), Some("cloud"));
        assert_eq!(loaded.cloud_consent.as_deref(), Some("granted"));
        assert!(
            loaded.cloud_consent_at.is_some(),
            "explicit consent answer must stamp its provenance timestamp"
        );
        assert_eq!(
            loaded.asr_gateway_url.as_deref(),
            Some("https://gateway.libraxis.cloud/v1/asr/sessions")
        );

        // On-disk placement: mode + gateway in speech.engine, consent in system.
        let raw: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(UserSettings::settings_path()).expect("read settings"),
        )
        .expect("parse settings");
        assert_eq!(
            raw.pointer("/speech/engine/asr_mode")
                .and_then(|v| v.as_str()),
            Some("cloud")
        );
        assert_eq!(
            raw.pointer("/system/cloud_audio_egress_consent")
                .and_then(|v| v.as_str()),
            Some("granted")
        );
    }

    /// Invalid mode, consent, and gateway values are rejected without touching
    /// the persisted state — a tampered write cannot arm egress.
    #[test]
    #[serial]
    fn c2_setters_reject_invalid_values() {
        let _tmp = setup_isolated_data_dir();
        let mut settings = UserSettings::default();

        settings.set_string("CODESCRIBE_ASR_MODE", "whisper_cloud");
        assert_eq!(settings.asr_mode, None, "unknown mode must be rejected");

        settings.set_string("CODESCRIBE_CLOUD_CONSENT", "yes");
        assert_eq!(
            settings.cloud_consent, None,
            "non-canonical consent must be rejected"
        );
        assert_eq!(settings.cloud_consent_at, None);

        settings.set_string(
            "CODESCRIBE_ASR_GATEWAY_URL",
            "https://user:sk-key@gateway.libraxis.cloud/mint",
        );
        assert_eq!(
            settings.asr_gateway_url, None,
            "credential-bearing URL must be rejected"
        );

        // Empty clears an existing consent record back to "never asked".
        settings.set_string("CODESCRIBE_CLOUD_CONSENT", "denied");
        assert_eq!(settings.cloud_consent.as_deref(), Some("denied"));
        settings.set_string("CODESCRIBE_CLOUD_CONSENT", "");
        assert_eq!(settings.cloud_consent, None);
        assert_eq!(settings.cloud_consent_at, None);
    }

    /// Resolution truth on the settings snapshot: fresh installs land on
    /// Apple-only, upgrades preserve the prior local/cloud choice, and cloud
    /// without a granted record refuses egress without reaching for weights.
    #[test]
    fn c2_resolved_asr_mode_covers_fresh_upgrade_and_consent_paths() {
        use crate::config::cloud_asr::{AsrProductMode, ModeDerivation};

        let fresh = UserSettings::default();
        let resolved = fresh.resolved_asr_mode();
        assert_eq!(resolved.mode, AsrProductMode::AppleOnly);
        assert_eq!(resolved.derivation, ModeDerivation::FreshDefault);

        let legacy_local = UserSettings {
            use_local_stt: Some(true),
            ..UserSettings::default()
        };
        assert_eq!(
            legacy_local.resolved_asr_mode().mode,
            AsrProductMode::LocalPower
        );

        let legacy_cloud = UserSettings {
            use_local_stt: Some(false),
            ..UserSettings::default()
        };
        let resolved = legacy_cloud.resolved_asr_mode();
        assert_eq!(resolved.mode, AsrProductMode::Cloud);
        assert_eq!(resolved.derivation, ModeDerivation::LegacyCloudChoice);

        let cloud_no_consent = UserSettings {
            asr_mode: Some("cloud".to_string()),
            ..UserSettings::default()
        };
        let resolved = cloud_no_consent.resolved_asr_mode();
        assert_eq!(resolved.mode, AsrProductMode::AppleOnly);
        assert_eq!(resolved.derivation, ModeDerivation::ConsentMissingFallback);
    }
}
