//! Fn+digit agent channel.
//!
//! The channel is not a dictation take and not a `State` variant. It subscribes
//! to the shared capture, seals on a repeated digit or on its own utterance
//! silence, and stamps `audience` on Bus rows. Paste and the overlay document
//! stay off. The hold badge is the only preview.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelPreview {
    pub autopaste: bool,
    pub overlay_document: bool,
    pub hold_badge: bool,
}

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
struct BindingFile {
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenAgentChannel {
    pub subscriber: CaptureSubscriberId,
    pub audience: String,
    pub silence_sec: f32,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelOpenMode {
    /// Acquire, start the device when idle, and run the channel session.
    Live,
    /// Register the subscriber and the open-channel record. No device start.
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
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = recorder_guard.as_mut().ok_or_else(|| {
            anyhow!("agent channel refused: recording controller has no recorder")
        })?;

        let subscriber = match mode {
            ChannelOpenMode::AttachedOnly => recorder.register_channel_feed().id,
            ChannelOpenMode::Live => {
                let session_id = format!("agent-channel-{digit}-{}", uuid::Uuid::new_v4());
                let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
                let sentence_pause = runtime_settings.values().light_plus_sentence_pause_sec;
                let language = runtime_settings
                    .values()
                    .whisper_language
                    .whisper_hint()
                    .map(str::to_string);
                let bus = TranscriptBus::open(TranscriptSession {
                    session_id: session_id.clone(),
                    mode: TranscriptMode::Agent,
                    has_latched_target: false,
                    latched_target_is_self: false,
                    audience: Some(bound.audience.clone()),
                    badge_only: true,
                })
                .map(Arc::new);
                hold_badge::show_badge_for_mode(BadgeMode::Assistive);
                let cursor_token = hold_badge::take_token();
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
                        session_id,
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

        channels.insert(
            digit,
            OpenAgentChannel {
                subscriber,
                audience: bound.audience,
                silence_sec,
                provider: bound.provider,
                provider_session_id: bound.provider_session_id,
            },
        );
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

    pub(crate) async fn agent_channel_snapshot(&self, digit: u8) -> Option<OpenAgentChannel> {
        self.agent_channels.lock().await.get(&digit).cloned()
    }

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
        assert!(!CHANNEL_PREVIEW.autopaste);
        assert!(!CHANNEL_PREVIEW.overlay_document);
        assert!(CHANNEL_PREVIEW.hold_badge);
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
}
