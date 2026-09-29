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
pub const CHANNEL_AUTOSEAL_SECS_DEFAULT: u64 = 120;

pub fn channel_open_label(digit: u8) -> String {
    format!("CHANNEL {digit} OPEN — mic is live")
}

/// Silence cap for an open channel session.
///
/// Unset or unreadable values use [`CHANNEL_AUTOSEAL_SECS_DEFAULT`]. `0`
/// disables the cap: a zero threshold would seal on the opening tick.
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
    })
}

fn refusal(error: ChannelOpenRefusal) -> anyhow::Error {
    anyhow!("{error}")
}

impl RecordingController {
    /// Fn+digit toggle. A second press of the same digit seals and closes.
    /// Dictation state is not changed.
    pub async fn toggle_agent_channel(&self, digit: u8) -> Result<()> {
        self.dispatch_agent_channel(digit, &binding_path(), ChannelOpenMode::Live)
            .await
    }

    pub(crate) async fn dispatch_agent_channel(
        &self,
        digit: u8,
        binding_file: &Path,
        mode: ChannelOpenMode,
    ) -> Result<()> {
        let mut channels = self.agent_channels.lock().await;
        if let Some(open) = channels.remove(&digit) {
            drop(channels);
            self.close_open_channel(open).await?;
            return Ok(());
        }

        let bound = resolve_digit(digit, binding_file).map_err(refusal)?;
        let runtime_settings = self.runtime_settings_arc().await;
        let silence_sec = runtime_settings.values().toggle_silence_sec;
        let opened_at = SystemTime::now();
        let last_voice_at = Arc::new(StdMutex::new(opened_at));
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
                let bus = TranscriptBus::open(TranscriptSession {
                    session_id: session_label.clone(),
                    mode: TranscriptMode::Agent,
                    has_latched_target: false,
                    latched_target_is_self: false,
                    audience: Some(bound.audience.clone()),
                    badge_only: true,
                })
                .map(Arc::new);
                hold_badge::show_badge_for_mode(BadgeMode::Assistive);
                let cursor_token = hold_badge::take_token();
                let open_label = channel_open_label(digit);
                hold_badge::update_transcript(cursor_token, &open_label, false);
                let voice_clock = Arc::clone(&last_voice_at);
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
            if let Err(error) = crate::presentation::agent_ack::append_json_line(
                &crate::presentation::transcript_bus::transcript_bus_path(),
                &line,
            ) {
                tracing::warn!(%error, digit, "channel open receipt was not appended");
            }
        }
        Ok(())
    }

    async fn close_open_channel(&self, open: OpenAgentChannel) -> Result<()> {
        {
            let mut recorder_guard = self.recorder.lock().await;
            if let Some(recorder) = recorder_guard.as_mut() {
                recorder.end_channel_session(open.subscriber).await?;
            }
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
        self.seal_channels_silent_for(now, bus, channel_autoseal_secs())
            .await
    }

    pub(crate) async fn seal_channels_silent_for(
        &self,
        now: SystemTime,
        bus: &Path,
        secs: u64,
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
                    silence_is_due(last, now, secs)
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
            let channel = digit.to_string();
            let agent = open.audience.clone();
            let session_id = open.session_id.clone();
            let opened_at = open.opened_at;
            let silence_sec = open.silence_sec;
            let provider = open.provider.clone();
            let provider_session_id = open.provider_session_id.clone();
            if let Err(error) = self.close_open_channel(open).await {
                tracing::warn!(%error, digit, "channel auto-seal could not close the capture");
                continue;
            }
            let line = crate::presentation::agent_ack::channel_session_line(
                &crate::presentation::agent_ack::ChannelSessionLine {
                    state: "sealed",
                    reason: "silence",
                    channel: &channel,
                    agent: &agent,
                    session_id: session_id.as_deref(),
                    autoseal_secs: secs,
                    opened_at,
                    utterance_silence_sec: silence_sec,
                    provider: provider.as_deref(),
                    provider_session_id: provider_session_id.as_deref(),
                },
            );
            if let Err(error) = crate::presentation::agent_ack::append_json_line(bus, &line) {
                tracing::warn!(%error, digit, "channel silence receipt was not appended");
            }
            sealed.push(digit);
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
        let error = controller
            .dispatch_agent_channel(4, &path, ChannelOpenMode::AttachedOnly)
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
    async fn second_press_closes_the_channel_and_leaves_dictation_idle() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        );
        controller
            .dispatch_agent_channel(3, &path, ChannelOpenMode::AttachedOnly)
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
            .dispatch_agent_channel(3, &path, ChannelOpenMode::Live)
            .await
            .expect("second press closes");
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        let (count, channel, _) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        assert_eq!(controller.current_state().await, super::super::State::Idle);
    }

    #[tokio::test]
    async fn conversation_refuses_while_a_channel_is_open() {
        let controller = RecordingController::new_without_keychain();
        let dir = tempfile::tempdir().expect("temp");
        let path = write_binding(
            dir.path(),
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude","provider_session_id":"ada-session"}}}"#,
        );
        controller
            .dispatch_agent_channel(1, &path, ChannelOpenMode::AttachedOnly)
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
        controller
            .dispatch_agent_channel(3, &path, ChannelOpenMode::AttachedOnly)
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

        let bus = dir.path().join("bus.jsonl");
        let opened = controller
            .agent_channel_snapshot(3)
            .await
            .expect("open")
            .opened_at;
        let still_open = controller
            .seal_channels_silent_for(opened + Duration::from_secs(10), &bus, 120)
            .await;
        assert!(still_open.is_empty());
        assert!(controller.agent_channel_snapshot(3).await.is_some());
        let disabled = controller
            .seal_channels_silent_for(opened + Duration::from_secs(10_000), &bus, 0)
            .await;
        assert!(disabled.is_empty(), "0 disables the cap");
        assert!(controller.channel_hud_states().await[0].loud);

        let sealed = controller
            .seal_channels_silent_for(opened + Duration::from_secs(120), &bus, 120)
            .await;
        assert_eq!(sealed, vec![3]);
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        assert!(controller.channel_hud_states().await.is_empty());
        let (count, channel, _) = controller.capture_subscriber_view().await;
        assert_eq!(count, 0);
        assert!(!channel);
        let again = controller
            .seal_channels_silent_for(opened + Duration::from_secs(500), &bus, 120)
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
}
