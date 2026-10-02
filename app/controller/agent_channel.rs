//! Fn+digit agent channel.
//!
//! The channel is not a dictation take and not a `State` variant. It subscribes
//! to the shared capture, seals on a repeated digit, on its own utterance
//! silence, or after `CODESCRIBE_CHANNEL_AUTOSEAL_SECS` without new channel
//! text, and stamps `audience` on Bus rows. A session a previous process left
//! open is sealed as an orphan when the controller starts. Paste and the
//! overlay document stay off. The hold badge is the preview; `ChannelHudState`
//! is the open-mic fact the overlay paints from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
use codescribe_core::config::Config;
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use codescribe_core::pipeline::contracts::EventSink;
use serde::Deserialize;
use tokio::sync::Mutex as TokioMutex;

use crate::audio::streaming_recorder::CaptureSubscriberId;
use crate::os::hold_badge::{self, BadgeMode};
use crate::presentation::{PresentationEmitter, TranscriptBus, TranscriptMode, TranscriptSession};
use crate::stt::active_names;

use super::RecordingController;

/// Schema id and filename. The file lives in the agent-bridge home, next to
/// the leases followers already read.
pub const BINDING_SCHEMA: &str = "vc.agent-audience-binding.v1";
pub const BINDING_FILENAME: &str = "vc.agent-audience-binding.v1.json";

/// Broadcast marker for Fn+0. Followers treat it as addressed to them.
pub const BROADCAST_AUDIENCE: &str = "*";

/// Why a channel refused to open. The display string is the loud reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOpenRefusal {
    InvalidDigit,
    BindingMissing { path: String },
    BindingMalformed { path: String, detail: String },
    DigitUnbound { digit: u8 },
}

impl std::fmt::Display for ChannelOpenRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDigit => write!(f, "agent channel digit must be 0-9"),
            Self::BindingMissing { path } => write!(
                f,
                "agent channel refused: audience binding file is missing at {path}"
            ),
            Self::BindingMalformed { path, detail } => write!(
                f,
                "agent channel refused: audience binding file at {path} is malformed: {detail}"
            ),
            Self::DigitUnbound { digit } => write!(
                f,
                "agent channel refused: digit {digit} has no agent session in the audience binding"
            ),
        }
    }
}

impl std::error::Error for ChannelOpenRefusal {}

/// Preview contract for every channel. Not a take's paste or overlay.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelPreview {
    pub autopaste: bool,
    pub overlay_document: bool,
    pub hold_badge: bool,
}

#[cfg(test)]
pub const CHANNEL_PREVIEW: ChannelPreview = ChannelPreview {
    autopaste: false,
    overlay_document: false,
    hold_badge: true,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundAgentSession {
    pub audience: String,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
    /// Dedicated channel bus from the binding (W5). `None` keeps the shared
    /// bus, so pre-W5 bindings migrate without a rewrite.
    pub bus: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BindingFile {
    schema: String,
    #[serde(default)]
    bindings: BTreeMap<String, BindingEntry>,
}

#[derive(Debug, Deserialize)]
struct BindingEntry {
    audience: String,
    provider: String,
    provider_session_id: String,
    /// Dedicated channel bus path written by the attach engine (W5).
    #[serde(default)]
    bus: Option<String>,
}

impl BindingEntry {
    fn bus(&self) -> Option<PathBuf> {
        self.bus
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenAgentChannel {
    pub subscriber: CaptureSubscriberId,
    pub audience: String,
    pub silence_sec: f32,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
    pub opened_at: SystemTime,
    pub last_voice_at: Arc<StdMutex<SystemTime>>,
    pub session_id: Option<String>,
    /// How this channel was opened; a silence-sealed channel reopens the same way.
    pub mode: ChannelOpenMode,
    /// Dedicated channel bus; `None` writes receipts to the shared bus.
    pub bus: Option<PathBuf>,
    /// Last non-empty projection, so the archived take carries its words.
    pub last_text: Arc<StdMutex<String>>,
    pub refinement_warnings: Arc<StdMutex<Vec<String>>>,
    /// The same bus session used by the reducer, including its fanout and
    /// per-destination persistence state, retained through the closing receipt.
    pub transcript_bus: Option<Arc<TranscriptBus>>,
}

/// Open-channel fact for the overlay. W2 exposes it; the overlay paint is separate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelHudState {
    pub open: bool,
    pub loud: bool,
    pub channel: String,
    pub audience: String,
    pub label: String,
    /// `0` means the silence cap is off.
    pub autoseal_secs: u64,
    pub autoseal_deadline: Option<SystemTime>,
    pub tts_ducking: bool,
    pub opened_at: SystemTime,
    /// Utterance silence configured for this channel, in milliseconds.
    pub utterance_silence_ms: u32,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
    /// Whether the bound provider session has a live bridge follower
    /// (fresh lease heartbeat). `None` = the snapshot made no liveness claim.
    pub follower_alive: Option<bool>,
}

fn utterance_silence_ms(seconds: f32) -> u32 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    let millis = (f64::from(seconds) * 1000.0).round();
    if millis >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        millis as u32
    }
}

pub const CHANNEL_AUTOSEAL_SECS_ENV: &str = "CODESCRIBE_CHANNEL_AUTOSEAL_SECS";
/// Hands-free delivery boundary (Founder decision, voice seal cc6c8248,
/// 2026-09-29): five seconds of silence seal the utterance and deliver it,
/// and the channel reopens immediately — silence is a delivery boundary,
/// never a hang-up.
pub const CHANNEL_AUTOSEAL_SECS_DEFAULT: u64 = 5;

pub fn channel_open_label(digit: u8) -> String {
    format!("CHANNEL {digit} OPEN — mic is live")
}

/// Silence threshold that seals and delivers the current channel utterance.
///
/// Unset or unreadable values use [`CHANNEL_AUTOSEAL_SECS_DEFAULT`]. `0`
/// disables the threshold: a zero value would seal on the opening tick.
pub fn channel_autoseal_secs() -> u64 {
    match std::env::var(CHANNEL_AUTOSEAL_SECS_ENV) {
        Ok(value) => value
            .trim()
            .parse()
            .unwrap_or(CHANNEL_AUTOSEAL_SECS_DEFAULT),
        Err(_) => CHANNEL_AUTOSEAL_SECS_DEFAULT,
    }
}

fn silence_is_due(last_voice: SystemTime, now: SystemTime, secs: u64) -> bool {
    if secs == 0 {
        return false;
    }
    now.duration_since(last_voice).unwrap_or_default() >= Duration::from_secs(secs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelOpenMode {
    /// Acquire, start the device when idle, and run the channel session.
    Live,
    /// Register the subscriber and the open-channel record. No device start.
    /// Test seam: exercised by this module's tests; prod opens `Live`.
    #[allow(dead_code)]
    AttachedOnly,
}

/// Why a channel session ended: the `reason` of its one `sealed` receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelSealReason {
    /// Utterance silence delivered the words; the quiet contract reopens
    /// the channel on the same binding.
    Silence,
    /// Fn+digit pressed again: the Founder hung up and nothing reopens it.
    Hangup,
    /// The process that opened the session ended without closing it (quit,
    /// crash, or a build that wrote no hang-up row). The next controller
    /// start seals it; no microphone of this process ever belonged to it.
    Orphan,
}

impl ChannelSealReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Silence => "silence",
            Self::Hangup => "hangup",
            Self::Orphan => "orphan",
        }
    }
}

/// The one writer of a channel session's `sealed` row, for every reason.
/// `open` is the session as its `open` row stated it; the seal repeats that
/// identity — channel, agent, `session_id`, `opened_at`, provider — because
/// followers pair the two rows on it (the overlay projection closes a
/// channel only when the seal's `opened_at` equals the open session's).
fn append_seal_receipt(
    bus: &Path,
    reason: ChannelSealReason,
    open: &crate::presentation::agent_ack::ChannelSessionLine<'_>,
    refinement_warnings: &[String],
) -> std::io::Result<()> {
    crate::presentation::agent_ack::append_json_line(
        bus,
        &seal_receipt_line(reason, open, refinement_warnings),
    )
}

fn seal_receipt_line(
    reason: ChannelSealReason,
    open: &crate::presentation::agent_ack::ChannelSessionLine<'_>,
    refinement_warnings: &[String],
) -> serde_json::Value {
    let mut line = crate::presentation::agent_ack::channel_session_line(
        &crate::presentation::agent_ack::ChannelSessionLine {
            state: "sealed",
            reason: reason.as_str(),
            ..*open
        },
    );
    line["raw_processing_status"] = serde_json::json!(if reason == ChannelSealReason::Orphan {
        "not_applicable"
    } else if refinement_warnings.is_empty() {
        "settled"
    } else if refinement_warnings
        .iter()
        .all(|code| code == "agent_raw_whisper_disabled")
    {
        "whisper_disabled"
    } else {
        "refinement_incomplete"
    });
    line["refinement_warnings"] = serde_json::json!(refinement_warnings);
    line
}

/// Bytes of each bus tail the orphan reconciliation reads: the budget of one
/// ack-scan pass. The shared bus runs to tens of GB and a full rescan once
/// took the machine's RAM; an orphan's `open` row is followed only by what
/// its own process wrote before it ended (on the Founder's bus, 64 MiB is
/// about twelve hours of dictation).
const ORPHAN_SCAN_WINDOW_BYTES: u64 = 64 << 20;

/// One `sealed` row the startup reconciliation appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanSeal {
    pub bus: PathBuf,
    pub channel: String,
    pub session_id: String,
}

/// The newest `channel-session` row of each channel within the last `window`
/// bytes of `bus`. Streams the tail one row at a time, so memory holds a
/// row, never the file; only rows naming the schema are parsed. A missing
/// bus has no rows.
fn latest_channel_rows(
    bus: &Path,
    window: u64,
) -> std::io::Result<BTreeMap<String, serde_json::Value>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let schema = crate::presentation::agent_ack::CHANNEL_SESSION_SCHEMA;
    let file = match std::fs::File::open(bus) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(error),
    };
    let start = file.metadata()?.len().saturating_sub(window);
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    if start > 0 {
        // Start on a row boundary: from the byte before the window, drop
        // everything through the first newline — only the row the window
        // cut in half, or nothing when the window begins exactly on a row.
        reader.seek(SeekFrom::Start(start - 1))?;
        reader.read_until(b'\n', &mut line)?;
    }
    let mut latest = BTreeMap::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            continue;
        };
        if !text.contains(schema) {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        if row.get("schema").and_then(serde_json::Value::as_str) != Some(schema) {
            continue;
        }
        if let Some(channel) = row.get("channel").and_then(serde_json::Value::as_str) {
            latest.insert(channel.to_string(), row);
        }
    }
    Ok(latest)
}

/// An `open` row as the session line its seal repeats, or `None` when the
/// row is not an open session, names no session, or belongs to `live`.
fn orphan_open_line<'a>(
    row: &'a serde_json::Value,
    live: &BTreeSet<String>,
) -> Option<crate::presentation::agent_ack::ChannelSessionLine<'a>> {
    use serde_json::Value;

    if row.get("state").and_then(Value::as_str) != Some("open") {
        return None;
    }
    let session_id = row.get("session_id").and_then(Value::as_str)?;
    if live.contains(session_id) {
        return None;
    }
    let opened_at =
        chrono::DateTime::parse_from_rfc3339(row.get("opened_at").and_then(Value::as_str)?).ok()?;
    Some(crate::presentation::agent_ack::ChannelSessionLine {
        state: "open",
        reason: "opened",
        channel: row.get("channel").and_then(Value::as_str)?,
        agent: row.get("agent").and_then(Value::as_str)?,
        session_id: Some(session_id),
        autoseal_secs: row
            .get("autoseal_secs")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        opened_at: SystemTime::from(opened_at),
        utterance_silence_sec: row
            .get("utterance_silence_sec")
            .and_then(Value::as_f64)
            .unwrap_or_default() as f32,
        provider: row.get("provider").and_then(Value::as_str),
        provider_session_id: row.get("provider_session_id").and_then(Value::as_str),
    })
}

/// Seals with `reason: orphan` every session whose `open` row is still the
/// newest `channel-session` row of its channel on its bus, unless this
/// process owns it (`live`). The seal lands on that same bus. A session
/// already followed by a newer row of its channel was ended for every
/// follower by that row — a seal, or the successor's `open` — and gets
/// nothing. Idempotent: the seal becomes the channel's newest row, so the
/// next start finds nothing open.
fn seal_orphaned_sessions(
    buses: &[PathBuf],
    live: &BTreeSet<String>,
    window: u64,
) -> Vec<OrphanSeal> {
    let mut sealed = Vec::new();
    for bus in buses {
        let latest = match latest_channel_rows(bus, window) {
            Ok(latest) => latest,
            Err(error) => {
                tracing::warn!(%error, bus = %bus.display(), "orphan channel scan could not read the bus");
                continue;
            }
        };
        for row in latest.values() {
            let Some(open) = orphan_open_line(row, live) else {
                continue;
            };
            match append_seal_receipt(bus, ChannelSealReason::Orphan, &open, &[]) {
                Ok(()) => sealed.push(OrphanSeal {
                    bus: bus.clone(),
                    channel: open.channel.to_string(),
                    session_id: open.session_id.unwrap_or_default().to_string(),
                }),
                Err(error) => tracing::warn!(
                    %error,
                    bus = %bus.display(),
                    channel = open.channel,
                    "orphan channel seal was not appended"
                ),
            }
        }
    }
    sealed
}

pub fn binding_path() -> PathBuf {
    active_names::bridge_home().join(BINDING_FILENAME)
}

pub fn load_binding(path: &Path) -> Result<BindingFile, ChannelOpenRefusal> {
    let path_string = path.display().to_string();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ChannelOpenRefusal::BindingMissing { path: path_string });
        }
        Err(error) => {
            return Err(ChannelOpenRefusal::BindingMalformed {
                path: path_string,
                detail: error.to_string(),
            });
        }
    };
    let file: BindingFile =
        serde_json::from_str(&text).map_err(|error| ChannelOpenRefusal::BindingMalformed {
            path: path_string.clone(),
            detail: error.to_string(),
        })?;
    if file.schema != BINDING_SCHEMA {
        return Err(ChannelOpenRefusal::BindingMalformed {
            path: path_string,
            detail: format!("schema must be {BINDING_SCHEMA}, found {}", file.schema),
        });
    }
    Ok(file)
}

pub fn resolve_digit(digit: u8, path: &Path) -> Result<BoundAgentSession, ChannelOpenRefusal> {
    if digit == 0 {
        return Ok(BoundAgentSession {
            audience: BROADCAST_AUDIENCE.to_string(),
            provider: None,
            provider_session_id: None,
            bus: None,
        });
    }
    if !(1..=9).contains(&digit) {
        return Err(ChannelOpenRefusal::InvalidDigit);
    }
    let file = load_binding(path)?;
    let Some(entry) = file.bindings.get(&digit.to_string()) else {
        return Err(ChannelOpenRefusal::DigitUnbound { digit });
    };
    let audience = entry.audience.trim();
    let provider = entry.provider.trim();
    let provider_session_id = entry.provider_session_id.trim();
    if audience.is_empty() || provider.is_empty() || provider_session_id.is_empty() {
        return Err(ChannelOpenRefusal::BindingMalformed {
            path: path.display().to_string(),
            detail: format!("digit {digit} must name audience, provider, and provider_session_id"),
        });
    }
    Ok(BoundAgentSession {
        audience: audience.to_string(),
        provider: Some(provider.to_string()),
        provider_session_id: Some(provider_session_id.to_string()),
        bus: entry.bus(),
    })
}

fn refusal(error: ChannelOpenRefusal) -> anyhow::Error {
    anyhow!("{error}")
}

impl RecordingController {
    /// Fn+digit toggle. A second press of the same digit hangs up: the
    /// session seals with `reason: hangup` and does not reopen. Dictation
    /// state is not changed.
    pub async fn toggle_agent_channel(&self, digit: u8) -> Result<()> {
        self.dispatch_agent_channel(
            digit,
            &binding_path(),
            &crate::presentation::transcript_bus::transcript_bus_path(),
            ChannelOpenMode::Live,
        )
        .await
    }

    /// `shared_bus` carries the rows and receipts of a channel whose binding
    /// names no dedicated bus.
    pub(crate) async fn dispatch_agent_channel(
        &self,
        digit: u8,
        binding_file: &Path,
        shared_bus: &Path,
        mode: ChannelOpenMode,
    ) -> Result<()> {
        let mut channels = self.agent_channels.lock().await;
        if let Some(open) = channels.remove(&digit) {
            drop(channels);
            self.close_open_channel(
                digit,
                open,
                ChannelSealReason::Hangup,
                shared_bus,
                channel_autoseal_secs(),
            )
            .await?;
            return Ok(());
        }

        let bound = resolve_digit(digit, binding_file).map_err(refusal)?;
        // Fn+0 always includes the shared bus. Freeze the binding's distinct
        // dedicated paths for this take so hang-up closes the same destinations.
        let mut broadcast_buses = Vec::new();
        if digit == 0 {
            match load_binding(binding_file) {
                Ok(binding) => {
                    for bus in binding.bindings.values().filter_map(BindingEntry::bus) {
                        if bus != shared_bus && !broadcast_buses.contains(&bus) {
                            broadcast_buses.push(bus);
                        }
                    }
                }
                Err(ChannelOpenRefusal::BindingMissing { .. }) => {}
                Err(error) => {
                    tracing::warn!(%error, "broadcast could not read dedicated destinations; shared bus remains active")
                }
            }
        }
        let runtime_settings = self.runtime_settings_arc().await;
        let silence_sec = runtime_settings.values().toggle_silence_sec;
        let opened_at = SystemTime::now();
        let last_voice_at = Arc::new(StdMutex::new(opened_at));
        let last_text = Arc::new(StdMutex::new(String::new()));
        let refinement_warnings = Arc::new(StdMutex::new(Vec::new()));
        let mut session_id = None;
        let mut transcript_bus = None;
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = recorder_guard.as_mut().ok_or_else(|| {
            anyhow!("agent channel refused: recording controller has no recorder")
        })?;

        let subscriber = match mode {
            ChannelOpenMode::AttachedOnly => recorder.register_channel_feed().id,
            ChannelOpenMode::Live => {
                let session_label = format!("agent-channel-{digit}-{}", uuid::Uuid::new_v4());
                super::begin_audio_capture(&session_label, runtime_settings.values().audio_retention).await?;
                session_id = Some(session_label.clone());
                let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
                let sentence_pause = runtime_settings.values().light_plus_sentence_pause_sec;
                let language = runtime_settings
                    .values()
                    .whisper_language
                    .whisper_hint()
                    .map(str::to_string);
                let bus = Arc::new(TranscriptBus::open_with_paths(
                    TranscriptSession {
                        session_id: session_label.clone(),
                        mode: TranscriptMode::Agent,
                        has_latched_target: false,
                        latched_target_is_self: false,
                        audience: Some(bound.audience.clone()),
                        badge_only: true,
                    },
                    bound
                        .bus
                        .clone()
                        .unwrap_or_else(|| shared_bus.to_path_buf()),
                    broadcast_buses,
                ));
                transcript_bus = Some(Arc::clone(&bus));
                // The Pointer Indicator knob rules every badge path: Settings
                // promises "Base size; Agent mode stays proportionally larger",
                // so the channel dot is the persisted base times the Assistive
                // multiplier, and Off means no dot while the channel listens.
                let badge_settings = Config::load_without_keychain();
                if badge_settings.hold_indicator {
                    hold_badge::show_hold_badge_with_config(
                        hold_badge::HoldBadgeConfig::from_mode_with_base_diameter(
                            BadgeMode::Assistive,
                            f64::from(badge_settings.hold_badge_size),
                        ),
                    );
                }
                let cursor_token = hold_badge::take_token();
                let open_label = channel_open_label(digit);
                hold_badge::update_transcript(cursor_token, &open_label, false);
                let voice_clock = Arc::clone(&last_voice_at);
                let heard_text = Arc::clone(&last_text);
                let emitter = Arc::new(
                    PresentationEmitter::new_with_authority(
                        Arc::new(TokioMutex::new(String::new())),
                        None,
                        None,
                        Some(bus),
                        Some(Arc::clone(&ledger)),
                        None,
                    )
                    .with_cursor_observer(Arc::new(move |projection| {
                        if projection.text.trim().is_empty() {
                            hold_badge::update_transcript(cursor_token, &open_label, false);
                            return;
                        }
                        *voice_clock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = SystemTime::now();
                        projection.text.clone_into(
                            &mut heard_text.lock().unwrap_or_else(|error| error.into_inner()),
                        );
                        hold_badge::update_transcript(
                            cursor_token,
                            &projection.text,
                            projection.degraded,
                        );
                    }))
                    .with_sentence_pause_sec(sentence_pause)
                    .with_refinement_warnings(Arc::clone(&refinement_warnings)),
                );
                let sink: Arc<dyn EventSink> = emitter;
                match recorder
                    .begin_channel_session(
                        session_label,
                        runtime_settings,
                        sink,
                        language,
                        silence_sec,
                        ledger,
                    )
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        super::finish_audio_capture(session_id.as_deref());
                        if channels.is_empty() {
                            hold_badge::hide_hold_badge();
                        }
                        return Err(error);
                    }
                }
            }
        };

        let audience = bound.audience.clone();
        let receipt_session = session_id.clone();
        let receipt_provider = bound.provider.clone();
        let receipt_provider_session = bound.provider_session_id.clone();
        let receipt_bus = transcript_bus.clone();
        channels.insert(
            digit,
            OpenAgentChannel {
                subscriber,
                audience: bound.audience,
                silence_sec,
                provider: bound.provider,
                provider_session_id: bound.provider_session_id,
                opened_at,
                last_voice_at,
                session_id,
                mode,
                bus: bound.bus,
                last_text,
                refinement_warnings,
                transcript_bus,
            },
        );
        drop(recorder_guard);
        drop(channels);
        if matches!(mode, ChannelOpenMode::Live) {
            let line = crate::presentation::agent_ack::channel_session_line(
                &crate::presentation::agent_ack::ChannelSessionLine {
                    state: "open",
                    reason: "opened",
                    channel: &digit.to_string(),
                    agent: &audience,
                    session_id: receipt_session.as_deref(),
                    autoseal_secs: channel_autoseal_secs(),
                    opened_at,
                    utterance_silence_sec: silence_sec,
                    provider: receipt_provider.as_deref(),
                    provider_session_id: receipt_provider_session.as_deref(),
                },
            );
            if let Some(bus) = receipt_bus {
                bus.record_channel_receipt(&line);
            }
        }
        Ok(())
    }

    /// The one closing throne of a live channel session, for silence and
    /// hang-up alike (a session no live process owns is sealed by
    /// [`Self::seal_orphaned_channel_sessions`]). It consumes the open record,
    /// which its caller removed from `agent_channels` exactly once, so a
    /// session gets at most one `sealed` receipt. The receipt follows the
    /// capture close: `end_channel_session` joins the transcription task, so
    /// every evidence row of the session is already on the bus when a
    /// follower reads the boundary. It does not wait for a ledger terminal
    /// seal: a take whose coverage was refused has no other row that
    /// releases its words.
    async fn close_open_channel(
        &self,
        digit: u8,
        open: OpenAgentChannel,
        reason: ChannelSealReason,
        shared_bus: &Path,
        autoseal_secs: u64,
    ) -> Result<()> {
        let retained_audio = {
            let mut recorder_guard = self.recorder.lock().await;
            match recorder_guard.as_mut() {
                Some(recorder) => {
                    let (_last, audio_path) = recorder.end_channel_session(open.subscriber).await?;
                    audio_path
                }
                None => None,
            }
        };
        if let Some(path) = retained_audio.as_deref() {
            // W5 retention parity: a capture the channel owned joins the same
            // take store and retention rules as dictation. The slug carries
            // the heard words; a voiceless close archives as no-speech.
            let heard = open
                .last_text
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            super::retain_session_audio(
                open.session_id.as_deref(),
                path,
                codescribe_core::state::SessionTranscriptArchive::Committed(&heard),
            ).await;
        }
        let opened = crate::presentation::agent_ack::ChannelSessionLine {
            state: "open",
            reason: "opened",
            channel: &digit.to_string(),
            agent: &open.audience,
            session_id: open.session_id.as_deref(),
            autoseal_secs,
            opened_at: open.opened_at,
            utterance_silence_sec: open.silence_sec,
            provider: open.provider.as_deref(),
            provider_session_id: open.provider_session_id.as_deref(),
        };
        let refinement_warnings = open
            .refinement_warnings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(bus) = open.transcript_bus.as_ref() {
            bus.record_channel_receipt(&seal_receipt_line(reason, &opened, &refinement_warnings));
        } else if let Err(error) = append_seal_receipt(
            open.bus.as_deref().unwrap_or(shared_bus),
            reason,
            &opened,
            &refinement_warnings,
        ) {
            tracing::warn!(
                %error,
                digit,
                reason = reason.as_str(),
                "channel seal receipt was not appended"
            );
        }
        super::finish_audio_capture(open.session_id.as_deref());
        let state = self.current_state().await;
        let channels = self.agent_channels.lock().await;
        if channels.is_empty() && state == super::State::Idle {
            hold_badge::hide_hold_badge();
        }
        Ok(())
    }

    /// Startup reconciliation of the channel-session ledger. A session whose
    /// `open` row is still the newest row of its channel belongs to a
    /// process that ended without sealing it — this controller has only just
    /// started, so no such microphone is live. Each gets one `sealed` row
    /// with `reason: orphan` on the bus that carried its `open` row: the
    /// shared bus, or a dedicated bus the binding names. Sessions this
    /// controller already owns are left alone. The channel lock is held for
    /// the scan and the appends, so a digit pressed meanwhile opens after
    /// them and no orphan row can follow a new session's `open` row.
    pub(crate) async fn seal_orphaned_channel_sessions(
        &self,
        shared_bus: &Path,
        binding_file: &Path,
    ) -> Vec<OrphanSeal> {
        let channels = self.agent_channels.lock().await;
        let live: BTreeSet<String> = channels
            .values()
            .filter_map(|open| open.session_id.clone())
            .collect();
        let mut buses = vec![shared_bus.to_path_buf()];
        if let Ok(binding) = load_binding(binding_file) {
            for bus in binding.bindings.values().filter_map(BindingEntry::bus) {
                if !buses.contains(&bus) {
                    buses.push(bus);
                }
            }
        }
        let sealed = tokio::task::spawn_blocking(move || {
            seal_orphaned_sessions(&buses, &live, ORPHAN_SCAN_WINDOW_BYTES)
        })
        .await;
        drop(channels);
        match sealed {
            Ok(sealed) => sealed,
            Err(error) => {
                tracing::warn!(%error, "orphan channel reconciliation stopped");
                Vec::new()
            }
        }
    }

    /// Orphan reconciliation, then the ack watcher plus the channel silence
    /// cap. Started once, from the live controller, so unit tests that only
    /// construct a controller do not scan the real bridge home.
    pub fn spawn_channel_guards(self: &Arc<Self>, handle: tokio::runtime::Handle) {
        if self.channel_guards_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let controller = Arc::clone(self);
        handle.spawn(async move {
            let orphans = controller
                .seal_orphaned_channel_sessions(
                    &crate::presentation::transcript_bus::transcript_bus_path(),
                    &binding_path(),
                )
                .await;
            if !orphans.is_empty() {
                tracing::info!(
                    ?orphans,
                    "sealed channel sessions a previous process left open"
                );
            }
            loop {
                if controller.shutdown_requested.load(Ordering::SeqCst) {
                    break;
                }
                let bridge = active_names::bridge_home();
                let bus = crate::presentation::transcript_bus::transcript_bus_path();
                let seal_bus = bus.clone();
                match tokio::task::spawn_blocking(move || {
                    crate::presentation::agent_ack::scan(&bridge, &bus)
                })
                .await
                {
                    Ok(Ok(stats)) if stats.appended > 0 => {
                        tracing::info!(appended = stats.appended, "agent ack rows appended");
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => tracing::warn!(%error, "agent ack scan failed"),
                    Err(error) => tracing::warn!(%error, "agent ack scan task stopped"),
                }
                let _sealed = controller
                    .poll_channel_autoseal(SystemTime::now(), &seal_bus)
                    .await;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    pub(crate) async fn poll_channel_autoseal(&self, now: SystemTime, bus: &Path) -> Vec<u8> {
        self.seal_channels_silent_for(now, bus, channel_autoseal_secs(), Some(&binding_path()))
            .await
    }

    /// Quiet delivery contract (Founder seal cc6c8248): silence seals and
    /// delivers the utterance spoken so far, then the channel reopens on the
    /// same binding. A channel that has heard no voice yet stays open — the
    /// session is persistent and only the Fn toggle hangs it up.
    pub(crate) async fn seal_channels_silent_for(
        &self,
        now: SystemTime,
        bus: &Path,
        secs: u64,
        reopen_binding: Option<&Path>,
    ) -> Vec<u8> {
        let due: Vec<(u8, OpenAgentChannel)> = {
            let mut channels = self.agent_channels.lock().await;
            let digits: Vec<u8> = channels
                .iter()
                .filter(|(_, open)| {
                    let last = *open
                        .last_voice_at
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    last > open.opened_at && silence_is_due(last, now, secs)
                })
                .map(|(digit, _)| *digit)
                .collect();
            digits
                .into_iter()
                .filter_map(|digit| channels.remove(&digit).map(|open| (digit, open)))
                .collect()
        };
        let mut sealed = Vec::new();
        for (digit, open) in due {
            let mode = open.mode;
            if let Err(error) = self
                .close_open_channel(digit, open, ChannelSealReason::Silence, bus, secs)
                .await
            {
                tracing::warn!(%error, digit, "channel auto-seal could not close the capture");
                continue;
            }
            sealed.push(digit);
            if let Some(binding) = reopen_binding
                && let Err(error) = self.dispatch_agent_channel(digit, binding, bus, mode).await
            {
                tracing::warn!(
                    %error,
                    digit,
                    "quiet-contract reopen failed; the channel stays closed"
                );
            }
        }
        sealed
    }

    pub async fn channel_hud_states(&self) -> Vec<ChannelHudState> {
        let secs = channel_autoseal_secs();
        let ducking = crate::audio::tts_duck::channel_capture_should_drop();
        let channels = self.agent_channels.lock().await;
        let mut digits: Vec<u8> = channels.keys().copied().collect();
        digits.sort_unstable();
        digits
            .into_iter()
            .filter_map(|digit| {
                let open = channels.get(&digit)?;
                let last = *open
                    .last_voice_at
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                Some(ChannelHudState {
                    open: true,
                    loud: true,
                    channel: digit.to_string(),
                    audience: open.audience.clone(),
                    label: channel_open_label(digit),
                    autoseal_secs: secs,
                    autoseal_deadline: (secs > 0)
                        .then(|| last.checked_add(Duration::from_secs(secs)))
                        .flatten(),
                    tts_ducking: ducking,
                    opened_at: open.opened_at,
                    utterance_silence_ms: utterance_silence_ms(open.silence_sec),
                    provider: open.provider.clone(),
                    provider_session_id: open.provider_session_id.clone(),
                    follower_alive: None,
                })
            })
            .collect()
    }

    /// Roster truth for the overlay popover: every bound digit — open or not —
    /// with follower liveness read from the session-bridge leases (fresh
    /// heartbeat, same convention as the helper's `--status`). Open channels
    /// keep their full HUD fields; a bound-but-closed digit carries
    /// `open: false` and `UNIX_EPOCH` in `opened_at`, which no consumer reads
    /// for closed rows. A roster row never starts anything — display only.
    pub async fn channel_roster_states(&self) -> Vec<ChannelHudState> {
        let bridge = codescribe_core::stt::active_names::bridge_home();
        self.channel_roster_states_at(&binding_path(), &bridge)
            .await
    }

    pub(crate) async fn channel_roster_states_at(
        &self,
        binding: &Path,
        bridge_home: &Path,
    ) -> Vec<ChannelHudState> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        let live = codescribe_core::stt::active_names::live_follower_sessions_at(
            bridge_home,
            now,
            codescribe_core::stt::active_names::LEASE_TTL_SECONDS,
        );
        let alive = |provider: &Option<String>, session: &Option<String>| -> Option<bool> {
            match (provider.as_deref(), session.as_deref()) {
                (Some(provider), Some(session)) => {
                    Some(live.contains(&(provider.to_lowercase(), session.to_string())))
                }
                _ => None,
            }
        };
        let mut states = self.channel_hud_states().await;
        for state in &mut states {
            state.follower_alive = alive(&state.provider, &state.provider_session_id);
        }
        let Ok(file) = load_binding(binding) else {
            return states;
        };
        for (digit, entry) in &file.bindings {
            if digit.len() != 1 || !digit.chars().all(|c| ('1'..='9').contains(&c)) {
                continue;
            }
            if states.iter().any(|state| &state.channel == digit) {
                continue;
            }
            let provider = Some(entry.provider.trim().to_string()).filter(|s| !s.is_empty());
            let session =
                Some(entry.provider_session_id.trim().to_string()).filter(|s| !s.is_empty());
            states.push(ChannelHudState {
                open: false,
                loud: false,
                channel: digit.clone(),
                audience: entry.audience.trim().to_string(),
                label: String::new(),
                autoseal_secs: 0,
                autoseal_deadline: None,
                tts_ducking: false,
                opened_at: SystemTime::UNIX_EPOCH,
                utterance_silence_ms: 0,
                follower_alive: alive(&provider, &session),
                provider,
                provider_session_id: session,
            });
        }
        states.sort_by(|a, b| a.channel.cmp(&b.channel));
        states
    }

    /// Conversation and energy calibration stay exclusive owners.
    pub(crate) async fn refuse_exclusive_while_agent_channel_open(
        &self,
        surface: &str,
    ) -> Result<()> {
        let channels = self.agent_channels.lock().await;
        let mut digits: Vec<u8> = channels.keys().copied().collect();
        digits.sort_unstable();
        let subscriber = {
            let recorder = self.recorder.lock().await;
            recorder
                .as_ref()
                .is_some_and(|recorder| recorder.has_non_take_subscriber())
        };
        if digits.is_empty() && !subscriber {
            return Ok(());
        }
        Err(anyhow!(
            "capture admission refused: agent channel is open on {surface}; digits {digits:?}. Conversation and energy calibration stay exclusive owners of the microphone"
        ))
    }

    #[cfg(test)]
    pub(crate) async fn agent_channel_snapshot(&self, digit: u8) -> Option<OpenAgentChannel> {
        self.agent_channels.lock().await.get(&digit).cloned()
    }

    #[cfg(test)]
    pub(crate) async fn capture_subscriber_view(&self) -> (usize, bool, bool) {
        let recorder = self.recorder.lock().await;
        let Some(recorder) = recorder.as_ref() else {
            return (0, false, false);
        };
        (
            recorder.capture_subscriber_count(),
            recorder.has_non_take_subscriber(),
            recorder.has_take_subscriber(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::time::{Duration, SystemTime};

    fn write_binding(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(BINDING_FILENAME);
        std::fs::write(&path, body).expect("write binding");
        path
    }

    fn bus_rows(bus: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(bus)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect()
    }

    /// Digit 3 bound to Leon on a dedicated `buses/channel-3.jsonl`.
    fn write_dedicated_binding(dir: &Path) -> (PathBuf, PathBuf) {
        let channel_bus = dir.join("buses/channel-3.jsonl");
        let binding = format!(
            r#"{{"schema":"vc.agent-audience-binding.v1","bindings":{{"3":{{"audience":"Leon","provider":"codex","provider_session_id":"leon-session","bus":"{}"}}}}}}"#,
            channel_bus.display()
        );
        (write_binding(dir, &binding), channel_bus)
    }

    /// Opens digit 3 without a device and stamps the session label a Live
    /// open mints, so receipts carry the identity followers key on. Returns
    /// the open instant.
    async fn open_stamped(
        controller: &RecordingController,
        binding: &Path,
        shared_bus: &Path,
        session: &str,
    ) -> SystemTime {
        controller
            .dispatch_agent_channel(3, binding, shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        stamp_session(controller, session).await
    }

    async fn stamp_session(controller: &RecordingController, session: &str) -> SystemTime {
        let mut channels = controller.agent_channels.lock().await;
        let open = channels.get_mut(&3).expect("channel 3 is open");
        open.session_id = Some(session.to_string());
        open.opened_at
    }

    async fn hear_voice_at(controller: &RecordingController, at: SystemTime) {
        let open = controller.agent_channel_snapshot(3).await.expect("open");
        *open
            .last_voice_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = at;
    }

    fn sealed_rows(rows: &[serde_json::Value]) -> Vec<(String, String)> {
        rows.iter()
            .filter(|row| row["schema"] == "codescribe.channel-session.v1")
            .filter(|row| row["state"] == "sealed")
            .map(|row| {
                (
                    row["session_id"].as_str().unwrap_or_default().to_string(),
                    row["reason"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn digit_zero_is_broadcast_without_a_binding_file() {
        let missing = Path::new("/tmp/codescribe-fn1-missing-binding.json");
        let bound = resolve_digit(0, missing).expect("broadcast");
        assert_eq!(bound.audience, "*");
        assert!(bound.provider.is_none());
        const {
            assert!(!CHANNEL_PREVIEW.autopaste);
            assert!(!CHANNEL_PREVIEW.overlay_document);
            assert!(CHANNEL_PREVIEW.hold_badge);
        }
    }

    #[test]
    fn missing_malformed_and_unbound_digits_refuse_loudly() {
        let dir = tempfile::tempdir().expect("temp");
        let missing = dir.path().join(BINDING_FILENAME);
        match resolve_digit(3, &missing) {
            Err(ChannelOpenRefusal::BindingMissing { path }) => {
                assert!(path.contains(BINDING_FILENAME), "{path}");
            }
            other => panic!("expected missing file, got {other:?}"),
        }

        let broken = write_binding(dir.path(), "{not json");
        match resolve_digit(3, &broken) {
            Err(ChannelOpenRefusal::BindingMalformed { detail, .. }) => {
                assert!(!detail.is_empty(), "{detail}");
            }
            other => panic!("expected malformed file, got {other:?}"),
        }

        let wrong_schema = write_binding(
            dir.path(),
            r#"{"schema":"other","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"s"}}}"#,
        );
        assert!(matches!(
            resolve_digit(3, &wrong_schema),
            Err(ChannelOpenRefusal::BindingMalformed { .. })
        ));

        let unbound = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude","provider_session_id":"lease-ada"}}}"#,
        );
        assert_eq!(
            resolve_digit(3, &unbound),
            Err(ChannelOpenRefusal::DigitUnbound { digit: 3 })
        );
        let leon = resolve_digit(1, &unbound).expect("bound session");
        assert_eq!(leon.audience, "Ada");
        assert_eq!(leon.provider.as_deref(), Some("claude"));
        assert_eq!(leon.provider_session_id.as_deref(), Some("lease-ada"));
        assert!(matches!(
            resolve_digit(10, &unbound),
            Err(ChannelOpenRefusal::InvalidDigit)
        ));
    }

    #[tokio::test]
    async fn missing_binding_does_not_open_capture() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join(BINDING_FILENAME);
        let shared_bus = dir.path().join("shared-bus.jsonl");
        let error = controller
            .dispatch_agent_channel(4, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect_err("missing binding must refuse");
        let message = format!("{error:#}");
        assert!(
            message.contains("missing"),
            "refusal must name the missing file: {message}"
        );
        assert!(controller.agent_channel_snapshot(4).await.is_none());
        let (count, channel, take) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        assert!(!take);
        assert_eq!(controller.current_state().await, super::super::State::Idle);
    }

    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn second_press_closes_the_channel_and_leaves_dictation_idle() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        );
        let shared_bus = dir.path().join("shared-bus.jsonl");
        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        let open = controller.agent_channel_snapshot(3).await.expect("open");
        assert_eq!(open.audience, "Leon");
        assert_eq!(open.provider.as_deref(), Some("codex"));
        assert_eq!(open.provider_session_id.as_deref(), Some("leon-session"));
        assert_eq!(
            crate::audio::streaming_recorder::channel_session_silence(open.silence_sec),
            Some(open.silence_sec),
            "the channel keeps its own hands-free silence while it stays open"
        );
        assert_eq!(controller.current_state().await, super::super::State::Idle);
        let (count, channel, take) = controller.capture_subscriber_view().await;
        assert_eq!(count, 1);
        assert!(channel);
        assert!(!take);

        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
            .await
            .expect("second press closes");
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        let (count, channel, _) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        assert_eq!(controller.current_state().await, super::super::State::Idle);
        let rows = bus_rows(&shared_bus);
        assert_eq!(
            rows.len(),
            1,
            "a binding without a dedicated bus seals on the shared bus: {rows:?}"
        );
        assert_eq!(rows[0]["reason"], "hangup", "{rows:?}");
    }

    #[tokio::test]
    async fn conversation_refuses_while_a_channel_is_open() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude","provider_session_id":"ada-session"}}}"#,
        );
        let shared_bus = dir.path().join("shared-bus.jsonl");
        controller
            .dispatch_agent_channel(1, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        let error = controller
            .refuse_exclusive_while_agent_channel_open("conversation")
            .await
            .expect_err("exclusive surface must refuse");
        let message = format!("{error:#}");
        assert!(message.contains("agent channel"), "{message}");
        assert!(message.contains("conversation"), "{message}");
        assert!(controller.agent_channel_snapshot(1).await.is_some());
        assert_eq!(controller.current_state().await, super::super::State::Idle);
    }

    #[test]
    #[serial(agent_ack_duck)]
    fn channel_autoseal_knob_defaults_to_the_brief_and_zero_disables() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { std::env::remove_var(CHANNEL_AUTOSEAL_SECS_ENV) };
            }
        }
        let _restore = Restore;
        unsafe { std::env::remove_var(CHANNEL_AUTOSEAL_SECS_ENV) };
        assert_eq!(channel_autoseal_secs(), CHANNEL_AUTOSEAL_SECS_DEFAULT);
        unsafe { std::env::set_var(CHANNEL_AUTOSEAL_SECS_ENV, "15") };
        assert_eq!(channel_autoseal_secs(), 15);
        unsafe { std::env::set_var(CHANNEL_AUTOSEAL_SECS_ENV, "0") };
        assert_eq!(channel_autoseal_secs(), 0);
        unsafe { std::env::set_var(CHANNEL_AUTOSEAL_SECS_ENV, "nope") };
        assert_eq!(channel_autoseal_secs(), CHANNEL_AUTOSEAL_SECS_DEFAULT);
        unsafe { std::env::set_var(CHANNEL_AUTOSEAL_SECS_ENV, " 40 ") };
        assert_eq!(channel_autoseal_secs(), 40);

        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(!super::silence_is_due(
            opened,
            opened + Duration::from_secs(119),
            120
        ));
        assert!(super::silence_is_due(
            opened,
            opened + Duration::from_secs(120),
            120
        ));
        assert!(!super::silence_is_due(
            opened,
            opened + Duration::from_secs(10_000),
            0
        ));
    }

    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn silence_past_the_cap_seals_the_channel_and_clears_the_open_state() {
        crate::audio::tts_duck::clear();
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        );
        let bus = dir.path().join("bus.jsonl");
        controller
            .dispatch_agent_channel(3, &path, &bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        let hud = controller.channel_hud_states().await;
        assert_eq!(hud.len(), 1);
        assert!(hud[0].open && hud[0].loud, "{hud:?}");
        assert_eq!(hud[0].channel, "3");
        assert_eq!(hud[0].audience, "Leon");
        assert!(hud[0].label.contains("CHANNEL 3 OPEN"), "{}", hud[0].label);
        assert!(!hud[0].tts_ducking);
        assert_eq!(hud[0].provider.as_deref(), Some("codex"));
        assert_eq!(hud[0].provider_session_id.as_deref(), Some("leon-session"));

        let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
        let opened = snapshot.opened_at;
        let still_open = controller
            .seal_channels_silent_for(opened + Duration::from_secs(10), &bus, 120, None)
            .await;
        assert!(still_open.is_empty());
        assert!(controller.agent_channel_snapshot(3).await.is_some());
        let voiceless = controller
            .seal_channels_silent_for(opened + Duration::from_secs(10_000), &bus, 120, None)
            .await;
        assert!(
            voiceless.is_empty(),
            "a channel that heard no voice is persistent and never seals on silence"
        );
        let disabled = controller
            .seal_channels_silent_for(opened + Duration::from_secs(10_000), &bus, 0, None)
            .await;
        assert!(disabled.is_empty(), "0 disables the threshold");
        assert!(controller.channel_hud_states().await[0].loud);

        *snapshot
            .last_voice_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = opened + Duration::from_secs(60);
        let too_soon = controller
            .seal_channels_silent_for(opened + Duration::from_secs(179), &bus, 120, None)
            .await;
        assert!(
            too_soon.is_empty(),
            "silence is measured from the last voice"
        );
        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(180), &bus, 120, None)
            .await;
        assert_eq!(sealed, vec![3]);
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        assert!(controller.channel_hud_states().await.is_empty());
        let (count, channel, _) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        let again = controller
            .seal_channels_silent_for(opened + Duration::from_secs(500), &bus, 120, None)
            .await;
        assert!(again.is_empty());

        let text = std::fs::read_to_string(&bus).expect("sealed receipt");
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(rows.len(), 1, "{text}");
        let row = &rows[0];
        assert_eq!(
            row.get("schema").and_then(|value| value.as_str()),
            Some("codescribe.channel-session.v1")
        );
        assert_eq!(
            row.get("kind").and_then(|value| value.as_str()),
            Some("channel_session")
        );
        assert_eq!(
            row.get("state").and_then(|value| value.as_str()),
            Some("sealed")
        );
        assert_eq!(
            row.get("reason").and_then(|value| value.as_str()),
            Some("silence")
        );
        assert_eq!(
            row.get("channel").and_then(|value| value.as_str()),
            Some("3")
        );
        assert_eq!(
            row.get("agent").and_then(|value| value.as_str()),
            Some("Leon")
        );
        assert_eq!(
            row.get("provider").and_then(|value| value.as_str()),
            Some("codex")
        );
        assert_eq!(
            row.get("provider_session_id")
                .and_then(|value| value.as_str()),
            Some("leon-session")
        );
        assert!(
            row.get("opened_at")
                .and_then(|value| value.as_str())
                .is_some_and(|stamp| stamp.ends_with('Z')),
            "{row}"
        );
        assert_eq!(
            row.get("loud").and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            row.get("autoseal_secs").and_then(|value| value.as_u64()),
            Some(120)
        );
        let emitted_at = row
            .get("emitted_at")
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(emitted_at.ends_with('Z'), "{emitted_at}");
    }

    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn a_binding_bus_routes_the_channel_receipts_to_the_dedicated_file() {
        crate::audio::tts_duck::clear();
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (path, channel_bus) = write_dedicated_binding(dir.path());

        let bound = resolve_digit(3, &path).expect("bound session");
        assert_eq!(bound.bus.as_deref(), Some(channel_bus.as_path()));

        let shared_bus = dir.path().join("shared-bus.jsonl");
        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
        assert_eq!(snapshot.bus.as_deref(), Some(channel_bus.as_path()));
        let opened = snapshot.opened_at;
        *snapshot
            .last_voice_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = opened + Duration::from_secs(60);

        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(180), &shared_bus, 120, None)
            .await;
        assert_eq!(sealed, vec![3]);

        assert!(
            !shared_bus.exists(),
            "a dedicated-bus channel writes nothing to the shared bus"
        );
        let text = std::fs::read_to_string(&channel_bus).expect("dedicated receipt");
        let row: serde_json::Value =
            serde_json::from_str(text.lines().next().expect("one row")).expect("json");
        assert_eq!(
            row.get("state").and_then(|value| value.as_str()),
            Some("sealed")
        );
        assert_eq!(
            row.get("channel").and_then(|value| value.as_str()),
            Some("3")
        );
    }

    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn silence_seal_reopens_the_channel_on_the_same_binding() {
        crate::audio::tts_duck::clear();
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        );
        let bus = dir.path().join("bus.jsonl");
        controller
            .dispatch_agent_channel(3, &path, &bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open");
        let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
        let opened = snapshot.opened_at;
        *snapshot
            .last_voice_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = opened + Duration::from_secs(1);

        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(6), &bus, 5, Some(&path))
            .await;
        assert_eq!(sealed, vec![3]);

        let reopened = controller
            .agent_channel_snapshot(3)
            .await
            .expect("the quiet contract reopens the channel after delivery");
        assert_eq!(reopened.mode, ChannelOpenMode::AttachedOnly);
        assert_eq!(reopened.audience, "Leon");
        let fresh_voice = *reopened
            .last_voice_at
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            fresh_voice, reopened.opened_at,
            "the reopened channel starts with a clean voice clock"
        );
        let hud = controller.channel_hud_states().await;
        assert_eq!(hud.len(), 1);
        assert!(hud[0].open, "{hud:?}");

        let text = std::fs::read_to_string(&bus).expect("sealed receipt");
        assert_eq!(
            text.lines().count(),
            1,
            "one silence seal writes one receipt; the attached-only reopen adds none: {text}"
        );
    }

    /// The observed loss: drafts reached the bus, the ledger refused terminal
    /// finality, and the Founder hung up before the silence boundary. The
    /// hang-up itself must put the closing row on the channel's own bus.
    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn hang_up_before_any_seal_writes_one_hangup_receipt_on_the_channel_bus() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (path, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");
        let session = "agent-channel-3-hangup";
        let opened = open_stamped(&controller, &path, &shared_bus, session).await;
        // Voice was heard one second in; the five-second silence boundary
        // has not passed, so no silence seal can precede the hang-up.
        hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
        assert!(
            controller
                .seal_channels_silent_for(opened + Duration::from_secs(2), &shared_bus, 5, None)
                .await
                .is_empty()
        );

        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
            .await
            .expect("second press hangs up");

        assert!(controller.agent_channel_snapshot(3).await.is_none());
        let (count, channel, _) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        assert!(
            !shared_bus.exists(),
            "a dedicated-bus channel writes nothing to the shared bus"
        );
        let rows = bus_rows(&channel_bus);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row["schema"], "codescribe.channel-session.v1");
        assert_eq!(row["kind"], "channel_session");
        assert_eq!(row["state"], "sealed");
        assert_eq!(row["reason"], "hangup");
        assert_eq!(row["channel"], "3");
        assert_eq!(row["agent"], "Leon");
        assert_eq!(row["session_id"], session);
        assert_eq!(row["loud"], false);
        assert_eq!(row["provider"], "codex");
        assert_eq!(row["provider_session_id"], "leon-session");
        assert_eq!(row["autoseal_secs"], channel_autoseal_secs());
        assert!(
            row["opened_at"]
                .as_str()
                .is_some_and(|stamp| stamp.ends_with('Z')),
            "{row}"
        );
        assert!(
            row["emitted_at"]
                .as_str()
                .is_some_and(|stamp| stamp.ends_with('Z')),
            "{row}"
        );

        // The hung-up session is gone: a later silence poll seals nothing.
        let later = controller
            .seal_channels_silent_for(
                opened + Duration::from_secs(600),
                &shared_bus,
                5,
                Some(&path),
            )
            .await;
        assert!(later.is_empty());
        assert_eq!(bus_rows(&channel_bus).len(), 1);
    }

    /// The roster names every bound digit and says who is actually listening:
    /// a fresh lease heartbeat marks the follower alive, a missing lease marks
    /// it dead, and a bound-but-closed digit still appears with `open: false`.
    #[tokio::test]
    async fn a_roster_row_carries_follower_liveness_for_bound_closed_channels() {
        let dir = tempfile::tempdir().expect("temp");
        let binding = dir.path().join("binding.json");
        std::fs::write(
            &binding,
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude-code","provider_session_id":"ada-session"},"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        )
        .expect("binding");
        let bridge = dir.path().join("bridge");
        std::fs::create_dir_all(bridge.join("leases")).expect("leases dir");
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_secs_f64();
        std::fs::write(
            bridge.join("leases/ada.json"),
            format!(
                r#"{{"schema":"codescribe.agent-bridge.lease.v1","lease_id":"ada","name":"ada","active":true,"heartbeat_unix":{now},"provider":"claude-code","provider_session_id":"ada-session"}}"#,
            ),
        )
        .expect("lease");

        let controller = RecordingController::new_without_keychain();
        let roster = controller.channel_roster_states_at(&binding, &bridge).await;

        assert_eq!(roster.len(), 2, "{roster:?}");
        assert_eq!(roster[0].channel, "1");
        assert_eq!(roster[0].audience, "Ada");
        assert!(!roster[0].open);
        assert_eq!(roster[0].follower_alive, Some(true));
        assert_eq!(roster[1].channel, "3");
        assert_eq!(roster[1].audience, "Leon");
        assert!(!roster[1].open);
        assert_eq!(
            roster[1].follower_alive,
            Some(false),
            "no lease means nobody is listening"
        );
    }

    /// One closing throne: a session sealed by silence is never sealed again
    /// by a hang-up. The hang-up closes the session the quiet contract
    /// reopened, and a digit press after a seal without reopen opens.
    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn a_silence_sealed_session_is_never_sealed_again_by_a_hang_up() {
        crate::audio::tts_duck::clear();
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (path, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");

        let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-first").await;
        hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(6), &shared_bus, 5, Some(&path))
            .await;
        assert_eq!(sealed, vec![3]);
        stamp_session(&controller, "agent-channel-3-reopened").await;
        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
            .await
            .expect("hang up the reopened session");
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        assert_eq!(
            sealed_rows(&bus_rows(&channel_bus)),
            vec![
                ("agent-channel-3-first".to_string(), "silence".to_string()),
                ("agent-channel-3-reopened".to_string(), "hangup".to_string()),
            ]
        );

        // Silence without a reopen leaves the digit closed; the next press
        // opens a fresh session instead of sealing the finished one twice.
        let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-last").await;
        hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(6), &shared_bus, 5, None)
            .await;
        assert_eq!(sealed, vec![3]);
        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("press after a seal opens");
        assert!(controller.agent_channel_snapshot(3).await.is_some());
        let sealed = sealed_rows(&bus_rows(&channel_bus));
        assert_eq!(sealed.len(), 3, "{sealed:?}");
        for session in [
            "agent-channel-3-first",
            "agent-channel-3-reopened",
            "agent-channel-3-last",
        ] {
            assert_eq!(
                sealed.iter().filter(|(id, _)| id == session).count(),
                1,
                "{session} is sealed exactly once: {sealed:?}"
            );
        }
        assert!(!shared_bus.exists());
    }

    /// A hang-up ends one session, not the channel: the next press reopens
    /// on the same binding and the quiet contract runs as before.
    #[tokio::test]
    #[serial(agent_ack_duck)]
    async fn reopen_after_a_hang_up_keeps_the_quiet_contract() {
        crate::audio::tts_duck::clear();
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (path, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");

        open_stamped(&controller, &path, &shared_bus, "agent-channel-3-before").await;
        controller
            .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
            .await
            .expect("hang up");
        assert!(controller.channel_hud_states().await.is_empty());

        let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-after").await;
        let hud = controller.channel_hud_states().await;
        assert_eq!(hud.len(), 1);
        assert!(hud[0].open && hud[0].loud, "{hud:?}");
        assert_eq!(hud[0].audience, "Leon");
        hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(6), &shared_bus, 5, Some(&path))
            .await;
        assert_eq!(sealed, vec![3]);
        let reopened = controller
            .agent_channel_snapshot(3)
            .await
            .expect("the quiet contract reopens after a hang-up cycle");
        assert_eq!(reopened.audience, "Leon");
        assert_eq!(reopened.bus.as_deref(), Some(channel_bus.as_path()));
        assert_eq!(
            sealed_rows(&bus_rows(&channel_bus)),
            vec![
                ("agent-channel-3-before".to_string(), "hangup".to_string()),
                ("agent-channel-3-after".to_string(), "silence".to_string()),
            ]
        );
    }

    /// Byte shape of the `open` row the writer put on the shared bus for a
    /// channel whose app was then quit (observed 2026-09-29, channel 2).
    /// Identities are neutral; field order and formats are the writer's.
    const ORPHAN_SESSION: &str = "agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f";
    const ORPHAN_OPEN_ROW: &str = r#"{"agent":"Leon","autoseal_secs":5,"channel":"2","emitted_at":"2026-09-29T17:22:06.346400Z","kind":"channel_session","loud":true,"opened_at":"2026-09-29T17:22:06.265163Z","provider":"claude-code","provider_session_id":"leon-session","reason":"opened","schema":"codescribe.channel-session.v1","session_id":"agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f","state":"open","utterance_silence_sec":5.0}"#;
    /// The session's ledger terminal seal, which followed its `open` row.
    /// It is transcript evidence, not a channel-session boundary.
    const ORPHAN_TERMINAL_SEAL_ROW: &str = r#"{"schema":"codescribe.transcript-evidence.v1","sequence":3,"session_id":"agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f","reducer_revision":5,"reducer_action":"record_ledger_terminal_seal","document_index":0,"rendered_text":"Iwo","terminal":false,"audience":"Leon"}"#;
    /// An earlier session of the same channel that the orphan superseded.
    const SUPERSEDED_OPEN_ROW: &str = r#"{"agent":"Leon","autoseal_secs":5,"channel":"2","emitted_at":"2026-09-29T16:34:23.279789Z","kind":"channel_session","loud":true,"opened_at":"2026-09-29T16:34:23.199613Z","provider":"claude-code","provider_session_id":"leon-session","reason":"opened","schema":"codescribe.channel-session.v1","session_id":"agent-channel-2-superseded","state":"open","utterance_silence_sec":5.0}"#;
    const CHANNEL_2_BINDING: &str = r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"2":{"audience":"Leon","provider":"claude-code","provider_session_id":"leon-session"}}}"#;

    /// Appends rows exactly as another process left them.
    fn append_raw(bus: &Path, rows: &[&str]) {
        use std::io::Write;
        if let Some(parent) = bus.parent() {
            std::fs::create_dir_all(parent).expect("bus dir");
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(bus)
            .expect("bus");
        for row in rows {
            writeln!(file, "{row}").expect("row");
        }
    }

    /// The `open` row a Live dispatch writes for `session` on channel 3.
    fn live_open_line(session: &str) -> crate::presentation::agent_ack::ChannelSessionLine<'_> {
        crate::presentation::agent_ack::ChannelSessionLine {
            state: "open",
            reason: "opened",
            channel: "3",
            agent: "Leon",
            session_id: Some(session),
            autoseal_secs: 5,
            opened_at: SystemTime::now(),
            utterance_silence_sec: 5.0,
            provider: Some("codex"),
            provider_session_id: Some("leon-session"),
        }
    }

    fn write_open_row(bus: &Path, open: &crate::presentation::agent_ack::ChannelSessionLine<'_>) {
        crate::presentation::agent_ack::append_json_line(
            bus,
            &crate::presentation::agent_ack::channel_session_line(open),
        )
        .expect("open row");
    }

    /// The observed bug: channel 2's newest row stayed `open` after the app
    /// quit, and the overlay painted a live microphone forever. One start
    /// seals it with the key the overlay projection pairs on.
    #[tokio::test]
    async fn a_session_a_previous_process_left_open_gets_one_orphan_seal() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let binding = write_binding(dir.path(), CHANNEL_2_BINDING);
        let shared_bus = dir.path().join("transcript-events.jsonl");
        append_raw(
            &shared_bus,
            &[
                SUPERSEDED_OPEN_ROW,
                ORPHAN_OPEN_ROW,
                ORPHAN_TERMINAL_SEAL_ROW,
            ],
        );

        let sealed = controller
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;

        assert_eq!(
            sealed,
            vec![OrphanSeal {
                bus: shared_bus.clone(),
                channel: "2".to_string(),
                session_id: ORPHAN_SESSION.to_string(),
            }]
        );
        let rows = bus_rows(&shared_bus);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(
            sealed_rows(&rows),
            vec![(ORPHAN_SESSION.to_string(), "orphan".to_string())],
            "the superseded session was ended by its successor's open row"
        );
        let (open, seal) = (&rows[1], &rows[3]);
        assert_eq!(seal["schema"], "codescribe.channel-session.v1");
        assert_eq!(seal["kind"], "channel_session");
        assert_eq!(seal["state"], "sealed");
        assert_eq!(seal["reason"], "orphan");
        assert_eq!(seal["loud"], false);
        // OverlayChannelDelivery.Bus keys a channel's session on these and
        // lets a non-open row close it only when `opened_at` is the open
        // session's own, byte for byte.
        for key in [
            "channel",
            "agent",
            "session_id",
            "opened_at",
            "provider",
            "provider_session_id",
            "autoseal_secs",
            "utterance_silence_sec",
        ] {
            assert_eq!(seal[key], open[key], "{key}");
        }
        let latest = latest_channel_rows(&shared_bus, ORPHAN_SCAN_WINDOW_BYTES).expect("scan");
        assert_eq!(
            latest["2"]["state"], "sealed",
            "channel 2 is no longer open"
        );
    }

    #[tokio::test]
    async fn channel_seal_records_whisper_timeout_and_disabled_status() {
        for (codes, status) in [
            (
                vec!["live_refinement_stop_deadline".to_string()],
                "refinement_incomplete",
            ),
            (
                vec!["agent_raw_whisper_disabled".to_string()],
                "whisper_disabled",
            ),
            (Vec::new(), "settled"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("channel.jsonl");
            let warnings = Arc::new(StdMutex::new(Vec::new()));
            let mut emitter =
                PresentationEmitter::new(Arc::new(TokioMutex::new(String::new())), None, None)
                    .with_refinement_warnings(warnings.clone());
            for code in &codes {
                emitter.on_event(
                    &codescribe_core::pipeline::contracts::EngineEvent::Warning {
                        code: code.clone(),
                        message: "refinement_not_completed=true".into(),
                    },
                );
            }
            emitter.finish().await;
            let open = live_open_line("agent-raw-receipt");
            append_seal_receipt(
                &path,
                ChannelSealReason::Hangup,
                &open,
                &warnings.lock().unwrap(),
            )
            .unwrap();
            let rows = bus_rows(&path);
            assert_eq!(rows[0]["raw_processing_status"], status);
            assert_eq!(rows[0]["refinement_warnings"], serde_json::json!(codes));
        }
    }

    #[tokio::test]
    async fn sessions_sealed_by_silence_or_hang_up_get_no_orphan_row() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (binding, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");
        for (session, reason) in [
            ("agent-channel-3-silence", ChannelSealReason::Silence),
            ("agent-channel-3-hangup", ChannelSealReason::Hangup),
        ] {
            let open = live_open_line(session);
            write_open_row(&channel_bus, &open);
            append_seal_receipt(&channel_bus, reason, &open, &[]).expect("seal");
        }
        let before = std::fs::read(&channel_bus).expect("bus");

        let sealed = controller
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;

        assert!(sealed.is_empty(), "{sealed:?}");
        assert_eq!(std::fs::read(&channel_bus).expect("bus"), before);
        assert!(!shared_bus.exists());
    }

    #[tokio::test]
    async fn a_second_start_writes_no_second_orphan_row() {
        let dir = tempfile::tempdir().expect("temp");
        let binding = write_binding(dir.path(), CHANNEL_2_BINDING);
        let shared_bus = dir.path().join("transcript-events.jsonl");
        append_raw(&shared_bus, &[ORPHAN_OPEN_ROW, ORPHAN_TERMINAL_SEAL_ROW]);

        let first = RecordingController::new_without_keychain()
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;
        let second = RecordingController::new_without_keychain()
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;

        assert_eq!(first.len(), 1, "{first:?}");
        assert!(second.is_empty(), "{second:?}");
        assert_eq!(
            sealed_rows(&bus_rows(&shared_bus)),
            vec![(ORPHAN_SESSION.to_string(), "orphan".to_string())]
        );
    }

    /// W5 buses: the `open` row sits on the binding's dedicated bus, so the
    /// orphan seal lands there and the shared bus is not touched.
    #[tokio::test]
    async fn an_open_row_on_a_dedicated_bus_is_sealed_on_that_bus() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (binding, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");
        append_raw(&shared_bus, &[ORPHAN_TERMINAL_SEAL_ROW]);
        let shared_before = std::fs::read(&shared_bus).expect("shared bus");
        let open = live_open_line("agent-channel-3-orphan");
        write_open_row(&channel_bus, &open);

        let sealed = controller
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;

        assert_eq!(
            sealed,
            vec![OrphanSeal {
                bus: channel_bus.clone(),
                channel: "3".to_string(),
                session_id: "agent-channel-3-orphan".to_string(),
            }]
        );
        assert_eq!(
            sealed_rows(&bus_rows(&channel_bus)),
            vec![("agent-channel-3-orphan".to_string(), "orphan".to_string())]
        );
        assert_eq!(
            std::fs::read(&shared_bus).expect("shared bus"),
            shared_before
        );
    }

    /// A session this controller opened is live, not an orphan, even when its
    /// `open` row is its channel's newest.
    #[tokio::test]
    async fn a_session_this_process_owns_is_never_sealed_as_an_orphan() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let (binding, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared-bus.jsonl");
        open_stamped(&controller, &binding, &shared_bus, "agent-channel-3-live").await;
        write_open_row(&channel_bus, &live_open_line("agent-channel-3-live"));
        let before = std::fs::read(&channel_bus).expect("bus");

        let sealed = controller
            .seal_orphaned_channel_sessions(&shared_bus, &binding)
            .await;

        assert!(sealed.is_empty(), "{sealed:?}");
        assert_eq!(std::fs::read(&channel_bus).expect("bus"), before);
        assert!(controller.agent_channel_snapshot(3).await.is_some());
    }

    /// The scan reads a bounded tail and starts on a row boundary: a row the
    /// window cuts is dropped whole, a row the window starts on is kept.
    #[test]
    fn the_orphan_scan_reads_only_whole_rows_of_the_bus_tail() {
        let dir = tempfile::tempdir().expect("temp");
        let bus = dir.path().join("bus.jsonl");
        let older = SUPERSEDED_OPEN_ROW.replace(r#""channel":"2""#, r#""channel":"4""#);
        append_raw(&bus, &[older.as_str(), ORPHAN_OPEN_ROW]);
        let newest = ORPHAN_OPEN_ROW.len() as u64 + 1;

        let exact = latest_channel_rows(&bus, newest).expect("scan");
        assert_eq!(exact.keys().collect::<Vec<_>>(), vec!["2"]);
        let one_more = latest_channel_rows(&bus, newest + 1).expect("scan");
        assert_eq!(one_more.keys().collect::<Vec<_>>(), vec!["2"]);
        let cut = latest_channel_rows(&bus, newest - 1).expect("scan");
        assert!(cut.is_empty(), "{cut:?}");
        let whole = latest_channel_rows(&bus, ORPHAN_SCAN_WINDOW_BYTES).expect("scan");
        assert_eq!(whole.keys().collect::<Vec<_>>(), vec!["2", "4"]);

        let sealed = seal_orphaned_sessions(std::slice::from_ref(&bus), &BTreeSet::new(), newest);
        assert_eq!(
            sealed
                .iter()
                .map(|seal| seal.channel.as_str())
                .collect::<Vec<_>>(),
            vec!["2"],
            "an open row outside the window is not read"
        );
        assert!(
            latest_channel_rows(&dir.path().join("missing.jsonl"), newest)
                .expect("missing bus")
                .is_empty()
        );
    }
}
