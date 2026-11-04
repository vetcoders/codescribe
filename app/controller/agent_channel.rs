//! Fn+digit agent channel.
//!
//! The channel is not a dictation take and not a `State` variant. It subscribes
//! to the shared capture and stamps `audience` on Bus rows. A repeated digit
//! hangs up. A measured pause of `agent_channel.send_pause_sec` after heard
//! voice sends the message; under `session_policy: continuous` (default) the
//! session keeps listening and the next message starts on the same capture
//! subscription, under `end_after_message` the session ends as a hang-up does.
//! A session a previous process left open is sealed as an orphan when the
//! controller starts. Paste and the overlay document stay off. The hold badge
//! is the preview; `ChannelHudState` is the open-mic fact the overlay paints
//! from.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use codescribe_core::config::Config;
use codescribe_core::config::agent_channel::{ChannelSegmentFinish, ChannelSessionPolicy};
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use codescribe_core::pipeline::contracts::EventSink;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

use crate::audio::streaming_recorder::CaptureSubscriberId;
use crate::os::hold_badge::{self, BadgeMode};
use crate::presentation::transcript_bus::ChannelCohortMember;
use crate::presentation::{PresentationEmitter, TranscriptBus, TranscriptMode, TranscriptSession};
use crate::stt::active_names;

use super::RecordingController;
use super::speech_clock::SpeechClock;

/// Schema id and filename. The file lives in the agent-bridge home, next to
/// the leases followers already read.
pub const BINDING_SCHEMA: &str = "vc.agent-audience-binding.v1";
pub const BINDING_FILENAME: &str = "vc.agent-audience-binding.v1.json";

/// Construct the live channel reducer from the exact generation also sent to
/// its decoder. Channels and dictation share the same track choices.
pub(crate) fn channel_presentation_emitter(
    bus: Arc<TranscriptBus>,
    ledger: Arc<StdMutex<AcousticLedger>>,
    settings: &codescribe_core::config::RuntimeSettingsSnapshot,
) -> PresentationEmitter {
    PresentationEmitter::new_with_authority(
        Arc::new(TokioMutex::new(String::new())),
        None,
        None,
        Some(bus),
        Some(ledger),
        None,
    )
    .with_transcription_controls(
        settings.seal_ledger_enabled(),
        settings.apple_live_enabled(),
    )
}

/// Document of message `index` of a continuous channel session. The label
/// prefix keeps `agent-channel-<digit>-`, so followers map it to its channel.
pub(crate) fn segment_document_id(label: &str, index: u32) -> String {
    format!("{label}-s{index}")
}

/// One message's transcript document: its bus session, ledger and reducer.
pub(crate) struct ChannelSegmentParts {
    pub transcript_bus: Arc<TranscriptBus>,
    pub ledger: Arc<StdMutex<AcousticLedger>>,
    pub emitter: Arc<PresentationEmitter>,
    pub last_text: Arc<StdMutex<String>>,
    pub refinement_warnings: Arc<StdMutex<Vec<String>>>,
}

/// Open the bus document, ledger and reducer of one channel message. Its
/// recipients receipt names the document, so the closing seal can publish.
/// Speech edges reach the session clock only while this message is current.
pub(crate) fn open_channel_segment(
    source: &ChannelSegmentSource,
    document_id: String,
    speech_clock: &Arc<StdMutex<ChannelSpeechClock>>,
    digit: u8,
) -> ChannelSegmentParts {
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let last_text = Arc::new(StdMutex::new(String::new()));
    let refinement_warnings = Arc::new(StdMutex::new(Vec::new()));
    let bus = Arc::new(TranscriptBus::open_with_paths(
        TranscriptSession {
            session_id: document_id.clone(),
            mode: TranscriptMode::Agent,
            has_latched_target: false,
            latched_target_is_self: false,
            audience: Some(source.audience.clone()),
            badge_only: true,
        },
        source.bus.clone(),
        source.broadcast_buses.clone(),
    ));
    bus.record_channel_receipt(&serde_json::json!({
        "schema": "codescribe.channel-recipients.v1", "kind": "channel_recipients",
        "session_id": document_id, "channel": digit.to_string(),
        "audience": source.audience, "recipients": source.recipients,
        "emitted_at": chrono::Utc::now().to_rfc3339(),
    }));
    let cursor_token = source.cursor_token;
    let voice_clock = Arc::clone(speech_clock);
    let generation = Arc::clone(&source.generation);
    let own_generation = generation.load(Ordering::SeqCst);
    let heard_text = Arc::clone(&last_text);
    let emitter = Arc::new(
        channel_presentation_emitter(
            Arc::clone(&bus),
            Arc::clone(&ledger),
            &source.runtime_settings,
        )
        .with_apple_preview_observer(Arc::new(move |text| {
            hold_badge::update_transcript(cursor_token, text);
        }))
        .with_speech_activity_observer(Arc::new(move |evidence| {
            if generation.load(Ordering::SeqCst) != own_generation {
                return;
            }
            let mut clock = voice_clock
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            clock.speech.observe(evidence, SystemTime::now());
            clock.publish_now(SystemTime::now());
        }))
        .with_cursor_observer(Arc::new(move |projection| {
            hold_badge::update_voice_state(cursor_token, projection.voice);
            crate::os::tray_status::set_tray_voice_for_take(cursor_token, projection.voice);
            if projection.text.trim().is_empty() {
                return;
            }
            projection
                .text
                .clone_into(&mut heard_text.lock().unwrap_or_else(|error| error.into_inner()));
        }))
        .with_sentence_pause_sec(
            source
                .runtime_settings
                .values()
                .light_plus_sentence_pause_sec,
        )
        .with_refinement_warnings(Arc::clone(&refinement_warnings)),
    );
    ChannelSegmentParts {
        transcript_bus: bus,
        ledger,
        emitter,
        last_text,
        refinement_warnings,
    }
}

/// Light+ for a channel message: terminal punctuation only. Every earlier
/// byte stays as committed; text that already ends a sentence is unchanged.
pub(crate) fn light_plus_terminal_punctuation(committed: &str) -> String {
    let trimmed = committed.trim_end();
    match trimmed.chars().last() {
        Some(last) if last.is_alphanumeric() => {
            let mut text = trimmed.to_string();
            text.push('.');
            text.push_str(&committed[trimmed.len()..]);
            text
        }
        _ => committed.to_string(),
    }
}

/// Broadcast marker for Fn+0. Followers treat it as addressed to them.
pub const BROADCAST_AUDIENCE: &str = "*";

/// Permanent mailbox channel and audience for the Founder.
pub const FOUNDER_CHANNEL: &str = "10";
pub const FOUNDER_AUDIENCE: &str = "founder";

/// Canonical bus path for the Founder's permanent mailbox (`channel-10.jsonl`).
pub fn founder_bus_path_at(bridge_home: &Path) -> PathBuf {
    bridge_home.join("buses").join("channel-10.jsonl")
}

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

/// One frozen archive intent. The canonical helper remains the binding writer;
/// the controller only guards capture admission until that helper has exited.
#[derive(Debug, Clone)]
pub struct AgentArchiveRequest {
    pub channel: u8,
    pub provider: String,
    pub provider_session_id: String,
    pub lease_id: String,
    pub bus: PathBuf,
    pub executable: PathBuf,
    pub bridge_home: PathBuf,
    /// Stop the owner's own live follower before archiving (the drawer's
    /// click on an agent that is alive but no longer reads). Off = the
    /// helper refuses a live reader, as it always did.
    pub release_reader: bool,
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

/// An owner who unticked channel 0 (`policy: "off"`) never receives a
/// broadcast, as the helper's peer send already decides (`bus-demux.py`
/// `agent_broadcast_policy`). Like the helper, a missing or invalid receipt
/// keeps the owner: a broken file must never cut an agent off.
fn broadcast_policy_off(
    root: &Path,
    provider: &str,
    session: &str,
    lease_id: &str,
    bus: &Path,
) -> bool {
    let bus = bus.to_string_lossy();
    let identity = format!("{provider}\0{session}\0{bus}");
    let key = &hex::encode(Sha256::digest(identity.as_bytes()))[..24];
    let path = root
        .join("runtime")
        .join("broadcast-policy")
        .join(format!("{key}.json"));
    let row: serde_json::Value = (|| {
        use std::io::{BufReader, Read};
        let file = std::fs::File::open(&path).ok()?;
        if file.metadata().ok()?.len() > 65536 {
            return None;
        }
        serde_json::from_reader(BufReader::new(file.take(65536))).ok()
    })()
    .unwrap_or(serde_json::Value::Null);
    row["schema"] == "codescribe.broadcast-policy.v1"
        && row["provider"] == provider
        && row["provider_session_id"] == session
        && row["lease_id"] == lease_id
        && row["bus"].as_str() == Some(bus.as_ref())
        && row["policy"] == "off"
}

/// Freeze only concrete bridge owners admitted before this capture starts.
/// Reading this snapshot never acknowledges a delivery or starts a follower.
fn frozen_channel_recipients(
    digit: u8,
    binding: &Path,
    shared_bus: &Path,
) -> Vec<serde_json::Value> {
    let Ok(file) = load_binding(binding) else {
        return Vec::new();
    };
    let root = binding.parent().unwrap_or_else(|| Path::new("."));
    let mut recipients = Vec::new();
    let mut seen = BTreeSet::new();
    for (channel, entry) in &file.bindings {
        if channel.len() != 1
            || !channel.bytes().all(|byte| (b'1'..=b'9').contains(&byte))
            || (digit != 0 && channel != &digit.to_string())
        {
            continue;
        }
        let provider = entry.provider.trim().to_lowercase();
        let session = entry.provider_session_id.trim();
        if provider.is_empty() || session.is_empty() {
            continue;
        }
        let identity = format!("{provider}\0{session}");
        let lease_id = hex::encode(Sha256::digest(identity.as_bytes()))[..32].to_string();
        let lease_path = root.join("leases").join(format!("{lease_id}.json"));
        let lease: serde_json::Value = (|| {
            use std::io::{BufReader, Read};
            let file = std::fs::File::open(&lease_path).ok()?;
            if file.metadata().ok()?.len() > 16 << 20 {
                return None;
            }
            serde_json::from_reader(BufReader::new(file.take(16 << 20))).ok()
        })()
        .unwrap_or(serde_json::Value::Null);
        // Clock after this lease's read. A timestamp taken before the scan
        // treats heartbeats refreshed during a large parse as future.
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        let age = now
            - lease["heartbeat_unix"]
                .as_f64()
                .unwrap_or(f64::NEG_INFINITY);
        let bus = entry.bus().unwrap_or_else(|| shared_bus.to_path_buf());
        let bus = bus.canonicalize().unwrap_or(bus);
        if lease["schema"] != "codescribe.agent-bridge.lease.v1"
            || lease["active"] != true
            || !age.is_finite()
            || !(0.0..=active_names::LEASE_TTL_SECONDS).contains(&age)
            || lease["provider"] != provider
            || lease["provider_session_id"] != session
            || lease["lease_id"] != lease_id
            || lease["bus"].as_str() != bus.to_str()
            || !seen.insert(lease_id.clone())
            || (digit == 0 && broadcast_policy_off(root, &provider, session, &lease_id, &bus))
        {
            continue;
        }
        recipients.push(serde_json::json!({
            "channel": channel, "name": entry.audience.trim(), "audience": entry.audience.trim(),
            "provider": provider, "provider_session_id": session, "lease_id": lease_id, "bus": bus,
        }));
    }
    recipients
}

/// Schema of the per-channel file the reply playback guard reads.
const CHANNEL_SPEECH_NOW_SCHEMA: &str = "codescribe.channel-speech-now.v1";

/// `bus-demux` starts an agent reply into an open channel only after this
/// file reports measured quiet, so a reply waits for a pause, not for a seal.
fn channel_speech_now_path(digit: u8) -> PathBuf {
    active_names::bridge_home()
        .join("runtime")
        .join(format!("channel-speech-{digit}.json"))
}

#[derive(Debug)]
struct SpeechNowFile {
    path: PathBuf,
    channel: u8,
    session_id: String,
    opened_at: SystemTime,
    published: Option<&'static str>,
}

/// The channel's speech clock and the phase file the reply playback guard
/// reads. Cadence follows measured speech edges, never recognizer output.
#[derive(Debug, Default)]
pub(crate) struct ChannelSpeechClock {
    pub(crate) speech: SpeechClock,
    now_file: Option<SpeechNowFile>,
}

impl ChannelSpeechClock {
    /// Publish this session's phase from now on.
    fn publish_to(
        &mut self,
        path: PathBuf,
        channel: u8,
        session_id: String,
        opened_at: SystemTime,
    ) {
        self.now_file = Some(SpeechNowFile {
            path,
            channel,
            session_id,
            opened_at,
            published: None,
        });
        self.publish_now(SystemTime::now());
    }

    /// Rewrite the file when the phase changed. Runs under the clock lock, so
    /// an earlier edge never overwrites a later one.
    fn publish_now(&mut self, now: SystemTime) {
        let phase = self.speech.phase_label();
        let since = match (self.speech.quiet_since(), &self.now_file) {
            (Some(since), _) => since,
            (None, Some(file)) if self.speech.last_edge.is_none() => file.opened_at,
            _ => now,
        };
        let Some(file) = self.now_file.as_mut() else {
            return;
        };
        if file.published == Some(phase) {
            return;
        }
        let since_ms = since
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let row = serde_json::json!({
            "schema": CHANNEL_SPEECH_NOW_SCHEMA,
            "channel": file.channel.to_string(),
            "session_id": file.session_id,
            "phase": phase,
            "since_ms": since_ms,
            "pid": std::process::id(),
        });
        let Some(name) = file.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let staging = file
            .path
            .with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        let written = file
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&staging, format!("{row}\n")))
            .and_then(|()| std::fs::rename(&staging, &file.path));
        match written {
            Ok(()) => file.published = Some(phase),
            Err(error) => {
                let _ = std::fs::remove_file(&staging);
                tracing::warn!(%error, phase, "channel speech phase not published");
            }
        }
    }

    /// Stop publishing and remove this session's file; the channel is closing.
    fn withdraw_now(&mut self) {
        if let Some(file) = self.now_file.take() {
            let _ = std::fs::remove_file(&file.path);
        }
    }
}

/// How one channel session sends and ends, frozen at open from its immutable
/// runtime snapshot. The HUD countdown and the seal both read it, so the
/// pause shown is the pause that sends; `toggle_silence_sec` is not an input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChannelTiming {
    pub policy: ChannelSessionPolicy,
    /// `None`: automatic sending is off (`CODESCRIBE_CHANNEL_AUTOSEAL_SECS=0`).
    pub send_pause: Option<Duration>,
    /// `None`: the session never ends for lack of voice.
    pub voiceless_close: Option<Duration>,
    pub finish: ChannelSegmentFinish,
    pub format_wait: Duration,
}

impl ChannelTiming {
    pub(crate) fn from_settings(
        settings: &codescribe_core::config::RuntimeSettingsSnapshot,
    ) -> Self {
        Self::from_config(settings.values().agent_channel)
    }

    pub(crate) fn from_config(
        channel: codescribe_core::config::agent_channel::AgentChannelConfig,
    ) -> Self {
        Self {
            policy: channel.session_policy,
            send_pause: channel.send_pause(),
            voiceless_close: channel.voiceless_close(),
            finish: channel.segment_finish,
            format_wait: channel.format_wait(),
        }
    }

    /// Whole seconds for the `autoseal_secs` row and HUD fields; `0` = off.
    pub(crate) fn autoseal_secs(&self) -> u64 {
        self.send_pause
            .map_or(0, |pause| pause.as_secs_f64().ceil() as u64)
    }

    /// The utterance silence the channel's own epoch gate runs with.
    pub(crate) fn utterance_silence_sec(&self) -> f32 {
        self.send_pause.map_or(
            codescribe_core::config::agent_channel::DEFAULT_SEND_PAUSE_SEC,
            |pause| pause.as_secs_f32(),
        )
    }

    pub(crate) fn continuous(&self) -> bool {
        self.policy == ChannelSessionPolicy::Continuous
    }
}

impl Default for ChannelTiming {
    fn default() -> Self {
        Self::from_config(codescribe_core::config::agent_channel::AgentChannelConfig::default())
    }
}

/// What a live session needs to start its next message on the same capture
/// subscription: destinations, recipients, and the frozen generation.
#[derive(Debug)]
pub(crate) struct ChannelSegmentSource {
    pub label: String,
    pub audience: String,
    pub bus: PathBuf,
    pub broadcast_buses: Vec<PathBuf>,
    pub recipients: Vec<serde_json::Value>,
    pub runtime_settings: Arc<codescribe_core::config::RuntimeSettingsSnapshot>,
    pub language: Option<String>,
    pub cursor_token: u64,
    /// The current message; speech edges of a draining earlier message are
    /// not this message's clock.
    pub generation: Arc<AtomicU64>,
}

#[derive(Clone)]
pub(crate) struct OpenAgentChannel {
    pub subscriber: CaptureSubscriberId,
    pub audience: String,
    /// Utterance silence of the channel's epoch gate: the send pause.
    pub silence_sec: f32,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
    pub opened_at: SystemTime,
    pub speech_clock: Arc<StdMutex<ChannelSpeechClock>>,
    /// The current message's document (bus session, ledger, archive). Equal
    /// to [`Self::channel_session_id`] for the first message of a
    /// one-message session; `<label>-s<n>` for a continuous session.
    pub session_id: Option<String>,
    /// Identity of the `channel_session` open and sealed rows.
    pub channel_session_id: Option<String>,
    /// When the current message's capture started.
    pub segment_opened_at: SystemTime,
    /// Messages this session has started, the current one included.
    pub segment_index: u32,
    pub timing: ChannelTiming,
    /// `None` for `AttachedOnly`, which runs no transcription.
    pub segment_source: Option<Arc<ChannelSegmentSource>>,
    /// The current message's reducer, for its one formatter pass.
    pub emitter: Option<Arc<PresentationEmitter>>,
    /// Resolves when the previous message of this session was delivered, so
    /// a slow formatter never reorders messages.
    pub delivery_turn: Arc<StdMutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    /// How this channel was opened.
    pub mode: ChannelOpenMode,
    /// Dedicated channel bus; `None` writes receipts to the shared bus.
    pub bus: Option<PathBuf>,
    /// Last non-empty projection, so the archived take carries its words.
    pub last_text: Arc<StdMutex<String>>,
    pub refinement_warnings: Arc<StdMutex<Vec<String>>>,
    /// The same bus session used by the reducer, including its fanout and
    /// per-destination persistence state, retained through the closing receipt.
    pub transcript_bus: Option<Arc<TranscriptBus>>,
    /// Live channel's own acoustic ledger. The same `Arc` is passed into
    /// `begin_channel_session` and held here until close, so the observer
    /// freezes this ledger after that task has joined. `AttachedOnly` stores
    /// nothing: that mode claims no live capture.
    pub capture_ledger: Option<Arc<StdMutex<AcousticLedger>>>,
}

/// The Founder speaks to several channels at once and every agent must know
/// it (Maciej, 2026-10-25: "musicie wiedzieć, że ja teraz nadaję do czterech
/// agentów"). The registry's live members, in digit order, become the shared
/// cohort every open channel bus reads as its `co_audience`. Fn+0 is listed
/// like any open channel, but its own bus follows no cohort.
fn cohort_members(
    channels: &std::collections::HashMap<u8, OpenAgentChannel>,
) -> Vec<ChannelCohortMember> {
    let mut members: Vec<ChannelCohortMember> = channels
        .iter()
        .filter(|(_, open)| matches!(open.mode, ChannelOpenMode::Live))
        .filter_map(|(digit, open)| {
            Some(ChannelCohortMember {
                digit: *digit,
                channel: digit.to_string(),
                audience: open.audience.clone(),
                // The live capture: the current message's document.
                session_id: open.session_id.clone()?,
                provider: open.provider.clone(),
                provider_session_id: open.provider_session_id.clone(),
            })
        })
        .collect();
    members.sort_unstable_by_key(|member| member.digit);
    members
}

/// Every registry mutation runs, with the registry guard held:
/// `freeze_closing_cohort` for the channels about to leave, the mutation,
/// then one `publish_cohort`. Bus writers only ever take the cohort read lock.
impl RecordingController {
    /// Swap the shared cohort for the registry's current live members.
    fn publish_cohort(&self, channels: &std::collections::HashMap<u8, OpenAgentChannel>) {
        let members = Arc::new(cohort_members(channels));
        *self
            .agent_channel_cohort
            .write()
            .unwrap_or_else(|error| error.into_inner()) = members;
    }

    /// Give each closing channel the cohort as it stood before the removal.
    /// One snapshot serves the whole batch, so channels closed by the same
    /// silence still list each other.
    fn freeze_closing_cohort<'a>(&self, closing: impl IntoIterator<Item = &'a OpenAgentChannel>) {
        let cohort = Arc::clone(
            &self
                .agent_channel_cohort
                .read()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for open in closing {
            if let Some(bus) = &open.transcript_bus {
                bus.freeze_co_audience(Arc::clone(&cohort));
            }
        }
    }

    /// Point an admitted channel's bus at the live cohort. Fn+0 keeps its own
    /// recipients and follows none.
    fn follow_cohort(&self, digit: u8, open_bus: Option<&Arc<TranscriptBus>>) {
        if digit != 0
            && let Some(bus) = open_bus
        {
            bus.attach_co_audience(Arc::clone(&self.agent_channel_cohort), digit);
        }
    }
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
    /// Whether that live follower's lease names a vc-frame pane, so a focus
    /// request can land. `None` = no live follower or no claim.
    pub frame_detected: Option<bool>,
    /// Whether that live follower's lease names an iTerm2 or Terminal.app
    /// tab the follower can bring to front. `None` = no live follower or no
    /// claim.
    pub terminal_detected: Option<bool>,
    /// `codex://threads/<thread_id>` for a Codex follower whose lease names
    /// its Codex app thread, live or not: the app opens that link itself, so
    /// it needs no follower. `None` = no thread known or no claim.
    pub codex_thread_url: Option<String>,
    /// Whether the local Codex store holds that thread's rollout, so the
    /// Codex app can open the link. `Some(false)` is a Codex Cloud thread.
    /// `None` together with `codex_thread_url`.
    pub codex_thread_local: Option<bool>,
    /// When the live follower's oldest unacknowledged delivery was emitted.
    /// A heartbeat proves the reader process, not the conversation behind
    /// it; a backlog that only grows is the overlay's "alive but not
    /// reading" signal. `None` = no backlog, no follower, or no claim.
    pub unread_since: Option<SystemTime>,
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

pub fn channel_open_label(digit: u8) -> String {
    format!("CHANNEL {digit} OPEN — mic is live")
}

/// Pointer label flashed for a chord on a vacant digit. Three words on
/// purpose: the pointer preview keeps only the tail of what it is given.
pub fn channel_refusal_label(digit: u8) -> String {
    format!("channel {digit} empty")
}

/// macOS's own refusal sound, played at the Settings volume.
pub(crate) const CHANNEL_REFUSAL_SOUND: &str = "Basso";

/// How long the refusal dot and its label stay beside the pointer; matches
/// the pointer preview's own freshness window.
pub(crate) const CHANNEL_REFUSAL_FLASH: Duration = Duration::from_millis(1200);

/// What the user gets back for a chord the controller refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelRefusalFeedback {
    /// System sound to play at the Settings volume; `None` stays quiet.
    pub cue: Option<&'static str>,
    /// Text flashed beside the pointer dot; `None` leaves the pointer alone.
    pub cursor_label: Option<String>,
}

impl ChannelRefusalFeedback {
    const NONE: Self = Self {
        cue: None,
        cursor_label: None,
    };
}

/// Maciej, 2026-10-17 01:41Z: Ctrl+digit on a vacant channel answered with
/// nothing — the only signal was a system notification the app never checks
/// it may post. A vacant slot is a refused user action, so it answers like
/// one: the refusal sound and one flash of a labelled pointer dot. A missing
/// binding file means no agent has attached yet, so it is vacant too. Only a
/// user chord (`Live`) is answered; an attached open is programmatic. A
/// malformed binding or any other failure is a broken runtime and stays with
/// the bridge's error path (tray + notification).
pub(crate) fn channel_refusal_feedback(
    digit: u8,
    error: &anyhow::Error,
    mode: ChannelOpenMode,
    hold_indicator: bool,
) -> ChannelRefusalFeedback {
    let vacant = matches!(
        error.downcast_ref::<ChannelOpenRefusal>(),
        Some(ChannelOpenRefusal::DigitUnbound { .. } | ChannelOpenRefusal::BindingMissing { .. })
    );
    if !vacant || !matches!(mode, ChannelOpenMode::Live) {
        return ChannelRefusalFeedback::NONE;
    }
    ChannelRefusalFeedback {
        cue: Some(CHANNEL_REFUSAL_SOUND),
        cursor_label: hold_indicator.then(|| channel_refusal_label(digit)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelOpenMode {
    /// Acquire, start the device when idle, and run the channel session.
    Live,
    /// Register the subscriber and the open-channel record. No device start.
    /// Test seam: exercised by this module's tests; prod opens `Live`.
    #[cfg(test)]
    AttachedOnly,
}

/// Why a channel session ended: the `reason` of its one `sealed` receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelSealReason {
    /// Utterance silence delivered the words. A one-message session ends as
    /// a hang-up does; a continuous one only seals that message. Also the
    /// voiceless close.
    Silence,
    /// Fn+digit pressed again: the Founder hung up and nothing reopens it.
    Hangup,
    /// Capture is handed to dictation, without extending each channel's feed.
    DictationHandoff,
    /// The process that opened the session ended without closing it (quit,
    /// crash, or a build that wrote no hang-up row). The next controller
    /// start seals it; no microphone of this process ever belonged to it.
    Orphan,
}

impl ChannelSealReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Silence => "silence",
            Self::Hangup | Self::DictationHandoff => "hangup",
            Self::Orphan => "orphan",
        }
    }
}

/// `state` of the row that closes one message of a continuous session. A
/// follower delivers that message on it; the session's open microphone, its
/// `opened_at` and the overlay's open channel are not this row's to close.
pub(crate) const SEGMENT_SEALED_STATE: &str = "segment_sealed";

/// What a settled close ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelClose {
    /// The whole session: its last message and the session's `sealed` row.
    Session,
    /// One message of a continuous session; the session keeps listening.
    Message,
}

/// What a closed message settles from: its released capture, its retention
/// lease, the bus it falls back to, and its place in the delivery order.
pub(crate) struct SettleTail {
    pub shared_bus: PathBuf,
    pub closed: Option<crate::audio::streaming_recorder::ClosedChannel>,
    pub retained_capture:
        Option<Arc<codescribe_core::state::history::audio_retention::CaptureLease>>,
    pub order: DeliveryOrder,
}

/// Delivery order of one session's messages: wait for `prior`, signal `done`.
pub(crate) struct DeliveryOrder {
    pub prior: Option<tokio::sync::oneshot::Receiver<()>>,
    pub done: Option<tokio::sync::oneshot::Sender<()>>,
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

/// Named captures still open within the last `window` bytes of `bus`.
/// Replacement captures can open while their predecessors drain; only a
/// non-open row for the same channel and session closes a capture. Streams
/// one row at a time and retains only unclosed captures. A missing bus is empty.
fn unclosed_channel_sessions(
    bus: &Path,
    window: u64,
) -> std::io::Result<BTreeMap<(String, String), serde_json::Value>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let schema = crate::presentation::agent_ack::CHANNEL_SESSION_SCHEMA;
    let file = match crate::presentation::transcript_bus_maintenance::generation::Reader::open(bus)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(error),
    };
    let start = file.len().saturating_sub(window);
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
        let Some(channel) = row.get("channel").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(session) = row.get("session_id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if channel.is_empty() || session.is_empty() {
            continue;
        }
        let key = (channel.to_string(), session.to_string());
        match row.get("state").and_then(serde_json::Value::as_str) {
            Some("open") => {
                latest.insert(key, row);
            }
            Some(_) => {
                latest.remove(&key);
            }
            None => {}
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

/// Seals every named capture still open on its bus unless owned by this
/// process (`live`). A successor opening on the same channel does not settle
/// a predecessor's decoder tail. Each orphan seal closes only its named
/// capture, making the next startup idempotent.
fn seal_orphaned_sessions(
    buses: &[PathBuf],
    live: &BTreeSet<String>,
    window: u64,
) -> Vec<OrphanSeal> {
    let mut sealed = Vec::new();
    for bus in buses {
        let latest = match unclosed_channel_sessions(bus, window) {
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
    anyhow::Error::new(error)
}

fn archive_agent_with_helper(request: AgentArchiveRequest) -> Result<String> {
    const RECEIPT_LIMIT: usize = 65_536;
    let bus = request
        .bus
        .to_str()
        .ok_or_else(|| anyhow!("agent archive bus path is not UTF-8"))?;
    let channel = request.channel.to_string();
    // The app creates this request from RealAgentBridgeInstaller.commandURL
    // after verifying its managed command marker; bus text never selects it.
    // Values below are separate fixed-option argv entries, never shell source.
    // nosemgrep: rust.actix.command-injection.rust-actix-command-injection.rust-actix-command-injection -- app-owned managed helper path, argv-only spawn; no HTTP, transcript or agent-tool command input.
    let mut child = Command::new(&request.executable)
        .args([
            "--archive-agent",
            &channel,
            "--provider",
            &request.provider,
            "--session",
            &request.provider_session_id,
            "--lease",
            &request.lease_id,
            "--bus",
            bus,
            "--bridge-home",
        ])
        .arg(&request.bridge_home)
        .args(request.release_reader.then_some("--release-reader"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // The helper names its refusal on stderr ("agent reader is alive;
        // channel retained"); the drawer shows that sentence, not a code.
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let killed = child.kill();
                let waited = child.wait();
                killed?;
                waited?;
                return Err(anyhow!("agent archive helper timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let waited = child.wait();
                waited?;
                return Err(error.into());
            }
        }
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(pipe) = child.stderr.take() {
            let _ = pipe.take(RECEIPT_LIMIT as u64).read_to_string(&mut stderr);
        }
        let reason = stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.trim_start_matches("cs-bus: ").to_string())
            .unwrap_or_else(|| format!("agent archive helper failed: {status}"));
        return Err(anyhow!("{reason}"));
    }
    let output = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("agent archive helper returned no receipt"))?;
    let mut bytes = Vec::new();
    output
        .take((RECEIPT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > RECEIPT_LIMIT {
        return Err(anyhow!("agent archive receipt exceeds its size limit"));
    }
    let receipt: serde_json::Value = serde_json::from_slice(&bytes)?;
    if receipt["schema"].as_str() != Some("codescribe.agent-archive.v1")
        || receipt["released"].as_bool() != Some(true)
        || receipt["provider"].as_str() != Some(request.provider.as_str())
        || receipt["provider_session_id"].as_str() != Some(request.provider_session_id.as_str())
        || receipt["lease_id"].as_str() != Some(request.lease_id.as_str())
        || receipt["channel"].as_str() != Some(channel.as_str())
        || receipt["bus"].as_str() != Some(bus)
    {
        return Err(anyhow!(
            "agent archive receipt does not match the frozen owner"
        ));
    }
    Ok(String::from_utf8(bytes)?)
}

impl RecordingController {
    /// Share the hotkey/autoseal lifecycle lock through canonical binding release.
    /// The blocking task owns the guard, so cancelling a foreign waiter cannot
    /// admit capture while its already-started helper still changes the binding.
    pub async fn archive_agent_channel(&self, request: AgentArchiveRequest) -> Result<String> {
        if !(1..=9).contains(&request.channel)
            || request.provider.is_empty()
            || request.provider_session_id.is_empty()
            || request.lease_id.is_empty()
            || !request.bus.is_absolute()
            || !request.executable.is_absolute()
            || !request.bridge_home.is_absolute()
        {
            return Err(anyhow!("invalid frozen agent archive request"));
        }
        let serial = Arc::clone(&self.serial_lock).lock_owned().await;
        {
            let channels = self.agent_channels.lock().await;
            if channels.contains_key(&request.channel) {
                return Err(anyhow!("cannot archive an open agent channel"));
            }
        }
        tokio::task::spawn_blocking(move || {
            let _serial = serial;
            archive_agent_with_helper(request)
        })
        .await
        .map_err(|error| anyhow!("agent archive task failed: {error}"))?
    }

    /// Modifier+digit toggle. A second press of the same digit hangs up: the
    /// session seals with `reason: hangup` and does not reopen. Dictation
    /// state is not changed: the channel modifier never records (one key, one
    /// role, resolved by the config loader), so a running take is never this
    /// chord's prefix.
    pub async fn toggle_agent_channel(&self, digit: u8) -> Result<()> {
        let state_before = self.current_state().await;
        let result = self
            .toggle_agent_channel_at(
                digit,
                &binding_path(),
                &crate::presentation::transcript_bus::transcript_bus_path(),
                ChannelOpenMode::Live,
            )
            .await;
        let action = match &result {
            Ok(true) => "opened",
            Ok(false) => "closed",
            Err(_) => "refused",
        };
        tracing::info!(
            "hotkey_decision gesture=channel_chord channel={digit} state={} action={action}",
            state_before.to_ipc_str()
        );
        result.map(|_| ())
    }

    async fn toggle_agent_channel_at(
        &self,
        digit: u8,
        binding_file: &Path,
        shared_bus: &Path,
        mode: ChannelOpenMode,
    ) -> Result<bool> {
        let _serial = Arc::clone(&self.serial_lock).lock_owned().await;
        let state = self.current_state().await;
        if state == super::State::Idle {
            self.cancel_pending_hold_start().await;
        }
        let dispatched = self
            .dispatch_agent_channel(digit, binding_file, shared_bus, mode)
            .await;
        let opened = match dispatched {
            Ok(opened) => opened,
            Err(error) => {
                self.answer_channel_refusal(digit, &error, mode).await;
                return Err(error);
            }
        };
        if opened && matches!(mode, ChannelOpenMode::Live) {
            let settings = self.runtime_settings_arc().await;
            if settings.values().beep_on_start {
                crate::audio::play_sound_with_volume("Pop", settings.values().sound_volume);
            }
        }
        Ok(opened)
    }

    /// Play and flash what a refused chord earns (see
    /// [`channel_refusal_feedback`]). The error still travels to the bridge,
    /// which logs it and leaves the tray alone for a vacant slot.
    async fn answer_channel_refusal(
        &self,
        digit: u8,
        error: &anyhow::Error,
        mode: ChannelOpenMode,
    ) {
        let config = self.get_config().await;
        let feedback = channel_refusal_feedback(digit, error, mode, config.hold_indicator);
        if let Some(cue) = feedback.cue {
            crate::audio::play_sound_with_volume(cue, config.sound_volume);
        }
        if let Some(label) = feedback.cursor_label {
            hold_badge::flash_hold_badge(
                hold_badge::HoldBadgeConfig::from_mode_with_base_diameter(
                    BadgeMode::Refused,
                    f64::from(config.hold_badge_size),
                ),
                &label,
                CHANNEL_REFUSAL_FLASH,
            );
        }
    }

    /// `shared_bus` carries the rows and receipts of a channel whose binding
    /// names no dedicated bus.
    pub(crate) async fn dispatch_agent_channel(
        &self,
        digit: u8,
        binding_file: &Path,
        shared_bus: &Path,
        mode: ChannelOpenMode,
    ) -> Result<bool> {
        let mut channels = self.agent_channels.lock().await;
        self.freeze_closing_cohort(channels.get(&digit));
        if let Some(open) = channels.remove(&digit) {
            self.publish_cohort(&channels);
            drop(channels);
            self.close_open_channel(digit, open, ChannelSealReason::Hangup, shared_bus)
                .await?;
            return Ok(false);
        }

        if self.shutdown_requested.load(Ordering::SeqCst) {
            return Err(anyhow!("capture admission closed for shutdown"));
        }

        let bound = resolve_digit(digit, binding_file).map_err(refusal)?;
        let mut recipients = frozen_channel_recipients(digit, binding_file, shared_bus);
        if digit != 0 {
            recipients.retain(|recipient| {
                recipient["provider"].as_str() == bound.provider.as_deref()
                    && recipient["provider_session_id"].as_str()
                        == bound.provider_session_id.as_deref()
                    && recipient["audience"].as_str() == Some(bound.audience.as_str())
            });
        }
        // Fn+0 includes the shared bus and the admitted owners' destinations.
        let mut broadcast_buses = Vec::new();
        if digit == 0 {
            for recipient in &recipients {
                if let Some(bus) = recipient["bus"].as_str().map(PathBuf::from)
                    && bus != shared_bus
                    && !broadcast_buses.contains(&bus)
                {
                    broadcast_buses.push(bus);
                }
            }
        }
        let runtime_settings = self.runtime_settings_arc().await;
        let timing = ChannelTiming::from_settings(&runtime_settings);
        let silence_sec = timing.utterance_silence_sec();

        let opened_at = SystemTime::now();
        let speech_clock = Arc::new(StdMutex::new(ChannelSpeechClock::default()));
        let last_text;
        let refinement_warnings;
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = recorder_guard.as_mut().ok_or_else(|| {
            anyhow!("agent channel refused: recording controller has no recorder")
        })?;

        let (subscriber, session_id, document_id, transcript_bus, capture_ledger, emitter, source) =
            match mode {
                #[cfg(test)]
                ChannelOpenMode::AttachedOnly => {
                    last_text = Arc::new(StdMutex::new(String::new()));
                    refinement_warnings = Arc::new(StdMutex::new(Vec::new()));
                    (
                        recorder.register_channel_feed().id,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                ChannelOpenMode::Live => {
                    let session_label = format!("agent-channel-{digit}-{}", uuid::Uuid::new_v4());
                    // A continuous session names each message's document;
                    // a one-message session's only document is the session.
                    let document_id = if timing.continuous() {
                        segment_document_id(&session_label, 1)
                    } else {
                        session_label.clone()
                    };
                    super::begin_audio_capture(
                        &document_id,
                        runtime_settings.values().audio_retention,
                    )
                    .await?;
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
                    hold_badge::update_transcript(cursor_token, &channel_open_label(digit));
                    let source = Arc::new(ChannelSegmentSource {
                        label: session_label.clone(),
                        audience: bound.audience.clone(),
                        bus: bound
                            .bus
                            .clone()
                            .unwrap_or_else(|| shared_bus.to_path_buf()),
                        broadcast_buses,
                        recipients,
                        runtime_settings: Arc::clone(&runtime_settings),
                        language: runtime_settings
                            .values()
                            .whisper_language
                            .whisper_hint()
                            .map(str::to_string),
                        cursor_token,
                        generation: Arc::new(AtomicU64::new(1)),
                    });
                    let parts =
                        open_channel_segment(&source, document_id.clone(), &speech_clock, digit);
                    last_text = Arc::clone(&parts.last_text);
                    refinement_warnings = Arc::clone(&parts.refinement_warnings);
                    let sink: Arc<dyn EventSink> = parts.emitter.clone();
                    // The first channel may own the physical microphone without a
                    // dictation take. Install its measured level tap before the
                    // recorder freezes the callback; a later subscriber keeps the
                    // tap already feeding the shared capture.
                    let opens_physical_capture = !recorder.recorder.is_active();
                    Self::configure_level_broadcast(recorder, self.event_broadcast.clone());
                    match recorder
                        .begin_channel_session(
                            document_id.clone(),
                            Arc::clone(&runtime_settings),
                            sink,
                            source.language.clone(),
                            silence_sec,
                            Arc::clone(&parts.ledger),
                        )
                        .await
                    {
                        Ok(id) => (
                            id,
                            Some(session_label),
                            Some(document_id),
                            Some(parts.transcript_bus),
                            Some(parts.ledger),
                            Some(parts.emitter),
                            Some(source),
                        ),
                        Err(error) => {
                            if opens_physical_capture {
                                recorder.set_level_callback(None);
                            }
                            super::finish_audio_capture(Some(document_id.as_str()));
                            if channels.is_empty() {
                                hold_badge::hide_hold_badge();
                            }
                            return Err(error);
                        }
                    }
                }
            };

        self.follow_cohort(digit, transcript_bus.as_ref());
        let audience = bound.audience.clone();
        let published_clock = Arc::clone(&speech_clock);
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
                speech_clock,
                session_id: document_id,
                channel_session_id: session_id,
                segment_opened_at: opened_at,
                segment_index: 1,
                timing,
                segment_source: source,
                emitter,
                delivery_turn: Arc::new(StdMutex::new(None)),
                mode,
                bus: bound.bus,
                last_text,
                refinement_warnings,
                transcript_bus,
                capture_ledger,
            },
        );
        self.publish_cohort(&channels);
        drop(recorder_guard);
        drop(channels);
        // The tray glyph mirrors the pointer badge: any open channel listens in
        // the Assistive lane even though the controller state stays Idle. The
        // flag goes up regardless of state so a take ending mid-channel cannot
        // blacken the dot (tray_status's Idle guard); the dot itself is only
        // repainted here when no foreign take owns the lifecycle.
        crate::os::tray_status::set_agent_channels_listening(true);
        if self.current_state().await == super::State::Idle {
            crate::os::tray_status::set_tray_indicator_mode(BadgeMode::Assistive);
            crate::os::tray_status::update_tray_status(
                crate::os::tray_status::TrayStatus::Listening,
            );
        }
        if matches!(mode, ChannelOpenMode::Live) {
            // Before the open receipt: the guard never sees this session open
            // without its phase file.
            if let Some(session) = receipt_session.clone() {
                published_clock
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .publish_to(channel_speech_now_path(digit), digit, session, opened_at);
            }
            let line = crate::presentation::agent_ack::channel_session_line(
                &crate::presentation::agent_ack::ChannelSessionLine {
                    state: "open",
                    reason: "opened",
                    channel: &digit.to_string(),
                    agent: &audience,
                    session_id: receipt_session.as_deref(),
                    autoseal_secs: timing.autoseal_secs(),
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
        Ok(true)
    }

    /// Epoch `begin_channel_session` stamps on every live channel capture.
    const CHANNEL_CAPTURE_EPOCH: u64 = 1;

    /// Observer for the channel ledger this session already owns.
    ///
    /// Called only after `end_channel_session` has joined, while `open` still
    /// holds that ledger. `AttachedOnly` and a missing ledger stay explicitly
    /// unavailable: identity that was not proven on this channel is omitted,
    /// and engine, seal, and metrics are not invented.
    fn channel_take_observer(
        open: &OpenAgentChannel,
    ) -> codescribe_core::pipeline::take_truth::TakeTruth {
        use codescribe_core::pipeline::take_truth::TakeTruth;
        if !matches!(open.mode, ChannelOpenMode::Live) {
            return TakeTruth::unavailable_evidence(None, None);
        }
        let Some(session_id) = open.session_id.clone() else {
            return TakeTruth::unavailable_evidence(None, Some(Self::CHANNEL_CAPTURE_EPOCH));
        };
        let Some(ledger) = open.capture_ledger.as_ref() else {
            return TakeTruth::unavailable_evidence(
                Some(session_id),
                Some(Self::CHANNEL_CAPTURE_EPOCH),
            );
        };
        let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
        TakeTruth::observe_ledger(&ledger, &session_id, Self::CHANNEL_CAPTURE_EPOCH)
    }

    /// Release the channel's capture interest before recognition/archive drain.
    /// The original open record and lease remain owned by its tracked tail;
    /// only that tail publishes its seal, after its producer has joined.
    async fn close_open_channel(
        &self,
        digit: u8,
        open: OpenAgentChannel,
        reason: ChannelSealReason,
        shared_bus: &Path,
    ) -> Result<()> {
        let stopped_at = Instant::now();
        open.speech_clock
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .withdraw_now();
        let retained_capture =
            super::retainable_session_id(open.session_id.as_deref()).and_then(|id| {
                codescribe_core::state::history::audio_retention::capture(&Config::config_dir(), id)
            });
        let shared_bus = shared_bus.to_path_buf();
        let live = matches!(open.mode, ChannelOpenMode::Live);
        let recorder = Arc::clone(&self.recorder);
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let prior = open
            .delivery_turn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let settlement = async move {
            let closed = {
                let mut recorder_guard = recorder.lock().await;
                match recorder_guard.as_mut() {
                    Some(recorder) => {
                        if live && reason == ChannelSealReason::Hangup {
                            recorder
                                .await_stop_pause(Some(open.subscriber), stopped_at)
                                .await;
                        }
                        if recorder.capture_subscriber_count() == 1
                            && !recorder.has_take_subscriber()
                        {
                            recorder.set_level_callback(None);
                        }
                        Some(recorder.close_channel_session(open.subscriber).await)
                    }
                    None => None,
                }
            };
            let _ = closed_tx.send(());
            Self::settle_closed_channel(
                digit,
                open,
                reason,
                ChannelClose::Session,
                SettleTail {
                    shared_bus,
                    closed,
                    retained_capture,
                    order: DeliveryOrder { prior, done: None },
                },
            )
            .await
        };
        if live {
            // Own the physical close before the first wait, including grace.
            // Dropping the foreign caller must not orphan an open PCM feed.
            let task = tokio::spawn(async move {
                if let Err(error) = settlement.await {
                    tracing::warn!(%error, digit, "released channel terminal tail failed");
                }
            });
            self.track_closed_capture_tail(task);
        } else {
            settlement.await?;
        }
        closed_rx
            .await
            .map_err(|_| anyhow!("channel capture close task ended before its fence"))?;
        // Only this foreground may hide the old badge. A cancelled caller's
        // retained tail never changes a replacement channel's UI.
        let state = self.current_state().await;
        let channels = self.agent_channels.lock().await;
        if channels.is_empty() {
            // The glyph's Idle guard stands down with the last channel; during
            // a dictation handoff the take's own lifecycle repaints the tray.
            crate::os::tray_status::set_agent_channels_listening(false);
        }
        if channels.is_empty() && state == super::State::Idle {
            hold_badge::hide_hold_badge();
            crate::os::tray_status::update_tray_status(crate::os::tray_status::TrayStatus::Idle);
        }
        Ok(())
    }

    fn track_closed_capture_tail(&self, task: tokio::task::JoinHandle<()>) {
        let mut tails = self
            .closed_capture_tails
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        tails.retain(|task| !task.is_finished());
        tails.push(task);
    }

    /// A continuous session's pause after heard voice: seal and deliver the
    /// message, and start the next one on the same capture subscription. The
    /// physical capture, the subscriber count and the session's open row stay
    /// as they are; only the message's document, ledger and archive turn over.
    /// The previous message settles in its own tail and never holds the
    /// microphone or the next message.
    async fn continue_channel_after_message(&self, digit: u8, shared_bus: &Path) -> Result<()> {
        let mut channels = self.agent_channels.lock().await;
        let Some(open) = channels.get_mut(&digit) else {
            return Ok(());
        };
        let next_index = open.segment_index.saturating_add(1);
        let next_opened_at = SystemTime::now();
        let mut previous = open.clone();
        previous.delivery_turn = Arc::new(StdMutex::new(None));
        let closed = match open.segment_source.clone() {
            Some(source) => {
                let document_id = segment_document_id(&source.label, next_index);
                super::begin_audio_capture(
                    &document_id,
                    source.runtime_settings.values().audio_retention,
                )
                .await?;
                source.generation.fetch_add(1, Ordering::SeqCst);
                let parts =
                    open_channel_segment(&source, document_id.clone(), &open.speech_clock, digit);
                let sink: Arc<dyn EventSink> = parts.emitter.clone();
                let rotated = {
                    let mut recorder_guard = self.recorder.lock().await;
                    match recorder_guard.as_mut() {
                        Some(recorder) => {
                            recorder
                                .rotate_channel_segment(
                                    open.subscriber,
                                    crate::audio::streaming_recorder::ChannelSessionSpec {
                                        session_id: document_id.clone(),
                                        runtime_settings: Arc::clone(&source.runtime_settings),
                                        event_sink: sink,
                                        language: source.language.clone(),
                                        acoustic_ledger: Arc::clone(&parts.ledger),
                                    },
                                    open.silence_sec,
                                )
                                .await
                        }
                        None => Err(anyhow!("recording controller has no recorder")),
                    }
                };
                let closed = match rotated {
                    Ok(closed) => closed,
                    Err(error) => {
                        // The previous message is still current; the
                        // session's close delivers it.
                        super::finish_audio_capture(Some(document_id.as_str()));
                        return Err(error);
                    }
                };
                if let Some(bus) = &previous.transcript_bus {
                    let cohort = Arc::clone(
                        &self
                            .agent_channel_cohort
                            .read()
                            .unwrap_or_else(|error| error.into_inner()),
                    );
                    bus.freeze_co_audience(cohort);
                }
                open.session_id = Some(document_id);
                open.last_text = parts.last_text;
                open.refinement_warnings = parts.refinement_warnings;
                open.capture_ledger = Some(parts.ledger);
                open.emitter = Some(parts.emitter);
                open.transcript_bus = Some(parts.transcript_bus);
                Some(closed)
            }
            None => {
                open.last_text = Arc::new(StdMutex::new(String::new()));
                open.refinement_warnings = Arc::new(StdMutex::new(Vec::new()));
                None
            }
        };
        // The turned-over message delivers after the one before it; the next
        // delivery of this session waits for it in turn.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let prior = open
            .delivery_turn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .replace(done_rx);
        let order = DeliveryOrder {
            prior,
            done: Some(done_tx),
        };
        open.segment_index = next_index;
        open.segment_opened_at = next_opened_at;
        {
            let mut clock = open
                .speech_clock
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            clock.speech = super::speech_clock::SpeechClock::default();
            clock.publish_now(next_opened_at);
        }
        let follow = open.transcript_bus.clone();
        // Co-audience names live captures; this one now has a new document.
        self.publish_cohort(&channels);
        drop(channels);
        self.follow_cohort(digit, follow.as_ref());

        let retained_capture = super::retainable_session_id(previous.session_id.as_deref())
            .and_then(|id| {
                codescribe_core::state::history::audio_retention::capture(&Config::config_dir(), id)
            });
        let live = matches!(previous.mode, ChannelOpenMode::Live);
        let settlement = Self::settle_closed_channel(
            digit,
            previous,
            ChannelSealReason::Silence,
            ChannelClose::Message,
            SettleTail {
                shared_bus: shared_bus.to_path_buf(),
                closed,
                retained_capture,
                order,
            },
        );
        if live {
            let task = tokio::spawn(async move {
                if let Err(error) = settlement.await {
                    tracing::warn!(%error, digit, "channel message settlement failed");
                }
            });
            self.track_closed_capture_tail(task);
            Ok(())
        } else {
            settlement.await
        }
    }

    /// Finish one message before delivery (`agent_channel.segment_finish`).
    /// Runs after the message's producer joined, so its reducer is terminal.
    /// A formatter that does not land within `format_wait_sec`, fails, or is
    /// refused as `stale_source` leaves the Light+ text, named on the row.
    async fn finish_channel_message(
        open: &OpenAgentChannel,
        committed: Option<String>,
    ) -> Option<crate::presentation::transcript_bus::ChannelDelivery> {
        use crate::presentation::transcript_bus::ChannelDelivery;
        let committed = committed?;
        let light_plus = || light_plus_terminal_punctuation(&committed);
        match open.timing.finish {
            ChannelSegmentFinish::Off => Some(ChannelDelivery {
                text: committed.clone(),
                revision: "committed",
                fallback: None,
            }),
            ChannelSegmentFinish::LightPlus => Some(ChannelDelivery {
                text: light_plus(),
                revision: "light_plus",
                fallback: None,
            }),
            ChannelSegmentFinish::Formatter => {
                let fallback = |reason| ChannelDelivery {
                    text: light_plus(),
                    revision: "light_plus",
                    fallback: Some(reason),
                };
                let (Some(emitter), Some(source)) =
                    (open.emitter.as_ref(), open.segment_source.as_ref())
                else {
                    return Some(fallback("no_formatter_authority"));
                };
                let Some(request) = emitter.terminal_formatter_request() else {
                    return Some(fallback("no_terminal_document"));
                };
                let settings = &source.runtime_settings;
                let policy = settings.formatting_policy();
                let language = settings.values().whisper_language;
                let formatted = tokio::time::timeout(
                    open.timing.format_wait,
                    codescribe_core::llm::ai_formatting::format_text_with_status_for_policy(
                        &request.source_text,
                        language.whisper_hint(),
                        settings,
                        None,
                    ),
                )
                .await;
                let Ok(result) = formatted else {
                    tracing::info!(
                        wait = ?open.timing.format_wait,
                        "channel message formatter missed its budget; Light+ delivered"
                    );
                    return Some(fallback("formatter_timeout"));
                };
                match emitter.apply_formatter_revision(
                    request.session_id,
                    request.source_revision,
                    policy,
                    result,
                ) {
                    Ok(_) => match emitter
                        .delivery_projection(policy, &request.source_text)
                        .map(|projection| projection.rendered_text)
                        .filter(|text| !text.trim().is_empty())
                    {
                        Some(text) => Some(ChannelDelivery {
                            text,
                            revision: "formatter",
                            fallback: None,
                        }),
                        None => Some(fallback("stale_source")),
                    },
                    Err(refusal) => {
                        tracing::info!(%refusal, "channel message formatter refused; Light+ delivered");
                        use crate::presentation::emitter::UserRevisionRefusal;
                        Some(fallback(match refusal {
                            UserRevisionRefusal::StaleRevision { .. }
                            | UserRevisionRefusal::SessionMismatch => "stale_source",
                            UserRevisionRefusal::FormatterUnavailable => "formatter_unavailable",
                            UserRevisionRefusal::FormatterNoop => "formatter_noop",
                            _ => "formatter_failed",
                        }))
                    }
                }
            }
        }
    }

    async fn settle_closed_channel(
        digit: u8,
        open: OpenAgentChannel,
        reason: ChannelSealReason,
        close: ChannelClose,
        tail: SettleTail,
    ) -> Result<()> {
        let SettleTail {
            shared_bus,
            closed,
            retained_capture,
            order,
        } = tail;
        let stopped = match closed {
            Some(closed) => closed.finish().await.map(|(_, path)| path),
            None => Ok(None),
        };
        let (retained_audio, failure_detail) = match stopped {
            Ok(path) => (path, None),
            Err(error) => {
                tracing::warn!(%error, digit, "channel producer/archive failed after capture release");
                if let Some(lease) = retained_capture.as_ref() {
                    lease.protect_retry();
                }
                let failure =
                    error.downcast_ref::<crate::audio::streaming_recorder::CaptureStopFailure>();
                let mut path = failure.and_then(|failure| failure.audio_path.clone());
                let archive = failure
                    .and_then(|failure| {
                        failure
                            .cause
                            .downcast_ref::<codescribe_core::audio::recorder::CaptureArchiveError>()
                    })
                    .cloned();
                if let Some(archive) = archive {
                    if let Some(lease) = retained_capture.as_ref()
                        && let Some(source) = archive.source_path.as_ref()
                    {
                        let _ = lease.record(std::slice::from_ref(source), false);
                    }
                    match tokio::task::spawn_blocking(move || {
                        let recovered = archive.recover_complete_archive()?;
                        archive.acknowledge_recovery(&recovered)?;
                        Ok::<_, anyhow::Error>(recovered.path().to_path_buf())
                    })
                    .await
                    {
                        Ok(Ok(recovered)) => path = Some(recovered),
                        other => tracing::warn!(
                            ?other,
                            digit,
                            "closed channel complete archive recovery failed"
                        ),
                    }
                }
                (
                    path,
                    Some(format!("channel terminal processing failed: {error:#}")),
                )
            }
        };
        // The producer joined above. Finish the message (formatter work may
        // run while an earlier message of this session is still settling),
        // then wait for that earlier delivery so messages never reorder.
        let delivery = match open.transcript_bus.as_ref() {
            Some(bus) => Self::finish_channel_message(&open, bus.channel_closing_text()).await,
            None => None,
        };
        let DeliveryOrder { prior, done } = order;
        if let Some(prior) = prior {
            let _ = prior.await;
        }
        // Publish the final held snapshot before retention can yield and
        // before any non-open channel receipt flushes it.
        if let Some(bus) = open.transcript_bus.as_ref() {
            let settings = Config::load_startup_runtime_snapshot(false);
            if let Some(ledger) = open.capture_ledger.as_ref() {
                let ledger = ledger.lock().unwrap_or_else(|error| error.into_inner());
                bus.observe_channel_publication_evidence(&ledger);
            }
            if let Err(error) =
                bus.publish_channel_closed(settings.agent_send_hold_sec(), delivery.as_ref())
            {
                tracing::warn!(%error, digit, "channel seal snapshot persistence failed");
            }
        }
        // The channel task has joined. Freeze its ledger before `open` drops
        // that handle. This is not the dictation recorder's ledger.
        let observer = Self::channel_take_observer(&open);
        if let Some(path) = retained_audio.as_deref() {
            // W5 retention parity: a capture the channel owned joins the same
            // take store and retention rules as dictation. The path is the
            // message's own subscriber-scoped archive; the device spill spans
            // other sessions and never enters this retention. The slug carries
            // the heard words; a voiceless close archives as no-speech.
            let heard = open
                .last_text
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            super::retain_session_audio_with_lease(
                open.session_id.as_deref(),
                path,
                match failure_detail.as_deref() {
                    Some(detail) => {
                        codescribe_core::state::SessionTranscriptArchive::Unavailable(detail)
                    }
                    None => codescribe_core::state::SessionTranscriptArchive::Committed(&heard),
                },
                retained_capture.clone(),
                &observer,
            )
            .await;
        }
        let channel = digit.to_string();
        let autoseal_secs = open.timing.autoseal_secs();
        let session = crate::presentation::agent_ack::ChannelSessionLine {
            state: "open",
            reason: "opened",
            channel: &channel,
            agent: &open.audience,
            session_id: open.channel_session_id.as_deref(),
            autoseal_secs,
            opened_at: open.opened_at,
            utterance_silence_sec: open.silence_sec,
            provider: open.provider.as_deref(),
            provider_session_id: open.provider_session_id.as_deref(),
        };
        let mut refinement_warnings = open
            .refinement_warnings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(detail) = failure_detail {
            refinement_warnings.push(detail);
        }
        let mut rows = Vec::new();
        // A continuous session's message closes under its own document; a
        // one-message session's document is the session and its seal closes both.
        let message_document = open.session_id != open.channel_session_id;
        if message_document {
            let mut row = seal_receipt_line(
                reason,
                &crate::presentation::agent_ack::ChannelSessionLine {
                    session_id: open.session_id.as_deref(),
                    opened_at: open.segment_opened_at,
                    ..session
                },
                &refinement_warnings,
            );
            row["state"] = serde_json::json!(SEGMENT_SEALED_STATE);
            row["loud"] = serde_json::json!(false);
            row["channel_session_id"] = serde_json::json!(open.channel_session_id);
            row["segment_index"] = serde_json::json!(open.segment_index);
            if let Some(delivery) = &delivery {
                row["delivered_revision"] = serde_json::json!(delivery.revision);
            }
            rows.push(row);
        }
        if close == ChannelClose::Session {
            rows.push(seal_receipt_line(
                reason,
                &session,
                if message_document {
                    &[]
                } else {
                    &refinement_warnings
                },
            ));
        }
        let mut receipt = Ok(());
        for row in &rows {
            if let Some(bus) = open.transcript_bus.as_ref() {
                bus.record_channel_receipt(row);
            } else if let Err(error) = crate::presentation::agent_ack::append_json_line(
                open.bus.as_deref().unwrap_or(&shared_bus),
                row,
            ) {
                receipt = Err(error);
            }
        }
        super::finish_audio_capture(open.session_id.as_deref());
        drop(retained_capture);
        if let Some(done) = done {
            let _ = done.send(());
        }
        Ok(receipt?)
    }

    /// Startup reconciliation of the channel-session ledger. A session whose
    /// `open` row has no close for its own identity belongs to a
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
            .flat_map(|open| [open.channel_session_id.clone(), open.session_id.clone()])
            .flatten()
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
        // A reply `bus-demux` plays through afplay is the agent, not the
        // Founder: channel feeds drop the microphone while it plays.
        crate::audio::tts_duck::watch_external_playback(
            active_names::bridge_home()
                .join("runtime")
                .join(crate::audio::tts_duck::EXTERNAL_PLAYBACK_MARKER),
        );
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
        let _serial = self.serial_lock.lock().await;
        self.seal_channels_silent_for(now, bus).await
    }

    /// Ordinary capture admission holds the controller's serial lock. Release
    /// each channel feed before admission; tracked tails join and publish its
    /// existing close receipt independently of the next capture.
    pub(crate) async fn close_agent_channels_for_dictation(&self) -> Result<()> {
        let mut open: Vec<_> = {
            let mut channels = self.agent_channels.lock().await;
            self.freeze_closing_cohort(channels.values());
            let open: Vec<_> = channels.drain().collect();
            self.publish_cohort(&channels);
            open
        };
        open.sort_by_key(|(digit, _)| *digit);
        let bus = crate::presentation::transcript_bus::transcript_bus_path();
        let mut first_error = None;
        for (digit, channel) in open {
            let retained = channel.clone();
            if let Err(error) = self
                .close_open_channel(digit, channel, ChannelSealReason::DictationHandoff, &bus)
                .await
            {
                let mut channels = self.agent_channels.lock().await;
                self.follow_cohort(digit, retained.transcript_bus.as_ref());
                channels.insert(digit, retained);
                self.publish_cohort(&channels);
                drop(channels);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The pause after heard voice (`agent_channel.send_pause_sec`, frozen in
    /// each channel's timing when it opened) sends the message. Under
    /// `end_after_message` the send also ends the session through the close a
    /// hang-up uses, so the capture is released and the next message is the
    /// next Fn+digit. Under `continuous` the session keeps its capture and its
    /// open row; the next message starts on the same subscription. A session
    /// that hears no voice for `voiceless_close_sec` since its current message
    /// began ends either way; agent playback into it does not count. A send
    /// pause of `0` (only via `CODESCRIBE_CHANNEL_AUTOSEAL_SECS`) turns both off.
    pub(crate) async fn seal_channels_silent_for(&self, now: SystemTime, bus: &Path) -> Vec<u8> {
        let playback = crate::audio::tts_duck::channel_capture_should_drop();
        let (due, continuing) = {
            let mut channels = self.agent_channels.lock().await;
            let mut ending = Vec::new();
            let mut continuing = Vec::new();
            for (digit, open) in channels.iter() {
                let mut clock = open
                    .speech_clock
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if playback {
                    clock.speech.note_agent_playback(now);
                }
                if clock.speech.silence_due(now, open.timing.send_pause) {
                    if open.timing.continuous() {
                        continuing.push(*digit);
                    } else {
                        ending.push(*digit);
                    }
                } else if clock.speech.voiceless_due(
                    open.segment_opened_at,
                    now,
                    open.timing.voiceless_close,
                ) {
                    ending.push(*digit);
                }
            }
            self.freeze_closing_cohort(ending.iter().filter_map(|digit| channels.get(digit)));
            let due: Vec<(u8, OpenAgentChannel)> = ending
                .into_iter()
                .filter_map(|digit| channels.remove(&digit).map(|open| (digit, open)))
                .collect();
            if !due.is_empty() {
                self.publish_cohort(&channels);
            }
            (due, continuing)
        };
        let mut sealed = Vec::new();
        for (digit, open) in due {
            if let Err(error) = self
                .close_open_channel(digit, open, ChannelSealReason::Silence, bus)
                .await
            {
                tracing::warn!(%error, digit, "channel auto-seal could not close the capture");
                continue;
            }
            sealed.push(digit);
        }
        for digit in continuing {
            match self.continue_channel_after_message(digit, bus).await {
                Ok(()) => sealed.push(digit),
                Err(error) => {
                    // The message cannot turn over: end the session instead
                    // of leaving its pause due forever.
                    tracing::warn!(%error, digit, "channel message rotation failed; ending session");
                    let open = {
                        let mut channels = self.agent_channels.lock().await;
                        self.freeze_closing_cohort(channels.get(&digit));
                        let open = channels.remove(&digit);
                        self.publish_cohort(&channels);
                        open
                    };
                    if let Some(open) = open
                        && self
                            .close_open_channel(digit, open, ChannelSealReason::Silence, bus)
                            .await
                            .is_ok()
                    {
                        sealed.push(digit);
                    }
                }
            }
        }
        sealed
    }

    pub async fn channel_hud_states(&self) -> Vec<ChannelHudState> {
        let ducking = crate::audio::tts_duck::channel_capture_should_drop();
        let channels = self.agent_channels.lock().await;
        let mut digits: Vec<u8> = channels.keys().copied().collect();
        digits.sort_unstable();
        digits
            .into_iter()
            .filter_map(|digit| {
                let open = channels.get(&digit)?;
                let autoseal_deadline = open
                    .speech_clock
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .speech
                    .deadline(open.timing.send_pause);
                Some(ChannelHudState {
                    open: true,
                    loud: true,
                    channel: digit.to_string(),
                    audience: open.audience.clone(),
                    label: channel_open_label(digit),
                    autoseal_secs: open.timing.autoseal_secs(),
                    autoseal_deadline,
                    tts_ducking: ducking,
                    opened_at: open.opened_at,
                    utterance_silence_ms: utterance_silence_ms(open.silence_sec),
                    provider: open.provider.clone(),
                    provider_session_id: open.provider_session_id.clone(),
                    follower_alive: None,
                    frame_detected: None,
                    terminal_detected: None,
                    codex_thread_url: None,
                    codex_thread_local: None,
                    unread_since: None,
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
        let live = codescribe_core::stt::active_names::live_follower_hosts_at(
            bridge_home,
            now,
            codescribe_core::stt::active_names::LEASE_TTL_SECONDS,
        );
        let unread = codescribe_core::stt::active_names::unread_backlog_since_at(
            bridge_home,
            now,
            codescribe_core::stt::active_names::LEASE_TTL_SECONDS,
        );
        let codex_threads = codescribe_core::stt::active_names::codex_threads_at(bridge_home);
        let alive = |provider: &Option<String>, session: &Option<String>| -> Option<bool> {
            match (provider.as_deref(), session.as_deref()) {
                (Some(provider), Some(session)) => {
                    Some(live.contains_key(&(provider.to_lowercase(), session.to_string())))
                }
                _ => None,
            }
        };
        let framed = |provider: &Option<String>, session: &Option<String>| -> Option<bool> {
            let (Some(provider), Some(session)) = (provider.as_deref(), session.as_deref()) else {
                return None;
            };
            live.get(&(provider.to_lowercase(), session.to_string()))
                .map(|hosts| hosts.frame_pane)
        };
        let terminal = |provider: &Option<String>, session: &Option<String>| -> Option<bool> {
            let (Some(provider), Some(session)) = (provider.as_deref(), session.as_deref()) else {
                return None;
            };
            live.get(&(provider.to_lowercase(), session.to_string()))
                .map(|hosts| hosts.terminal_tab)
        };
        let codex_thread = |provider: &Option<String>,
                            session: &Option<String>|
         -> (Option<String>, Option<bool>) {
            let (Some(provider), Some(session)) = (provider.as_deref(), session.as_deref()) else {
                return (None, None);
            };
            codex_threads
                .get(&(provider.to_lowercase(), session.to_string()))
                .map_or((None, None), |thread| {
                    (
                        Some(format!("codex://threads/{}", thread.thread_id)),
                        Some(thread.rollout),
                    )
                })
        };
        let unread_since = |provider: &Option<String>,
                            session: &Option<String>|
         -> Option<SystemTime> {
            let (Some(provider), Some(session)) = (provider.as_deref(), session.as_deref()) else {
                return None;
            };
            unread
                .get(&(provider.to_lowercase(), session.to_string()))
                .map(|since| SystemTime::UNIX_EPOCH + Duration::from_secs_f64(since.max(0.0)))
        };
        let mut states = self.channel_hud_states().await;
        for state in &mut states {
            state.follower_alive = alive(&state.provider, &state.provider_session_id);
            state.frame_detected = framed(&state.provider, &state.provider_session_id);
            state.terminal_detected = terminal(&state.provider, &state.provider_session_id);
            (state.codex_thread_url, state.codex_thread_local) =
                codex_thread(&state.provider, &state.provider_session_id);
            state.unread_since = unread_since(&state.provider, &state.provider_session_id);
        }
        if !states.iter().any(|state| state.channel == "0") {
            states.push(ChannelHudState {
                open: false,
                loud: false,
                channel: "0".into(),
                audience: BROADCAST_AUDIENCE.into(),
                label: String::new(),
                autoseal_secs: 0,
                autoseal_deadline: None,
                tts_ducking: false,
                opened_at: SystemTime::UNIX_EPOCH,
                utterance_silence_ms: 0,
                provider: None,
                provider_session_id: None,
                follower_alive: None,
                frame_detected: None,
                terminal_detected: None,
                codex_thread_url: None,
                codex_thread_local: None,
                unread_since: None,
            });
        }
        let Ok(file) = load_binding(binding) else {
            states.sort_by(|a, b| a.channel.cmp(&b.channel));
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
            let (codex_thread_url, codex_thread_local) = codex_thread(&provider, &session);
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
                frame_detected: framed(&provider, &session),
                terminal_detected: terminal(&provider, &session),
                codex_thread_url,
                codex_thread_local,
                unread_since: unread_since(&provider, &session),
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
#[path = "agent_channel_tests.rs"]
mod tests;
