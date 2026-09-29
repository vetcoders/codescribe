//! Fn+digit agent channel.
//!
//! The channel is not a dictation take and not a `State` variant. It subscribes
//! to the shared capture, seals on a repeated digit, on its own utterance
//! silence, or after `CODESCRIBE_CHANNEL_AUTOSEAL_SECS` without new channel
//! text, and stamps `audience` on Bus rows. Paste and the overlay document
//! stay off. The hold badge is the preview; `ChannelHudState` is the open-mic
//! fact the overlay paints from.

use std::collections::BTreeMap;
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
}

impl ChannelSealReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Silence => "silence",
            Self::Hangup => "hangup",
        }
    }
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
    let bus = entry
        .bus
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Ok(BoundAgentSession {
        audience: audience.to_string(),
        provider: Some(provider.to_string()),
        provider_session_id: Some(provider_session_id.to_string()),
        bus,
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
        let runtime_settings = self.runtime_settings_arc().await;
        let silence_sec = runtime_settings.values().toggle_silence_sec;
        let opened_at = SystemTime::now();
        let last_voice_at = Arc::new(StdMutex::new(opened_at));
        let last_text = Arc::new(StdMutex::new(String::new()));
        let mut session_id = None;
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = recorder_guard.as_mut().ok_or_else(|| {
            anyhow!("agent channel refused: recording controller has no recorder")
        })?;

        let subscriber = match mode {
            ChannelOpenMode::AttachedOnly => recorder.register_channel_feed().id,
            ChannelOpenMode::Live => {
                let session_label = format!("agent-channel-{digit}-{}", uuid::Uuid::new_v4());
                session_id = Some(session_label.clone());
                let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
                let sentence_pause = runtime_settings.values().light_plus_sentence_pause_sec;
                let language = runtime_settings
                    .values()
                    .whisper_language
                    .whisper_hint()
                    .map(str::to_string);
                // W5: a binding with a dedicated bus routes this channel's
                // rows there; without one the shared bus stays authoritative.
                let bus = Some(Arc::new(TranscriptBus::open_with_path(
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
                )));
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
                        bus,
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
                    .with_sentence_pause_sec(sentence_pause),
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
        let receipt_bus = bound.bus.clone();
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
            let open_receipt_bus = receipt_bus.as_deref().unwrap_or(shared_bus);
            if let Err(error) =
                crate::presentation::agent_ack::append_json_line(open_receipt_bus, &line)
            {
                tracing::warn!(%error, digit, "channel open receipt was not appended");
            }
        }
        Ok(())
    }

    /// The one closing throne of a channel session, for silence and hang-up
    /// alike. It consumes the open record, which its caller removed from
    /// `agent_channels` exactly once, so a session gets at most one `sealed`
    /// receipt. The receipt follows the capture close: `end_channel_session`
    /// joins the transcription task, so every evidence row of the session is
    /// already on the bus when a follower reads the boundary. It does not
    /// wait for a ledger terminal seal: a take whose coverage was refused has
    /// no other row that releases its words.
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
            );
        }
        let line = crate::presentation::agent_ack::channel_session_line(
            &crate::presentation::agent_ack::ChannelSessionLine {
                state: "sealed",
                reason: reason.as_str(),
                channel: &digit.to_string(),
                agent: &open.audience,
                session_id: open.session_id.as_deref(),
                autoseal_secs,
                opened_at: open.opened_at,
                utterance_silence_sec: open.silence_sec,
                provider: open.provider.as_deref(),
                provider_session_id: open.provider_session_id.as_deref(),
            },
        );
        let seal_receipt_bus = open.bus.as_deref().unwrap_or(shared_bus);
        if let Err(error) =
            crate::presentation::agent_ack::append_json_line(seal_receipt_bus, &line)
        {
            tracing::warn!(
                %error,
                digit,
                reason = reason.as_str(),
                "channel seal receipt was not appended"
            );
        }
        let state = self.current_state().await;
        let channels = self.agent_channels.lock().await;
        if channels.is_empty() && state == super::State::Idle {
            hold_badge::hide_hold_badge();
        }
        Ok(())
    }

    /// Ack watcher plus the channel silence cap. Started once, from the live
    /// controller, so unit tests that only construct a controller do not scan
    /// the real bridge home.
    pub fn spawn_channel_guards(self: &Arc<Self>, handle: tokio::runtime::Handle) {
        if self.channel_guards_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let controller = Arc::clone(self);
        handle.spawn(async move {
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
                })
            })
            .collect()
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
}
