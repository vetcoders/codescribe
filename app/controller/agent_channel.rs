//! Agent channel: non-take capture consumer keyed by Fn+digit.
//!
//! A channel opens its own capture session, ledger, and reducer projection. It
//! stamps every sealed transcript row with an `audience` tag so bus followers
//! can route it without parsing the spoken text. Live preview is shown only
//! through the cursor badge; the tray and global assistive flag are untouched.

use crate::audio::streaming_recorder::{ChannelCaptureSession, StreamingRecorder};
use crate::config::Config;
use crate::os::hold_badge::{BadgeMode, show_badge_for_mode};
use crate::presentation::transcript_bus::TranscriptSessionEndReason;
use crate::presentation::{PresentationEmitter, TranscriptBus, TranscriptMode, TranscriptSession};
use anyhow::{Context, Result};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Stable filename for digit → audience bindings under the app data root.
pub const AGENT_AUDIENCE_BINDINGS_FILE: &str = "agent-audience-bindings.json";

/// Why an agent channel could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOpenRefusal {
    /// Digit outside 0–9.
    InvalidDigit,
    /// The audience binding file cannot be read or parsed.
    BindingUnavailable,
    /// Digit is not present in the binding file (and is not the broadcast 0).
    DigitUnbound,
}

impl fmt::Display for ChannelOpenRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDigit => write!(f, "agent channel digit must be 0-9"),
            Self::BindingUnavailable => {
                write!(f, "agent audience binding file is missing or unreadable")
            }
            Self::DigitUnbound => write!(f, "digit has no audience binding"),
        }
    }
}

impl std::error::Error for ChannelOpenRefusal {}

/// File-backed digit → audience map.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AgentAudienceBindingFile {
    /// `digit` is stored as a string key (JSON objects cannot have integer
    /// keys in some encoders). Values are audience names; `"*"` is broadcast.
    #[serde(default)]
    pub bindings: std::collections::HashMap<String, String>,
}

impl AgentAudienceBindingFile {
    /// Load bindings from the app data root. Missing file is not an error; it
    /// simply means no digits are bound.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read agent audience bindings at {}", path.display()))?;
        let file: Self = serde_json::from_str(&text)
            .with_context(|| format!("parse agent audience bindings at {}", path.display()))?;
        Ok(file)
    }

    /// Production path under `Config::config_dir()`.
    pub fn path() -> PathBuf {
        Config::config_dir().join(AGENT_AUDIENCE_BINDINGS_FILE)
    }
}

/// One active agent channel.
pub struct AgentChannel {
    pub digit: u8,
    pub audience: String,
    pub capture: ChannelCaptureSession,
}

/// Resolve the audience for a digit.
///
/// - `0` → broadcast audience `"*"`; the binding file is never consulted.
/// - `1`–`9` → look up the digit in the binding file.
pub fn resolve_channel_audience(
    digit: u8,
    binding: Option<&AgentAudienceBindingFile>,
) -> Result<String, ChannelOpenRefusal> {
    if digit == 0 {
        return Ok("*".to_string());
    }
    if !(1..=9).contains(&digit) {
        return Err(ChannelOpenRefusal::InvalidDigit);
    }
    let Some(binding) = binding else {
        return Err(ChannelOpenRefusal::BindingUnavailable);
    };
    let key = digit.to_string();
    binding
        .bindings
        .get(&key)
        .cloned()
        .ok_or(ChannelOpenRefusal::DigitUnbound)
}

/// Build the isolated presentation/reducer sink for one channel.
///
/// The returned emitter is the channel's [`EventSink`]. Its cursor observer
/// writes into the hold badge using a private token, never touching the global
/// assistive session flag.
pub fn build_channel_event_sink(
    audience: String,
    session_id: String,
    sentence_pause_sec: f32,
) -> (Arc<PresentationEmitter>, Arc<TranscriptBus>) {
    let transcript_buffer = Arc::new(tokio::sync::Mutex::new(String::new()));
    let bus = TranscriptBus::open(TranscriptSession {
        session_id: session_id.clone(),
        mode: TranscriptMode::Agent,
        has_latched_target: false,
        latched_target_is_self: false,
        audience: Some(audience.clone()),
    })
    .unwrap_or_else(|| {
        // Observability failure is not fatal; open the fallback path so capture
        // and badge preview still work.
        TranscriptBus::open_at(
            TranscriptSession {
                session_id: session_id.clone(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
                audience: Some(audience.clone()),
            },
            std::path::PathBuf::from("/dev/null"),
            None,
        )
        .unwrap_or_else(|_| {
            TranscriptBus::open(TranscriptSession {
                session_id: session_id.clone(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
                audience: Some(audience.clone()),
            })
            .expect("transcript bus fallback must not fail twice")
        })
    });
    let bus = Arc::new(bus);
    let cursor_token = crate::os::hold_badge::take_token();
    let emitter = Arc::new(
        PresentationEmitter::new_with_authority(
            transcript_buffer,
            None,
            None,
            Some(Arc::clone(&bus)),
            None,
            None,
        )
        .with_cursor_observer(Arc::new(move |projection| {
            crate::os::hold_badge::update_transcript(
                cursor_token,
                &projection.text,
                projection.degraded,
            );
        }))
        .with_sentence_pause_sec(sentence_pause_sec),
    );
    (emitter, bus)
}

/// Load the binding file for one digit, packaging the refusal reason.
pub fn load_binding_for_digit(digit: u8) -> Result<AgentAudienceBindingFile, ChannelOpenRefusal> {
    if digit == 0 {
        // Digit 0 is broadcast and never needs the file.
        return Ok(AgentAudienceBindingFile::default());
    }
    AgentAudienceBindingFile::load().map_err(|_| ChannelOpenRefusal::BindingUnavailable)
}

/// Show the low-level badge for an open channel without touching the global
/// assistive session flag.
pub fn show_channel_badge() {
    show_badge_for_mode(BadgeMode::Assistive);
}

/// Hide the low-level badge. Safe to call even if no channel badge is visible.
pub fn hide_channel_badge() {
    crate::os::hold_badge::hide_hold_badge();
}

/// Close one channel and release its badge if it was the last open channel.
pub async fn close_agent_channel(
    channels: &Mutex<std::collections::HashMap<u8, AgentChannel>>,
    digit: u8,
    recorder: &Mutex<Option<StreamingRecorder>>,
) -> Result<()> {
    let mut channels_guard = channels.lock().await;
    let Some(channel) = channels_guard.remove(&digit) else {
        debug!(digit, "close_agent_channel: channel was not open");
        return Ok(());
    };
    drop(channels_guard);

    info!(digit, audience = %channel.audience, "closing agent channel");
    let mut recorder_guard = recorder.lock().await;
    if let Some(recorder) = recorder_guard.as_mut() {
        if let Err(error) = recorder.stop_channel_capture_session(channel.capture).await {
            warn!(digit, %error, "channel stop failed");
        }
    }
    drop(recorder_guard);

    // Hide badge only when no channels remain.
    let channels_guard = channels.lock().await;
    if channels_guard.is_empty() {
        hide_channel_badge();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(map: &[(&str, &str)]) -> AgentAudienceBindingFile {
        AgentAudienceBindingFile {
            bindings: map
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn resolve_channel_audience_digit_zero_is_broadcast() {
        assert_eq!(resolve_channel_audience(0, None).unwrap(), "*");
    }

    #[test]
    fn resolve_channel_audience_invalid_digit_fails() {
        assert_eq!(
            resolve_channel_audience(10, Some(&binding(&[("10", "x")]))).unwrap_err(),
            ChannelOpenRefusal::InvalidDigit
        );
    }

    #[test]
    fn resolve_channel_audience_missing_binding_fails() {
        let file = binding(&[("1", "thread-a"), ("2", "thread-b")]);
        assert_eq!(
            resolve_channel_audience(3, Some(&file)).unwrap_err(),
            ChannelOpenRefusal::DigitUnbound
        );
    }

    #[test]
    fn resolve_channel_audience_bound_digit_returns_audience() {
        let file = binding(&[("1", "thread-a"), ("2", "thread-b")]);
        assert_eq!(
            resolve_channel_audience(1, Some(&file)).unwrap(),
            "thread-a"
        );
        assert_eq!(
            resolve_channel_audience(2, Some(&file)).unwrap(),
            "thread-b"
        );
    }

    #[test]
    fn resolve_channel_audience_no_file_fails_for_nonzero() {
        assert_eq!(
            resolve_channel_audience(1, None).unwrap_err(),
            ChannelOpenRefusal::BindingUnavailable
        );
    }
}
