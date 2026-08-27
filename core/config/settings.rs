//! User-facing settings stored as JSON (GUI-managed).
//!
//! These are the "regular user" tier. Power users override via ~/.codescribe/.env.
//!
//! # Immutable settings part set
//!
//! Declared owners live in this file and in `loader.rs`. Session consumers
//! borrow one selected generation. No second default owner, synchronizer, or
//! mutable runtime bus may return.
//!
//! | Part | Owner symbol / surface | Role |
//! |---|---|---|
//! | Persisted intent | [`UserSettings`] | durable `settings.json` choices |
//! | Loader input | [`SettingsLoaderInput`] | one-shot load envelope for the core loader |
//! | Immutable snapshot | [`RuntimeSettingsSnapshot`] | session-frozen runtime truth |
//! | Provenance | [`SettingsSnapshotProvenance`] | source attribution for the snapshot |
//! | Digest | [`SettingsSnapshotDigest`] | integrity fingerprint for bus/session evidence |
//! | Validation | [`SettingsSnapshotValidation`] | admit/refuse contract before snapshot seal |

use super::types::{Config, ModeBinding, ShortcutBinding, WorkMode, default_mode_bindings};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::llm::provider::{ProviderKind, WireFamily};
use crate::pipeline::acoustic_ledger::EnergyCalibration;

use super::energy_calibration::{
    EnergyCalibrationRefusal, EnergyCalibrationStatus, SealedEnergyCalibration,
};

/// Serialize settings read/migrate/write transactions. A V1 load writes a
/// backup and a V3 replacement, so it is a writer even though the public API is
/// named `load`; one lock keeps concurrent migrations and saves from crossing.
fn settings_io_lock() -> MutexGuard<'static, ()> {
    static SETTINGS_IO: OnceLock<Mutex<()>> = OnceLock::new();
    SETTINGS_IO
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// Power-user override for the product-owned Silero seal lane.
///
/// The durable field lives at `audio.seal_lane_armed` in `settings.json` and
/// defaults to armed. This environment token is deliberately the exception to
/// the normal promoted-key rule: when present in `.env` or process env it still
/// wins in either direction, but Settings writes never create or rewrite it.
pub const SILERO_FUSION_ENV: &str = "CODESCRIBE_SILERO_FUSION";

/// Supported production default for the mandatory committed-utterance lane.
pub const DEFAULT_SEAL_LANE_ARMED: bool = true;

/// Source of the effective seal-lane verdict in one immutable settings generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealLaneSource {
    Settings,
    EnvOverride,
}

impl SealLaneSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::EnvOverride => "env_override",
        }
    }
}

fn default_seal_lane_armed() -> Option<bool> {
    Some(DEFAULT_SEAL_LANE_ARMED)
}

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

/// Persisted user intent (`settings.json`) — W1-A part, single owner.
///
/// # Part contract
/// - **inputs:** explicit GUI/bridge edits and validated migration writes
/// - **outputs:** sparse Option fields consumed only by the one core loader
/// - **forbidden authority:** must not resolve runtime defaults/env precedence;
///   must not act as a live mutable session snapshot; must not own PCM,
///   reducer, formatter, or delivery decisions
/// - **intended W2 consumers:** `core/config/loader.rs` (sole reader that
///   builds [`RuntimeSettingsSnapshot`]); Swift/bridge edit transport may
///   write intent but cannot mint session truth
///
/// All fields are Option — None means "use default or documented env override"
/// at loader time, never at consumer time.
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
    /// Product-owned arming of the mandatory Silero seal lane. Missing legacy
    /// values migrate to the supported production default (`true`). The
    /// `CODESCRIBE_SILERO_FUSION` power-user override is resolved only by the
    /// immutable snapshot loader and always wins when present.
    #[serde(
        default = "default_seal_lane_armed",
        skip_serializing_if = "Option::is_none"
    )]
    pub seal_lane_armed: Option<bool>,
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
    /// Seeds `CODESCRIBE_LAYERED_TRANSCRIPTION`. In Local Power, absent means
    /// the required Apple-first patcher default is armed; explicit `off` is a
    /// named degraded override. `phase1` remains a compatibility token.
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

/// One-shot loader input envelope — W1-A part, declared for the core loader.
///
/// # Part contract
/// - **inputs:** settings.json path, optional env-file permission, process-env
///   override permission (keys must already be registered in `docs/ENV_REGISTRY.toml`)
/// - **outputs:** material handed to the single loader pass that emits
///   [`RuntimeSettingsSnapshot`]
/// - **forbidden authority:** must not retain mutable runtime state; must not
///   re-read files mid-take; must not invent a second default owner or
///   synchronizer; must not select engines/routes/text
/// - **intended W2 consumers:** `core/config/loader.rs` only, once, at
///   app/session start or explicit next-session reload
///
/// W1 places the type. No loader call site is connected here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsLoaderInput {
    /// Canonical path of persisted intent (`settings.json`).
    pub settings_path: PathBuf,
    /// When true, the loader may seed missing keys from the optional `.env` file.
    pub allow_env_file: bool,
    /// When true, registered process-env overrides may win over persisted intent.
    pub allow_process_env_overrides: bool,
}

/// Source attribution for one immutable runtime settings snapshot — W1-A part.
///
/// # Part contract
/// - **inputs:** path/digest of persisted intent, names of applied env overrides
///   (never secret values), whether code defaults filled gaps, load timestamp
/// - **outputs:** provenance carried inside [`RuntimeSettingsSnapshot`]
/// - **forbidden authority:** must not reinterpret precedence; must not mutate
///   snapshot values; must not stand in for bus/session identity
/// - **intended W2 consumers:** snapshot constructors in the core loader;
///   Transcript Bus session evidence; diagnostics that display source lineage
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSnapshotProvenance {
    /// Absolute path of the `settings.json` that contributed persisted intent.
    pub settings_json_path: Option<PathBuf>,
    /// SHA-256 of the persisted intent bytes when a file was read.
    pub settings_json_sha256: Option<String>,
    /// Registered env override *names* applied during the load (values omitted).
    pub env_overlay_keys: Vec<String>,
    /// True when documented code defaults filled any absent field.
    pub defaults_applied: bool,
    /// Unix epoch milliseconds when the loader sealed this provenance.
    pub loaded_at_unix_ms: u64,
    /// Path the loader consulted for the acoustic calibration artifact.
    pub energy_calibration_path: PathBuf,
    /// SHA-256 of the sealed calibration artifact; `None` when missing/refused.
    pub energy_calibration_sha256: Option<String>,
}

/// Integrity fingerprint of one immutable runtime settings snapshot — W1-A part.
///
/// # Part contract
/// - **inputs:** canonical non-secret snapshot material selected by the loader
/// - **outputs:** opaque digest string for session evidence and reproducibility
/// - **forbidden authority:** must not carry secrets; must not decide settings
///   values; must not act as a second settings store
/// - **intended W2 consumers:** [`RuntimeSettingsSnapshot`]; Transcript Bus
///   session evidence; operator take envelopes (digest only)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SettingsSnapshotDigest(String);

impl SettingsSnapshotDigest {
    /// Wrap an already-computed digest string. W2 owns the hashing algorithm.
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// Borrow the digest text for evidence sinks.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validation contract that admits or refuses a candidate snapshot — W1-A part.
///
/// # Part contract
/// - **inputs:** resolved [`Config`] candidate plus [`SettingsSnapshotProvenance`]
/// - **outputs:** `Ok(())` admit or [`SettingsSnapshotValidationError`] refuse
/// - **forbidden authority:** must not repair by inventing defaults beyond the
///   documented loader rules; must not write `settings.json`; must not wire
///   consumers
/// - **intended W2 consumers:** the single loader path immediately before minting
///   [`RuntimeSettingsSnapshot`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsSnapshotValidation;

/// Why a candidate runtime settings snapshot was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsSnapshotValidationError {
    /// A field failed the documented sanitize/range/enum contract.
    InvalidField { field: &'static str, reason: String },
    /// Precedence inputs conflicted in a way the loader must not silently heal.
    ConflictingSources { detail: String },
}

impl std::fmt::Display for SettingsSnapshotValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidField { field, .. } => {
                write!(formatter, "runtime settings field '{field}' was refused")
            }
            Self::ConflictingSources { .. } => {
                formatter.write_str("runtime settings sources conflict")
            }
        }
    }
}

impl std::error::Error for SettingsSnapshotValidationError {}

impl SettingsSnapshotValidation {
    /// Structural admit gate. W1 declares the contract; W2 supplies full rules.
    ///
    /// The current body only refuses an empty digest placeholder so the part is
    /// executable without becoming a second default owner.
    pub fn admit(
        _values: &Config,
        _provenance: &SettingsSnapshotProvenance,
        digest: &SettingsSnapshotDigest,
    ) -> Result<(), SettingsSnapshotValidationError> {
        if digest.as_str().trim().is_empty() {
            return Err(SettingsSnapshotValidationError::InvalidField {
                field: "digest",
                reason: "settings snapshot digest must be non-empty".to_string(),
            });
        }
        Ok(())
    }
}

/// Stable identity of one resolved LLM lane inside the immutable settings throne.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeLlmLaneKind {
    Main,
    Formatting,
    Assistive,
}

impl RuntimeLlmLaneKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Formatting => "formatting",
            Self::Assistive => "assistive",
        }
    }
}

/// Credential facts sealed by the loader for one LLM lane.
///
/// The secret stays private, has no serde implementation, and is redacted from
/// `Debug`. Consumers may borrow it for a request but cannot derive a second
/// settings authority from this record.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeLlmCredential {
    key_account: String,
    api_key: Option<String>,
    account_auth: bool,
}

impl RuntimeLlmCredential {
    pub(super) fn seal(
        key_account: impl Into<String>,
        api_key: Option<String>,
        account_auth: bool,
    ) -> Self {
        Self {
            key_account: key_account.into(),
            api_key,
            account_auth,
        }
    }

    pub fn key_account(&self) -> &str {
        &self.key_account
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub const fn account_auth(&self) -> bool {
        self.account_auth
    }
}

impl fmt::Debug for RuntimeLlmCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeLlmCredential")
            .field("key_account", &self.key_account)
            .field("api_key_present", &self.api_key.is_some())
            .field("account_auth", &self.account_auth)
            .finish()
    }
}

/// One fully resolved LLM lane. No consumer may reparse settings, env, or
/// Keychain after receiving this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLlmLane {
    lane: RuntimeLlmLaneKind,
    provider: ProviderKind,
    wire_family: WireFamily,
    endpoint: String,
    model: String,
    credential: RuntimeLlmCredential,
    available: bool,
    unavailable_reason: Option<String>,
}

impl RuntimeLlmLane {
    pub(super) fn seal(
        lane: RuntimeLlmLaneKind,
        provider: ProviderKind,
        endpoint: String,
        model: String,
        credential: RuntimeLlmCredential,
        available: bool,
        unavailable_reason: Option<String>,
    ) -> Self {
        Self {
            lane,
            provider,
            wire_family: provider.wire_family(),
            endpoint,
            model,
            credential,
            available,
            unavailable_reason,
        }
    }

    pub const fn lane(&self) -> RuntimeLlmLaneKind {
        self.lane
    }

    pub const fn provider(&self) -> ProviderKind {
        self.provider
    }

    pub const fn wire_family(&self) -> WireFamily {
        self.wire_family
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn credential(&self) -> &RuntimeLlmCredential {
        &self.credential
    }

    pub const fn available(&self) -> bool {
        self.available
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
    }

    pub(super) fn digest_material(&self) -> String {
        format!(
            "{}|{}|{:?}|{}|{}|{}|{}|{}|{}",
            self.lane.as_str(),
            self.provider.as_str(),
            self.wire_family,
            self.endpoint,
            self.model,
            self.credential.key_account,
            self.credential.api_key.is_some(),
            self.credential.account_auth,
            self.available,
        )
    }
}

/// The complete LLM part set sealed exactly once by the settings loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLlmLanes {
    main: RuntimeLlmLane,
    formatting: RuntimeLlmLane,
    assistive: RuntimeLlmLane,
}

impl RuntimeLlmLanes {
    pub(super) fn seal(
        main: RuntimeLlmLane,
        formatting: RuntimeLlmLane,
        assistive: RuntimeLlmLane,
    ) -> Self {
        debug_assert_eq!(main.lane(), RuntimeLlmLaneKind::Main);
        debug_assert_eq!(formatting.lane(), RuntimeLlmLaneKind::Formatting);
        debug_assert_eq!(assistive.lane(), RuntimeLlmLaneKind::Assistive);
        Self {
            main,
            formatting,
            assistive,
        }
    }

    pub fn lane(&self, lane: RuntimeLlmLaneKind) -> &RuntimeLlmLane {
        match lane {
            RuntimeLlmLaneKind::Main => &self.main,
            RuntimeLlmLaneKind::Formatting => &self.formatting,
            RuntimeLlmLaneKind::Assistive => &self.assistive,
        }
    }

    pub fn main(&self) -> &RuntimeLlmLane {
        &self.main
    }

    pub fn formatting(&self) -> &RuntimeLlmLane {
        &self.formatting
    }

    pub fn assistive(&self) -> &RuntimeLlmLane {
        &self.assistive
    }

    pub(super) fn digest_material(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.main.digest_material(),
            self.formatting.digest_material(),
            self.assistive.digest_material(),
        )
    }
}

/// Where the content of a resolved prompt actually came from.
///
/// Runtime execution records this typed evidence rather than carrying an
/// absolute path or raw I/O error text into the selected generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSource {
    /// A non-empty operator override on disk.
    CustomFile,
    /// The compiled-in default: no file, or a file that was empty.
    BuiltInFallback,
    /// The file exists but could not be read; the default was used instead.
    ReadError,
}

impl PromptSource {
    /// Stable identifier for digest material, logs, and telemetry.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CustomFile => "custom_file",
            Self::BuiltInFallback => "built_in_fallback",
            Self::ReadError => "read_error",
        }
    }
}

/// One prompt sealed for a selected runtime generation.
///
/// Content is intentionally private and omitted from `Debug`. Consumers may
/// borrow the exact attempt-zero bytes but cannot mutate or re-resolve them.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeSealedPrompt {
    composed_content: String,
    source: PromptSource,
    composed_sha256: String,
    base_sha256: String,
    tuning_sha256: Option<String>,
}

impl RuntimeSealedPrompt {
    pub(super) fn seal(
        composed_content: String,
        source: PromptSource,
        composed_sha256: String,
        base_sha256: String,
        tuning_sha256: Option<String>,
    ) -> Self {
        Self {
            composed_content,
            source,
            composed_sha256,
            base_sha256,
            tuning_sha256,
        }
    }

    pub fn composed_content(&self) -> &str {
        &self.composed_content
    }

    pub const fn source(&self) -> PromptSource {
        self.source
    }

    pub fn composed_sha256(&self) -> &str {
        &self.composed_sha256
    }

    pub fn base_sha256(&self) -> &str {
        &self.base_sha256
    }

    pub fn tuning_sha256(&self) -> Option<&str> {
        self.tuning_sha256.as_deref()
    }

    fn digest_material(&self) -> String {
        format!(
            "source={}|composed_sha256={}|base_sha256={}|tuning_sha256={}",
            self.source.as_str(),
            self.composed_sha256,
            self.base_sha256,
            self.tuning_sha256.as_deref().unwrap_or("none"),
        )
    }
}

impl fmt::Debug for RuntimeSealedPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSealedPrompt")
            .field("source", &self.source)
            .field("composed_sha256", &self.composed_sha256)
            .field("base_sha256", &self.base_sha256)
            .field("tuning_sha256", &self.tuning_sha256)
            .finish()
    }
}

/// Formatter prompt and retry truth for one selected generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFormatterExecution {
    formatting_prompt: Option<RuntimeSealedPrompt>,
    assistive_prompt: RuntimeSealedPrompt,
    max_retries: u32,
    retry_delay: Duration,
}

impl RuntimeFormatterExecution {
    pub(super) fn seal(
        formatting_prompt: Option<RuntimeSealedPrompt>,
        assistive_prompt: RuntimeSealedPrompt,
        max_retries: u32,
        retry_delay: Duration,
    ) -> Self {
        Self {
            formatting_prompt,
            assistive_prompt,
            max_retries,
            retry_delay,
        }
    }

    pub fn formatting_prompt(&self) -> Option<&RuntimeSealedPrompt> {
        self.formatting_prompt.as_ref()
    }

    pub fn assistive_prompt(&self) -> &RuntimeSealedPrompt {
        &self.assistive_prompt
    }

    pub const fn max_retries(&self) -> u32 {
        self.max_retries
    }

    pub const fn retry_delay(&self) -> Duration {
        self.retry_delay
    }

    fn digest_material(&self) -> String {
        let formatting = self
            .formatting_prompt
            .as_ref()
            .map(RuntimeSealedPrompt::digest_material)
            .unwrap_or_else(|| "off".to_string());
        format!(
            "formatting_prompt={formatting}\nassistive_prompt={}\nmax_retries={}\nretry_delay_ms={}",
            self.assistive_prompt.digest_material(),
            self.max_retries,
            self.retry_delay.as_millis(),
        )
    }
}

/// Shared request timing for formatter and Agent providers in one generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAiRequestTiming {
    attempt_timeout: Duration,
    inter_chunk_timeout: Duration,
}

impl RuntimeAiRequestTiming {
    pub(super) const fn seal(attempt_timeout: Duration, inter_chunk_timeout: Duration) -> Self {
        Self {
            attempt_timeout,
            inter_chunk_timeout,
        }
    }

    pub const fn attempt_timeout(&self) -> Duration {
        self.attempt_timeout
    }

    pub const fn inter_chunk_timeout(&self) -> Duration {
        self.inter_chunk_timeout
    }

    fn digest_material(&self) -> String {
        format!(
            "attempt_timeout_ms={}\ninter_chunk_timeout_ms={}",
            self.attempt_timeout.as_millis(),
            self.inter_chunk_timeout.as_millis(),
        )
    }
}

/// All AI execution facts owned by one immutable runtime settings generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAiExecution {
    formatter: RuntimeFormatterExecution,
    request_timing: RuntimeAiRequestTiming,
}

impl RuntimeAiExecution {
    pub(super) fn seal(
        formatter: RuntimeFormatterExecution,
        request_timing: RuntimeAiRequestTiming,
    ) -> Self {
        Self {
            formatter,
            request_timing,
        }
    }

    pub const fn formatter(&self) -> &RuntimeFormatterExecution {
        &self.formatter
    }

    pub const fn request_timing(&self) -> &RuntimeAiRequestTiming {
        &self.request_timing
    }

    pub(super) fn digest_material(&self) -> String {
        format!(
            "{}\n{}",
            self.formatter.digest_material(),
            self.request_timing.digest_material(),
        )
    }
}

/// Immutable per-session runtime settings snapshot — the settings throne.
///
/// # Part contract
/// - **inputs:** one validated [`Config`] from the single core loader pass,
///   [`SettingsSnapshotProvenance`], and [`SettingsSnapshotDigest`]
/// - **outputs:** frozen session truth for recording, lanes, bridge projection,
///   and Swift runtime rows
/// - **forbidden authority:** must not reread `settings.json`/env; must not
///   normalize defaults independently; must not mutate during an in-flight take;
///   must not own PCM identity, document reduction, formatter authorship, or
///   delivery routing; must not resurrect deleted parallel settings resolvers or
///   mutable settings-propagation buses
/// - **intended W2 consumers:** recording controller / session start, bridge
///   projection, Swift runtime rows, STT/LLM lanes, Transcript Bus session
///   evidence (digest + provenance only at the bus)
///
/// Hot edits create a *new* snapshot for the next recording session. An
/// in-flight take keeps this value and every AI retry/request fact it selected.
#[derive(Debug, Clone)]
pub struct RuntimeSettingsSnapshot {
    /// Resolved runtime values after defaults + allowed env overlays.
    values: Config,
    /// Persisted intent captured by the same loader pass for consumers whose
    /// policy constructors still accept the public settings schema.
    user_settings: UserSettings,
    /// Resolved provider, protocol, endpoint, model, credential, and
    /// availability facts for every LLM lane.
    llm_lanes: RuntimeLlmLanes,
    /// Effective formatting policy resolved by the same loader pass. Runtime
    /// consumers may not reconstruct this from process env or persisted rows.
    formatting_policy: FormattingPolicy,
    /// Prompt, retry, and shared Agent/formatter request facts sealed by the
    /// same loader pass as the selected lanes and policy.
    ai_execution: RuntimeAiExecution,
    /// Where the values came from.
    provenance: SettingsSnapshotProvenance,
    /// Integrity fingerprint for session evidence.
    digest: SettingsSnapshotDigest,
    /// Measured acoustic calibration truth read by the same loader pass.
    /// `Missing`/`Refused` are explicit fail-closed states the admission gate
    /// names before any microphone opens; nothing here invents a floor.
    energy_calibration: SealedEnergyCalibration,
    /// Effective mandatory-lane verdict after `settings.json` plus the optional
    /// power-user env override. Consumers never re-read either source.
    seal_lane_armed: bool,
}

/// Everything one loader pass resolved, handed to [`RuntimeSettingsSnapshot::seal_loaded`]
/// as a unit so no part can be sealed from a different pass.
pub(crate) struct RuntimeSnapshotParts {
    pub(crate) values: Config,
    pub(crate) user_settings: UserSettings,
    pub(crate) llm_lanes: RuntimeLlmLanes,
    pub(crate) formatting_policy: FormattingPolicy,
    pub(crate) ai_execution: RuntimeAiExecution,
    pub(crate) provenance: SettingsSnapshotProvenance,
    pub(crate) digest: SettingsSnapshotDigest,
    pub(crate) energy_calibration: SealedEnergyCalibration,
    pub(crate) seal_lane_armed: bool,
}

impl RuntimeSettingsSnapshot {
    /// Seal values plus persisted intent, AI execution facts, and measured
    /// acoustic calibration truth in the sole settings-loader writer.
    pub(crate) fn seal_loaded(
        parts: RuntimeSnapshotParts,
    ) -> Result<Self, SettingsSnapshotValidationError> {
        let RuntimeSnapshotParts {
            values,
            user_settings,
            llm_lanes,
            formatting_policy,
            ai_execution,
            provenance,
            digest,
            energy_calibration,
            seal_lane_armed,
        } = parts;
        SettingsSnapshotValidation::admit(&values, &provenance, &digest)?;
        Ok(Self {
            values,
            user_settings,
            llm_lanes,
            formatting_policy,
            ai_execution,
            provenance,
            digest,
            energy_calibration,
            seal_lane_armed,
        })
    }

    /// Borrow the frozen runtime values.
    pub fn values(&self) -> &Config {
        &self.values
    }

    /// Borrow persisted intent frozen beside the effective values.
    pub fn user_settings(&self) -> &UserSettings {
        &self.user_settings
    }

    /// Borrow all resolved LLM lanes from the immutable settings throne.
    pub fn llm_lanes(&self) -> &RuntimeLlmLanes {
        &self.llm_lanes
    }

    /// Effective per-take formatter policy from the immutable settings throne.
    pub const fn formatting_policy(&self) -> FormattingPolicy {
        self.formatting_policy
    }

    /// Borrow the AI execution facts sealed for this exact generation.
    pub const fn ai_execution(&self) -> &RuntimeAiExecution {
        &self.ai_execution
    }

    /// Borrow provenance for evidence sinks.
    pub fn provenance(&self) -> &SettingsSnapshotProvenance {
        &self.provenance
    }

    /// Borrow the digest for Transcript Bus / take envelopes.
    pub fn digest(&self) -> &SettingsSnapshotDigest {
        &self.digest
    }

    /// Loader verdict on the acoustic calibration artifact (sealed / missing /
    /// refused) for status surfaces and the admission gate.
    pub fn energy_calibration_status(&self) -> &EnergyCalibrationStatus {
        self.energy_calibration.status()
    }

    /// The sealed calibration truth (artifact + status) for this generation.
    pub fn energy_calibration(&self) -> &SealedEnergyCalibration {
        &self.energy_calibration
    }

    /// Effective mandatory-lane truth selected by this immutable generation.
    pub const fn seal_lane_armed(&self) -> bool {
        self.seal_lane_armed
    }

    /// Product setting behind the effective value, before an env override.
    pub fn seal_lane_setting_armed(&self) -> bool {
        self.user_settings
            .seal_lane_armed
            .unwrap_or(DEFAULT_SEAL_LANE_ARMED)
    }

    /// Whether the effective value came from Settings or the power-user env
    /// override. Provenance records names only; no env value is exposed.
    pub fn seal_lane_source(&self) -> SealLaneSource {
        if self
            .provenance
            .env_overlay_keys
            .iter()
            .any(|key| key == SILERO_FUSION_ENV)
        {
            SealLaneSource::EnvOverride
        } else {
            SealLaneSource::Settings
        }
    }

    /// The ledger calibration for one live capture path at its actual rate,
    /// or the precise refusal. This is the only way a session obtains an
    /// [`EnergyCalibration`]; there is no default.
    pub fn energy_calibration_for_capture(
        &self,
        device_name: &str,
        sample_rate: u32,
    ) -> Result<EnergyCalibration, EnergyCalibrationRefusal> {
        self.energy_calibration
            .for_capture(device_name, sample_rate)
    }
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
/// `stt_engine` is the newer product selector that supersedes it. Both are
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

/// Normalize the only accepted local STT engine settings.
///
/// `candle` is the low-level spelling of the user-facing `whisper` route.
/// Retired or unknown selectors are rejected rather than kept as dormant
/// compatibility values that a future router could accidentally revive.
pub(crate) fn normalize_stt_engine(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some("auto".to_string()),
        "apple" => Some("apple".to_string()),
        "whisper" | "candle" => Some("whisper".to_string()),
        _ => None,
    }
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
    #[serde(
        default = "default_seal_lane_armed",
        skip_serializing_if = "Option::is_none"
    )]
    seal_lane_armed: Option<bool>,
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
    SILERO_FUSION_ENV,
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
                seal_lane_armed: self.seal_lane_armed,
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
            seal_lane_armed: v2
                .audio
                .as_ref()
                .and_then(|audio| audio.seal_lane_armed)
                .or(Some(DEFAULT_SEAL_LANE_ARMED)),
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
                .and_then(|e| e.stt_engine.as_deref())
                .and_then(normalize_stt_engine)
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
    fn write_json_atomic(path: &Path, json: &str) -> anyhow::Result<()> {
        Self::write_json_atomic_with(path, json, |from, to| fs::rename(from, to))
    }

    /// Atomic settings write with the final rename injected for deterministic
    /// failure tests. Production always passes `fs::rename`; tests never depend
    /// on guessing the unique temp filename.
    fn write_json_atomic_with<F>(path: &Path, json: &str, rename: F) -> anyhow::Result<()>
    where
        F: FnOnce(&Path, &Path) -> std::io::Result<()>,
    {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("settings path has no parent: {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.json");
        let tmp = parent.join(format!(
            ".{filename}.tmp.{}.{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let outcome = (|| -> anyhow::Result<()> {
            let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            rename(&tmp, path)?;
            // `parent` is derived only from the canonical internal settings
            // path above; opening it read-only is the durability fsync, not a
            // request-controlled file lookup.
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        outcome
    }

    /// Returns the settings directory.
    ///
    /// Respects `CODESCRIBE_DATA_DIR` for test isolation; otherwise uses
    /// `~/Library/Application Support/Codescribe/`.
    pub fn settings_dir() -> PathBuf {
        if let Ok(test_dir) = std::env::var("CODESCRIBE_DATA_DIR") {
            PathBuf::from(test_dir)
        } else {
            BaseDirs::new()
                .map(|b| b.data_dir().join("Codescribe"))
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    PathBuf::from(home).join("Library/Application Support/Codescribe")
                })
        }
    }

    /// Returns the path to `settings.json`.
    pub fn settings_path() -> PathBuf {
        Self::settings_dir().join("settings.json")
    }

    /// Loads settings from disk. Returns `Default` on any error.
    pub fn load() -> Self {
        let _data_io = match super::storage_reset::begin_app_data_io() {
            Ok(guard) => guard,
            Err(error) => {
                warn!(%error, "Settings load skipped while app-data reset owns the process");
                return Self::default();
            }
        };
        let _settings_io = settings_io_lock();
        Self::load_unlocked()
    }

    /// Load while the settings transaction lock and app-data admission are held.
    fn load_unlocked() -> Self {
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
                                if let Err(e) = v1.save_unlocked() {
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
        let _data_io = super::storage_reset::begin_app_data_io()?;
        let _settings_io = settings_io_lock();
        self.save_unlocked()
    }

    /// Remove only Agent-owned fields from the persisted JSON document.
    ///
    /// This intentionally edits the raw JSON value instead of doing a
    /// `load()` -> `save()` round-trip. `load()` is fail-soft and returns
    /// defaults for malformed input; using it in a destructive reset could
    /// therefore replace an unreadable settings file with defaults and erase
    /// unrelated user choices. Unknown fields and every non-Agent subtree are
    /// preserved value-for-value. A malformed document is left untouched.
    pub fn remove_agent_owned_state() -> anyhow::Result<()> {
        let _data_io = super::storage_reset::begin_app_data_io()?;
        let _settings_io = settings_io_lock();
        let path = Self::settings_path();
        if !path.exists() {
            return Ok(());
        }

        let contents = fs::read_to_string(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&contents)?;
        let is_v2 = value.get("schema_version").is_some();

        if is_v2 {
            let before: SettingsV2 = serde_json::from_value(value.clone())?;
            Self::validate_v2(&before)?;
        } else {
            let _: Self = serde_json::from_value(value.clone())?;
        }

        let mut changed = false;
        if is_v2 {
            changed |= remove_json_keys_at(
                &mut value,
                &["speech", "assistive"],
                &["llm_endpoint", "llm_model", "provider"],
            )?;
            changed |= remove_json_keys_at(
                &mut value,
                &["system"],
                &[
                    "agent_workspace_roots",
                    "openai_oauth_client_id",
                    "anthropic_oauth_client_id",
                    "xai_oauth_client_id",
                ],
            )?;
            changed |=
                remove_json_keys_at(&mut value, &["agent"], &["permissions", "capabilities"])?;
            changed |= remove_json_keys_at(&mut value, &["interaction"], &["agent_enter_sends"])?;

            let after: SettingsV2 = serde_json::from_value(value.clone())?;
            Self::validate_v2(&after)?;
        } else {
            changed |= remove_json_keys_at(
                &mut value,
                &[],
                &[
                    "llm_assistive_endpoint",
                    "llm_assistive_model",
                    "llm_assistive_provider",
                    "openai_oauth_client_id",
                    "anthropic_oauth_client_id",
                    "xai_oauth_client_id",
                    "agent_workspace_roots",
                    "agent_permissions",
                    "agent_capabilities",
                    "agent_enter_sends",
                ],
            )?;
            let _: Self = serde_json::from_value(value.clone())?;
        }

        if !changed {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&value)?;
        Self::write_json_atomic(&path, &json)
    }

    /// Persist while the settings transaction lock and app-data admission are held.
    fn save_unlocked(&self) -> anyhow::Result<()> {
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
            "CODESCRIBE_STT_ENGINE" => match normalize_stt_engine(value) {
                Some(normalized) => self.stt_engine = Some(normalized),
                None => {
                    warn!(
                        "Rejected STT engine write (expected auto|apple|whisper|candle): {value}"
                    );
                    return;
                }
            },
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
            SILERO_FUSION_ENV => self.seal_lane_armed = Some(value),
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

/// Remove named keys from an existing JSON object reached by `path`. Missing
/// sections are a no-op; a present non-object is an error so reset never
/// normalizes malformed state by destroying sibling settings.
fn remove_json_keys_at(
    value: &mut serde_json::Value,
    path: &[&str],
    keys: &[&str],
) -> anyhow::Result<bool> {
    let mut current = value;
    for component in path {
        let Some(next) = current.get_mut(*component) else {
            return Ok(false);
        };
        current = next;
    }
    let object = current.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "settings path {} must be an object",
            if path.is_empty() {
                "<root>".to_string()
            } else {
                path.join(".")
            }
        )
    })?;
    let mut changed = false;
    for key in keys {
        changed |= object.remove(*key).is_some();
    }
    Ok(changed)
}

/// Persistence is exercised against real files in a temp data dir, not against
/// in-memory conversions — the failures these guard against (ghosted fields,
/// migration loss, write amplification) only appear on the round-trip through
/// disk. All tests are `#[serial]`: the data dir is selected by process env.
#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SEAL_LANE_ARMED, FormattingPolicy, SILERO_FUSION_ENV, UserSettings, is_promoted_key,
    };
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

    /// Legacy schema-v3 files have no seal-lane field. Loading them performs
    /// the semantic migration to the armed product default, and the next
    /// canonical write materializes that value under `audio`.
    #[test]
    #[serial]
    fn seal_lane_field_migrates_and_round_trips_through_v2_schema() {
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        fs::write(&path, r#"{"schema_version":3,"audio":{}}"#)
            .expect("write legacy schema-v3 settings");

        let mut loaded = UserSettings::load();
        assert_eq!(loaded.seal_lane_armed, Some(DEFAULT_SEAL_LANE_ARMED));
        assert!(is_promoted_key(SILERO_FUSION_ENV));

        loaded.save().expect("materialize migrated seal-lane field");
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read migrated settings"))
                .expect("parse migrated settings");
        assert_eq!(
            migrated
                .pointer("/audio/seal_lane_armed")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );

        loaded.set_bool(SILERO_FUSION_ENV, false);
        let reloaded = UserSettings::load();
        assert_eq!(reloaded.seal_lane_armed, Some(false));
    }

    #[test]
    #[serial]
    fn agent_reset_removes_only_owned_json_fields_and_preserves_unknowns() {
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        let seeded = serde_json::json!({
            "schema_version": 3,
            "interaction": {
                "agent_enter_sends": true,
                "auto_paste_enabled": false,
                "future_interaction": "keep"
            },
            "speech": {
                "language": "pl",
                "assistive": {
                    "llm_endpoint": "https://agent.example",
                    "llm_model": "agent-model",
                    "provider": "openai-responses"
                },
                "formatting": { "level": "smart" },
                "future_speech": { "keep": true }
            },
            "system": {
                "agent_workspace_roots": ["/tmp/project"],
                "openai_oauth_client_id": "openai-client",
                "anthropic_oauth_client_id": "anthropic-client",
                "xai_oauth_client_id": "xai-client",
                "onboarding_mode": "basic",
                "future_system": 42
            },
            "agent": {
                "permissions": null,
                "capabilities": null,
                "future_agent": "keep"
            },
            "future_top_level": { "keep": "exactly" }
        });
        fs::write(
            &path,
            serde_json::to_string_pretty(&seeded).expect("serialize fixture"),
        )
        .expect("seed settings");

        UserSettings::remove_agent_owned_state().expect("surgical Agent settings reset");

        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read reset settings"))
                .expect("parse reset settings");
        for pointer in [
            "/interaction/agent_enter_sends",
            "/speech/assistive/llm_endpoint",
            "/speech/assistive/llm_model",
            "/speech/assistive/provider",
            "/system/agent_workspace_roots",
            "/system/openai_oauth_client_id",
            "/system/anthropic_oauth_client_id",
            "/system/xai_oauth_client_id",
            "/agent/permissions",
            "/agent/capabilities",
        ] {
            assert!(
                after.pointer(pointer).is_none(),
                "Agent field survived: {pointer}"
            );
        }
        for pointer in [
            "/interaction/auto_paste_enabled",
            "/interaction/future_interaction",
            "/speech/language",
            "/speech/formatting",
            "/speech/future_speech",
            "/system/onboarding_mode",
            "/system/future_system",
            "/agent/future_agent",
            "/future_top_level",
        ] {
            assert_eq!(
                after.pointer(pointer),
                seeded.pointer(pointer),
                "non-Agent field changed: {pointer}"
            );
        }
    }

    #[test]
    #[serial]
    fn agent_reset_refuses_malformed_settings_without_rewriting_bytes() {
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        let malformed = b"{ this is not valid settings JSON";
        fs::write(&path, malformed).expect("seed malformed settings");

        UserSettings::remove_agent_owned_state().expect_err("malformed settings must fail closed");

        assert_eq!(
            fs::read(&path).expect("read malformed settings after refusal"),
            malformed,
            "Agent reset rewrote malformed settings"
        );
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

    /// A failed final rename removes its unique temp and leaves the last
    /// committed settings bytes untouched. This is the fault-injection seam for
    /// atomic persistence; blocking a historical fixed temp name proves nothing
    /// now that every transaction owns a UUID path.
    #[test]
    #[serial]
    fn atomic_settings_rename_failure_preserves_committed_truth_and_cleans_temp() {
        let _tmp = setup_isolated_data_dir();
        let path = UserSettings::settings_path();
        let original = UserSettings {
            auto_paste_enabled: Some(false),
            ..Default::default()
        };
        original.save().expect("seed committed settings");
        let before = fs::read(&path).expect("read committed settings");

        let replacement = UserSettings {
            auto_paste_enabled: Some(true),
            ..original
        };
        let json = serde_json::to_string_pretty(&replacement.to_v2())
            .expect("serialize replacement settings");
        let error = UserSettings::write_json_atomic_with(&path, &json, |_from, _to| {
            Err(std::io::Error::other("forced settings rename failure"))
        })
        .expect_err("forced rename must fail");
        assert!(error.to_string().contains("forced settings rename failure"));
        assert_eq!(
            fs::read(&path).expect("read settings after failed rename"),
            before
        );
        let orphan_temps: Vec<_> = fs::read_dir(UserSettings::settings_dir())
            .expect("read settings directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".settings.json.tmp.")
            })
            .collect();
        assert!(orphan_temps.is_empty(), "failed write leaked a unique temp");
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
